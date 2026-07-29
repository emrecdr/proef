//! The execution session: embedded `run_entries` over the scenario's artifact
//! (ADR-0001, ADR-0010, TECH-SPEC §5).
//!
//! The artifact text **is** the executed input: it is parsed once per session
//! and every batch runs a contiguous slice of its entries (`run_entries` takes
//! an entries slice + the full content — verified). Because `run_entries`
//! builds its HTTP client per call, batches chain state across calls:
//! variables via `HurlResult.variables` (lossless) and cookies via a
//! Netscape-format temp file (`CookieStore::to_netscape` →
//! `cookie_input_file`), the `SessionState` mechanics of TECH-SPEC §5.
//!
//! Budgets (ADR-0007): hurl cannot be interrupted mid-call, so each batch gets
//! a wall-clock budget from its parsed `[Options]` (timeout × (retry+1) +
//! retry intervals + margin); the orchestrator's watchdog abandons the thread
//! when it expires.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use hurl::runner::{self, RunnerOptionsBuilder, VariableSet};
use hurl::util::logger::{Logger, LoggerOptionsBuilder};
use hurl::util::term::{Stderr, Stdout, WriteMode};
use hurl_core::ast::{CountOption, DurationOption, HurlFile, OptionKind, SectionValue};
use hurl_core::error::DisplaySourceError;
use hurl_core::types::{Count, DurationUnit};

use proef_core::cancel::CancellationToken;
use proef_core::engine::{ArtifactRef, EngineSession, HttpDefaults, ScenarioCtx};
use proef_core::error::EngineError;
use proef_core::event::{Event, EventSink};
use proef_core::step::{BatchResult, Status, StepBatch, StepOutcome};
use proef_core::world::{Value as WorldValue, World};

/// Margin added to every computed batch budget (ADR-0007).
const BUDGET_MARGIN: Duration = Duration::from_secs(5);

pub(crate) struct HurlSession {
    artifact: ArtifactRef,
    secrets: Arc<BTreeMap<String, String>>,
    http: HttpDefaults,
    file_root: Option<std::path::PathBuf>,
    scenario: Arc<str>,
    parsed: Option<HurlFile>,
    chained: Option<VariableSet>,
    cookie_dir: Option<tempfile::TempDir>,
    cookie_file: Option<std::path::PathBuf>,
}

impl HurlSession {
    pub(crate) fn open(ctx: &ScenarioCtx) -> Result<Self, EngineError> {
        let artifact = ctx.artifact.clone().ok_or_else(|| {
            EngineError::setup("the hurl engine executes artifacts — none was provided")
        })?;
        Ok(Self {
            artifact,
            secrets: Arc::clone(&ctx.secrets),
            http: ctx.http,
            file_root: ctx.file_root.clone(),
            scenario: Arc::clone(&ctx.scenario),
            parsed: None,
            chained: None,
            cookie_dir: None,
            cookie_file: None,
        })
    }

    fn ensure_parsed(&mut self) -> Result<(), EngineError> {
        if self.parsed.is_none() {
            // ADR-0010: this is byte-for-byte the artifact text.
            let file = hurl_core::parser::parse_hurl_file(&self.artifact.text).map_err(|err| {
                EngineError::setup(format!(
                    "emitted artifact does not parse (line {}): {:?}",
                    err.pos.line, err.kind
                ))
            })?;
            self.parsed = Some(file);
        }
        Ok(())
    }

    /// The parsed-entry indices belonging to each of the `step_count` steps of
    /// batch `ordinal` (via the sidecar map's explicit batch/step indices and
    /// line ranges — a step without a map entry yields an empty set).
    fn entries_per_step(&self, ordinal: usize, step_count: usize) -> Vec<Vec<usize>> {
        let Some(parsed) = &self.parsed else {
            return Vec::new();
        };
        let ranges: Vec<[usize; 2]> = (0..step_count)
            .map(|step| {
                self.artifact
                    .map
                    .entries
                    .iter()
                    .find(|entry| entry.batch == ordinal && entry.step == step)
                    .map_or([0, 0], |entry| entry.hurl_lines)
            })
            .collect();
        // Partition, never overlap: each parsed entry is anchored by its
        // *request line* (`Request.source_info` spans leading comments, so a
        // comment-only step's range would overlap the next entry and the same
        // request would run twice — one authored POST, two sent). A claimed
        // set guarantees disjointness even if ranges are pathological.
        let mut claimed: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        ranges
            .iter()
            .map(|[start, end]| {
                let owned: Vec<usize> = parsed
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(index, entry)| {
                        let request_line = entry.request.url.source_info.start.line;
                        request_line >= *start && request_line <= *end && !claimed.contains(index)
                    })
                    .map(|(index, _)| index)
                    .collect();
                claimed.extend(owned.iter().copied());
                owned
            })
            .collect()
    }

    fn seed_variables(&self, world: &World) -> VariableSet {
        let mut variables = self.chained.clone().unwrap_or_default();
        for (name, value) in world.merged() {
            variables.insert(name.to_owned(), to_hurl_value(value));
        }
        for (name, value) in self.secrets.iter() {
            variables.insert_secret(name.clone(), value.clone());
        }
        variables
    }

    fn runner_options(&self) -> runner::RunnerOptions {
        let mut builder = RunnerOptionsBuilder::new();
        builder.timeout(Duration::from_millis(self.http.timeout_ms));
        builder.connect_timeout(Duration::from_millis(self.http.timeout_ms.min(10_000)));
        if self.http.follow_location {
            builder.follow_location(hurl::http::FollowLocation::Follow(
                hurl::http::CredentialForwarding::default(),
            ));
        }
        if let Some(path) = &self.cookie_file
            && path.exists()
        {
            builder.cookie_input_file(Some(path.display().to_string()));
        }
        if let Some(file_root) = &self.file_root {
            // Confine file bodies to the feature's directory (§13).
            let current = std::env::current_dir().unwrap_or_else(|_| ".".into());
            builder.context_dir(&hurl::util::path::ContextDir::new(&current, file_root));
        }
        builder.build()
    }

    /// Feature anchor + artifact span for the map entry of (ordinal, step) —
    /// selected by the sidecar's explicit indices, never positionally.
    fn anchor(&self, ordinal: usize, step_index: usize) -> Option<(&StepRefLike, [usize; 2])> {
        self.artifact
            .map
            .entries
            .iter()
            .find(|entry| entry.batch == ordinal && entry.step == step_index)
            .map(|entry| (&entry.feature, entry.hurl_lines))
    }
}

type StepRefLike = proef_core::emit::FeatureAnchor;

impl EngineSession for HurlSession {
    // One cohesive listing of the batch lifecycle; splitting hides the order.
    #[allow(clippy::too_many_lines)]
    fn run_batch(
        &mut self,
        batch: &StepBatch,
        world: &mut World,
        events: &EventSink,
        _cancel: &CancellationToken,
    ) -> BatchResult {
        // The scenario-wide batch ordinal — the sidecar's `batch` key. Never a
        // per-session counter: with interleaved engines the session only sees
        // *its* batches, and a local count would select the wrong sidecar rows.
        let ordinal = batch.index;

        if let Err(err) = self.ensure_parsed() {
            return BatchResult {
                steps: Vec::new(),
                error: Some(err),
            };
        }
        let step_entries = self.entries_per_step(ordinal, batch.steps.len());
        let mut outcomes: Vec<StepOutcome> = Vec::new();
        let mut batch_error: Option<EngineError> = None;

        // Steps to execute (skip empty `when:` guards), preserving order.
        let mut plan: Vec<(usize, &proef_core::step::LoweredStep, &[usize])> = Vec::new();
        for (index, step) in batch.steps.iter().enumerate() {
            let guarded_off = step
                .when
                .as_ref()
                .is_some_and(|guard| guard.0.trim().is_empty());
            if guarded_off {
                outcomes.push(StepOutcome {
                    step: step.step.clone(),
                    status: Status::Skipped,
                    attempts: 0,
                    duration: Duration::ZERO,
                    detail: Some("skipped by `when:` guard".to_owned()),
                    artifact_span: None,
                });
                emit_step(
                    events,
                    &self.scenario,
                    batch,
                    step,
                    Status::Skipped,
                    0,
                    0,
                    &[],
                    Some("skipped by `when:` guard"),
                );
                continue;
            }
            let entries = step_entries.get(index).map_or(&[][..], Vec::as_slice);
            plan.push((index, step, entries));
        }

        // Split into contiguous entry runs (skips may create gaps).
        let mut runs: Vec<Vec<(usize, &proef_core::step::LoweredStep, &[usize])>> = Vec::new();
        for item in plan {
            match runs.last_mut() {
                Some(run) if is_contiguous(run, item.2) => run.push(item),
                _ => runs.push(vec![item]),
            }
        }

        let mut failed = false;
        for run in runs {
            if failed {
                for (_, step, _) in run {
                    outcomes.push(skipped_outcome(step));
                    emit_step(
                        events,
                        &self.scenario,
                        batch,
                        step,
                        Status::Skipped,
                        0,
                        0,
                        &[],
                        Some("not run (an earlier step in the batch failed)"),
                    );
                }
                continue;
            }
            let entry_indices: Vec<usize> =
                run.iter().flat_map(|(_, _, e)| e.iter().copied()).collect();
            let (Some(&first), Some(&last)) = (entry_indices.first(), entry_indices.last()) else {
                continue;
            };

            let variables = self.seed_variables(world);
            let options = self.runner_options();
            let mut stdout = Stdout::new(WriteMode::Buffered);
            let stderr = Stderr::new(WriteMode::Buffered);
            let logger_options = LoggerOptionsBuilder::new().color(false).build();
            let secret_values = variables.secrets();
            let mut logger = Logger::new(&logger_options, stderr, &secret_values);
            let input = hurl_core::input::Input::new(&format!("{}.hurl", self.artifact.slug));
            // Live progress (ADR-0001): hurl reports each entry attempt —
            // including retries — onto the event spine as `EntryRunning`.
            let listener = ProgressListener {
                events,
                scenario: &self.scenario,
                engine: batch.engine.as_str(),
                offset: first,
            };

            let Some(parsed) = &self.parsed else { break };
            let result = runner::run_entries(
                &parsed.entries[first..=last],
                &self.artifact.text,
                Some(&input),
                &options,
                &variables,
                &mut stdout,
                Some(&listener),
                &mut logger,
            );

            // Chain state for the next call (TECH-SPEC §5).
            self.chained = Some(result.variables.clone());
            if let Err(err) = write_cookie_file(
                &mut self.cookie_dir,
                &mut self.cookie_file,
                &result.cookie_store.to_netscape(),
            ) {
                batch_error = batch_error.or(Some(err));
            }

            // Merge captures into the World (typed), honoring saveAs: global.
            for entry_result in &result.entries {
                for capture in &entry_result.captures {
                    let value = from_hurl_value(&capture.value);
                    let target_step = run
                        .iter()
                        .find(|(_, _, entries)| {
                            entries.iter().any(|&e| {
                                parsed.entries.get(e).is_some_and(|pe| {
                                    line_in_entry(pe, entry_result.source_info.start.line)
                                })
                            })
                        })
                        .map(|(_, step, _)| *step);
                    world.set(capture.name.clone(), value.clone());
                    if let Some(step) = target_step
                        && step.save_as.contains_key(&capture.name)
                    {
                        world.set_global(capture.name.clone(), value);
                    }
                }
            }

            // Map entry results to step outcomes.
            for (index, step, entries) in &run {
                let mut per_entry_results: BTreeMap<usize, u32> = BTreeMap::new();
                let mut duration = Duration::ZERO;
                let mut captures: Vec<String> = Vec::new();
                let mut errors: Vec<String> = Vec::new();
                let mut reached = false;
                let mut assert_failure = false;
                for entry_result in &result.entries {
                    let line = entry_result.source_info.start.line;
                    let entry_index = entries.iter().copied().find(|&e| {
                        parsed
                            .entries
                            .get(e)
                            .is_some_and(|pe| line_in_entry(pe, line))
                    });
                    let Some(entry_index) = entry_index else {
                        continue;
                    };
                    reached = true;
                    *per_entry_results.entry(entry_index).or_default() += 1;
                    duration += entry_result.transfer_duration;
                    captures.extend(entry_result.captures.iter().map(|c| c.name.clone()));
                    for error in &entry_result.errors {
                        if is_assert_error(&error.kind) {
                            assert_failure = true;
                        }
                        errors.push(redact(&error.description(), &self.secrets));
                    }
                }
                // Multiple results for the *same* entry are retries: attempts
                // is the retried entry's count — never the sum across a
                // multi-entry step (a 2-entry step is 1 attempt, not 2). Only
                // the final result's errors decide (earlier ones retried).
                let attempts = per_entry_results.values().copied().max().unwrap_or(0);
                let final_failed = last_result_failed(&result, parsed, entries);
                let status = if !reached {
                    Status::Skipped
                } else if final_failed {
                    // `optional:` failures warn (the runner's aggregate
                    // status logic agrees) — emitting Failed here would make
                    // the console/record contradict the summary.
                    if step.optional {
                        Status::Warned
                    } else {
                        Status::Failed
                    }
                } else {
                    Status::Passed
                };
                captures.sort();
                captures.dedup();
                let span = self.anchor(ordinal, *index).map(|(_, lines)| lines);
                let detail = if matches!(status, Status::Failed | Status::Warned) {
                    let location = span.map_or_else(String::new, |[a, _]| {
                        format!(" (artifact {}.hurl:{a})", self.artifact.slug)
                    });
                    Some(format!("{}{location}", errors.join("; ")))
                } else {
                    None
                };
                if status == Status::Failed {
                    failed = true;
                    let message = detail.clone().unwrap_or_default();
                    batch_error = batch_error.or(Some(if assert_failure {
                        EngineError::assert_failed(message)
                    } else {
                        EngineError::infra(message)
                    }));
                }
                outcomes.push(StepOutcome {
                    step: step.step.clone(),
                    status,
                    attempts: attempts.max(u32::from(reached)),
                    duration,
                    detail: detail.clone(),
                    artifact_span: span.map(|[a, b]| {
                        (
                            u32::try_from(a).unwrap_or(u32::MAX),
                            u32::try_from(b).unwrap_or(u32::MAX),
                        )
                    }),
                });
                emit_step(
                    events,
                    &self.scenario,
                    batch,
                    step,
                    status,
                    attempts.max(u32::from(reached)),
                    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                    &captures,
                    detail.as_deref(),
                );
            }
        }

        BatchResult {
            steps: outcomes,
            error: batch_error,
        }
    }

    fn batch_budget(&mut self, batch: &StepBatch) -> Option<Duration> {
        self.ensure_parsed().ok()?;
        let ordinal = batch.index;
        let parsed = self.parsed.as_ref()?;
        let default_timeout = Duration::from_millis(self.http.timeout_ms);
        let mut budget = Duration::ZERO;
        for entries in self.entries_per_step(ordinal, batch.steps.len()) {
            for index in entries {
                let entry = parsed.entries.get(index)?;
                let (retries, interval) = entry_retry(entry);
                let timeout = entry_timeout(entry).unwrap_or(default_timeout);
                let delay = entry_delay(entry);
                // Saturating throughout: user-authored durations must never
                // panic the budget math (they cap at Duration::MAX instead).
                let attempts = retries.saturating_add(1);
                budget = budget
                    .saturating_add(timeout.saturating_mul(attempts))
                    .saturating_add(interval.saturating_mul(retries))
                    .saturating_add(delay.saturating_mul(attempts));
            }
        }
        Some(budget.saturating_add(BUDGET_MARGIN))
    }

    fn finish(&mut self) -> Result<(), EngineError> {
        self.chained = None;
        self.cookie_file = None;
        self.cookie_dir = None; // TempDir drop removes the files
        Ok(())
    }
}

/// hurl's live progress listener (ADR-0001), forwarding every entry attempt
/// onto the event spine as [`Event::EntryRunning`]. hurl reports the entry
/// index 1-based and *relative to the slice* it was given; `offset` rebases
/// it to the artifact-wide 0-based ordinal the sidecar speaks.
struct ProgressListener<'a> {
    events: &'a EventSink,
    scenario: &'a Arc<str>,
    engine: &'a str,
    offset: usize,
}

impl runner::EventListener for ProgressListener<'_> {
    fn on_entry_running(
        &self,
        current: hurl_core::types::Index,
        _last: hurl_core::types::Index,
        retry_count: usize,
    ) {
        self.events.emit(&Event::EntryRunning {
            scenario: Arc::clone(self.scenario),
            engine: Arc::from(self.engine),
            entry: self.offset + current.to_zero_based(),
            retry: u32::try_from(retry_count).unwrap_or(u32::MAX),
        });
    }
}

/// Round-trip the cookie store to a Netscape temp file for the next
/// `run_entries` call (disjoint-field borrows: callable while the parsed
/// artifact is borrowed).
fn write_cookie_file(
    dir_slot: &mut Option<tempfile::TempDir>,
    file_slot: &mut Option<std::path::PathBuf>,
    netscape: &str,
) -> Result<(), EngineError> {
    if dir_slot.is_none() {
        *dir_slot =
            Some(tempfile::TempDir::new().map_err(|err| {
                EngineError::infra("cannot create cookie temp dir").with_source(err)
            })?);
    }
    let Some(dir) = dir_slot.as_ref() else {
        return Err(EngineError::infra("cookie temp dir vanished"));
    };
    let path = dir.path().join("cookies.txt");
    std::fs::write(&path, netscape)
        .map_err(|err| EngineError::infra("cannot write cookie file").with_source(err))?;
    *file_slot = Some(path);
    Ok(())
}

/// Does an `EntryResult` reported at `result_line` belong to this parsed
/// entry? Containment within the request's comment-inclusive span (probed:
/// spans cover leading comments; result lines fall inside them).
fn line_in_entry(parsed_entry: &hurl_core::ast::Entry, result_line: usize) -> bool {
    let start = parsed_entry.request.source_info.start.line;
    let end = parsed_entry.request.source_info.end.line;
    result_line >= start && result_line <= end
}

fn skipped_outcome(step: &proef_core::step::LoweredStep) -> StepOutcome {
    StepOutcome {
        step: step.step.clone(),
        status: Status::Skipped,
        attempts: 0,
        duration: Duration::ZERO,
        detail: None,
        artifact_span: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_step(
    events: &EventSink,
    scenario: &Arc<str>,
    batch: &StepBatch,
    step: &proef_core::step::LoweredStep,
    status: Status,
    attempts: u32,
    duration_ms: u64,
    captures: &[String],
    detail: Option<&str>,
) {
    events.emit(&Event::StepFinished {
        scenario: Arc::clone(scenario),
        engine: Arc::from(batch.engine.as_str()),
        step: step.step.clone(),
        status,
        attempts,
        duration_ms,
        captures: captures.to_vec(),
        detail: detail.map(ToOwned::to_owned),
    });
}

fn is_contiguous(
    run: &[(usize, &proef_core::step::LoweredStep, &[usize])],
    next: &[usize],
) -> bool {
    let last = run
        .iter()
        .rev()
        .find_map(|(_, _, entries)| entries.last().copied());
    match (last, next.first()) {
        (Some(last), Some(&first)) => first == last + 1,
        // An entry-less step joins any run; a first entry opens one.
        (_, None) | (None, Some(_)) => true,
    }
}

/// Did the *final* attempt of any of these entries fail?
fn last_result_failed(result: &runner::HurlResult, parsed: &HurlFile, entries: &[usize]) -> bool {
    for &entry_index in entries {
        let Some(parsed_entry) = parsed.entries.get(entry_index) else {
            continue;
        };
        let last = result
            .entries
            .iter()
            .rfind(|er| line_in_entry(parsed_entry, er.source_info.start.line));
        if last.is_some_and(|er| !er.errors.is_empty()) {
            return true;
        }
    }
    false
}

fn is_assert_error(kind: &runner::RunnerErrorKind) -> bool {
    use runner::RunnerErrorKind as K;
    matches!(
        kind,
        K::AssertBodyDiffError { .. }
            | K::AssertBodyValueError { .. }
            | K::AssertFailure { .. }
            | K::AssertHeaderValueError { .. }
            | K::AssertStatus { .. }
            | K::AssertVersion { .. }
            // Query failures over the *response* are test failures too: a
            // backend answering malformed JSON/XML (or missing the queried
            // header) failed the test's expectation — proef and the
            // environment are fine (ADR-0009: exit 1, never 3).
            | K::QueryInvalidJson
            | K::QueryInvalidXml
            | K::QueryHeaderNotFound
    )
}

fn redact(text: &str, secrets: &BTreeMap<String, String>) -> String {
    let mut out = text.to_owned();
    for value in secrets.values() {
        if !value.is_empty() {
            out = out.replace(value, "***");
        }
    }
    out
}

fn to_hurl_value(value: &WorldValue) -> runner::Value {
    match value {
        WorldValue::Null => runner::Value::Null,
        WorldValue::Bool(b) => runner::Value::Bool(*b),
        WorldValue::Int(i) => runner::Value::Number(runner::Number::Integer(*i)),
        WorldValue::Float(f) => runner::Value::Number(runner::Number::Float(*f)),
        WorldValue::String(s) => runner::Value::String(s.clone()),
    }
}

fn from_hurl_value(value: &runner::Value) -> WorldValue {
    match value {
        runner::Value::Bool(b) => WorldValue::Bool(*b),
        runner::Value::Null => WorldValue::Null,
        runner::Value::Number(number) => match number {
            runner::Number::Integer(i) => WorldValue::Int(*i),
            runner::Number::Float(f) => WorldValue::Float(*f),
            other @ runner::Number::BigInteger(_) => WorldValue::String(other.to_string()),
        },
        runner::Value::String(s) => WorldValue::String(s.clone()),
        other => WorldValue::String(other.to_string()),
    }
}

/// Per-entry `[Options]` retry (finite by pack lint) and interval.
fn entry_retry(entry: &hurl_core::ast::Entry) -> (u32, Duration) {
    let mut retries = 0u32;
    let mut interval = Duration::from_secs(1);
    for section in &entry.request.sections {
        if let SectionValue::Options(options) = &section.value {
            for option in options {
                match &option.kind {
                    OptionKind::Retry(CountOption::Literal(Count::Finite(count))) => {
                        retries = u32::try_from(*count).unwrap_or(u32::MAX);
                    }
                    OptionKind::RetryInterval(DurationOption::Literal(duration)) => {
                        interval = ast_duration(duration);
                    }
                    _ => {}
                }
            }
        }
    }
    (retries, interval)
}

/// Per-entry `[Options] delay:` — an uninterruptible sleep hurl applies per
/// attempt; it must be part of the budget or the watchdog kills the scenario.
fn entry_delay(entry: &hurl_core::ast::Entry) -> Duration {
    for section in &entry.request.sections {
        if let SectionValue::Options(options) = &section.value {
            for option in options {
                if let OptionKind::Delay(DurationOption::Literal(duration)) = &option.kind {
                    return ast_duration(duration);
                }
            }
        }
    }
    Duration::ZERO
}

fn entry_timeout(entry: &hurl_core::ast::Entry) -> Option<Duration> {
    for section in &entry.request.sections {
        if let SectionValue::Options(options) = &section.value {
            for option in options {
                if let OptionKind::MaxTime(DurationOption::Literal(duration)) = &option.kind {
                    return Some(ast_duration(duration));
                }
            }
        }
    }
    None
}

fn ast_duration(duration: &hurl_core::ast::Duration) -> Duration {
    let value = duration.value.as_u64();
    match duration.unit {
        // hurl's default unit is milliseconds (retry-interval/delay).
        // Saturating: authored values must never panic the budget math.
        Some(DurationUnit::MilliSecond) | None => Duration::from_millis(value),
        Some(DurationUnit::Second) => Duration::from_secs(value),
        Some(DurationUnit::Minute) => Duration::from_secs(value.saturating_mul(60)),
        Some(DurationUnit::Hour) => Duration::from_secs(value.saturating_mul(3600)),
    }
}
