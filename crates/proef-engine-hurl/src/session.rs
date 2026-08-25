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

/// Connect-phase cap: even a suite with generous response budgets should fail
/// fast on an unreachable host — the request timeout applies on top.
const CONNECT_TIMEOUT_CAP_MS: u64 = 10_000;

pub(crate) struct HurlSession {
    artifact: ArtifactRef,
    secrets: Arc<BTreeMap<String, String>>,
    /// Variable name → secret name for this scenario (ADR-0018).
    secret_bindings: Arc<BTreeMap<String, String>>,
    redactions: proef_core::report::Redactions,
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
            redactions: proef_core::report::Redactions::new(ctx.secrets.values().cloned()),
            secrets: Arc::clone(&ctx.secrets),
            secret_bindings: Arc::clone(&ctx.secret_bindings),
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
        // Keyed by the *variable* name the artifact reads, with the value taken
        // by *secret* name — they differ when a fragment binding renamed one
        // (ADR-0018). Core owns that join so no engine can invert it.
        for (variable, value) in
            proef_core::engine::secret_variables(&self.secret_bindings, &self.secrets)
        {
            variables.insert_secret(variable.to_owned(), value.to_owned());
        }
        variables
    }

    fn runner_options(&self) -> runner::RunnerOptions {
        let mut builder = RunnerOptionsBuilder::new();
        builder.timeout(Duration::from_millis(self.http.timeout_ms));
        builder.connect_timeout(Duration::from_millis(
            self.http.timeout_ms.min(CONNECT_TIMEOUT_CAP_MS),
        ));
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

    /// Artifact line span for the map entry of (ordinal, step) — selected by
    /// the sidecar's explicit indices, never positionally.
    fn anchor(&self, ordinal: usize, step_index: usize) -> Option<[usize; 2]> {
        self.artifact
            .map
            .entries
            .iter()
            .find(|entry| entry.batch == ordinal && entry.step == step_index)
            .map(|entry| entry.hurl_lines)
    }
}

impl EngineSession for HurlSession {
    // One cohesive listing of the batch lifecycle; splitting hides the order.
    #[allow(clippy::too_many_lines)]
    fn run_batch(
        &mut self,
        batch: &StepBatch,
        world: &mut World,
        events: &EventSink,
        cancel: &CancellationToken,
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
                .is_some_and(proef_core::step::Guard::skips);
            if guarded_off {
                outcomes.push(StepOutcome {
                    detail: Some("skipped by `when:` guard".to_owned()),
                    ..skipped_outcome(step)
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
                    &[],
                    None,
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
            // Between-runs cancellation point (ADR-0007 finer grain): honor
            // Ctrl-C at run granularity instead of waiting out the budget.
            let skip_detail = if failed {
                Some("not run (an earlier step in the batch failed)")
            } else if cancel.is_cancelled() {
                Some("not run (run cancelled)")
            } else {
                None
            };
            if let Some(detail) = skip_detail {
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
                        Some(detail),
                        &[],
                        None,
                    );
                }
                continue;
            }
            let entry_indices: Vec<usize> =
                run.iter().flat_map(|(_, _, e)| e.iter().copied()).collect();
            let (Some(&first), Some(&last)) = (entry_indices.first(), entry_indices.last()) else {
                // Nothing to execute (comment-only payloads): the steps must
                // still surface in every sink — a scenario may never report
                // green while silently having run nothing (load-time lint
                // rejects this; this is the engine-side backstop).
                for (_, step, _) in run {
                    outcomes.push(StepOutcome {
                        detail: Some(NO_ENTRIES_DETAIL.to_owned()),
                        ..skipped_outcome(step)
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
                        Some(NO_ENTRIES_DETAIL),
                        &[],
                        None,
                    );
                }
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
                // A broken cookie chain would run later requests against
                // stale session state and blame the resulting assert
                // failures on the app under test — stop the batch instead.
                batch_error = batch_error.or(Some(err));
                failed = true;
            }

            // Merge captures into the World (typed), honoring saveAs: global.
            // A capture whose value IS a secret never promotes: promotions
            // persist to `.proef-state.json` in plaintext, and secrets reach
            // no sink (ADR-0005) — the refusal warns on the owning step.
            let mut refused_promotions: BTreeMap<usize, Vec<String>> = BTreeMap::new();
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
                        .map(|(index, step, _)| (*index, *step));
                    world.set(capture.name.clone(), value.clone());
                    // The World owns the promotion gate (ADR-0005): it
                    // refuses a value carrying any secret raw *or encoded* —
                    // the engine-side check this replaces matched whole-value
                    // equality only, so a composite (`Bearer <token>`) or a
                    // base64 reflection persisted to disk in plaintext.
                    if let Some((step_index, step)) = target_step
                        && step.save_as.contains_key(&capture.name)
                        && !world.set_global(capture.name.clone(), value)
                    {
                        refused_promotions
                            .entry(step_index)
                            .or_default()
                            .push(capture.name.clone());
                    }
                }
            }

            // Map entry results to step outcomes. hurl computed expected/
            // actual and the true failing line — surface them, anchored on
            // the error's own source line, not the entry's first line.
            // The line table feeds `fixme` for every rendered error — build
            // it once per batch, not per error (retries multiply errors).
            let artifact_lines: Vec<&str> = self.artifact.text.lines().collect();
            let render_error = |error: &runner::RunnerError| -> String {
                let fixme = error
                    .fixme(&artifact_lines)
                    .to_string(hurl_core::text::Format::Plain)
                    .split_whitespace()
                    .filter(|word| !word.chars().all(|c| c == '^'))
                    .collect::<Vec<_>>()
                    .join(" ");
                let rendered = format!(
                    "{} ({}.hurl:{}: {})",
                    error.description(),
                    self.artifact.slug,
                    error.source_info.start.line,
                    fixme
                );
                self.redactions.apply(&rendered)
            };
            // Lines owned by merged-asserts steps (§2.7): their errors
            // attribute to the authored `Then`, never to the host request —
            // attribution moves, it does not duplicate.
            let merged_spans: Vec<[usize; 2]> = run
                .iter()
                .filter(|(_, s, _)| {
                    matches!(
                        s.payload,
                        proef_core::step::StepPayload::MergedAsserts { .. }
                    )
                })
                .filter_map(|(i, _, _)| self.anchor(ordinal, *i))
                .collect();
            let in_merged_span = |line: usize| {
                merged_spans
                    .iter()
                    .any(|[start, end]| line >= *start && line <= *end)
            };
            for (index, step, entries) in &run {
                let span = self.anchor(ordinal, *index);
                let merged = matches!(
                    step.payload,
                    proef_core::step::StepPayload::MergedAsserts { .. }
                );
                let mut per_entry_results: BTreeMap<usize, u32> = BTreeMap::new();
                let mut duration = Duration::ZERO;
                let mut captures: Vec<String> = Vec::new();
                let mut errors: Vec<String> = Vec::new();
                let mut reached = false;
                let mut assert_failure = false;
                let mut user_fault = false;
                // This step's last-run entry (final attempt): its `curl` becomes
                // the reproduce hint, stringified once and only on failure.
                let mut last_entry = None;
                // One classify-and-collect for both attribution branches.
                let mut record_error = |error: &runner::RunnerError| {
                    match classify_error(error) {
                        HurlErrorClass::Assert => assert_failure = true,
                        HurlErrorClass::UserInput => user_fault = true,
                        HurlErrorClass::Infra => {}
                    }
                    errors.push(render_error(error));
                };
                if merged {
                    // §2.7: this step's asserts live on `span` lines inside
                    // the previous request's entry. Attribute that host
                    // entry's *final* attempt: errors on our lines are ours,
                    // a clean final attempt passes us.
                    // The host is the entry whose request starts closest
                    // above our lines: merged asserts always extend the
                    // response of the entry they follow (never a request
                    // span, so `line_in_entry` cannot find them).
                    if let Some([span_start, span_end]) = span
                        && let Some(host_entry) = parsed
                            .entries
                            .iter()
                            .filter(|pe| pe.request.source_info.start.line <= span_start)
                            .max_by_key(|pe| pe.request.source_info.start.line)
                    {
                        let host_results: Vec<_> = result
                            .entries
                            .iter()
                            .filter(|er| line_in_entry(host_entry, er.source_info.start.line))
                            .collect();
                        reached = !host_results.is_empty();
                        per_entry_results
                            .insert(0, u32::try_from(host_results.len()).unwrap_or(u32::MAX));
                        if let Some(final_result) = host_results.last() {
                            last_entry = Some(*final_result);
                            for error in &final_result.errors {
                                let line = error.source_info.start.line;
                                if line < span_start || line > span_end {
                                    continue;
                                }
                                record_error(error);
                            }
                        }
                    }
                } else {
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
                        last_entry = Some(entry_result);
                        *per_entry_results.entry(entry_index).or_default() += 1;
                        duration += entry_result.transfer_duration;
                        captures.extend(entry_result.captures.iter().map(|c| c.name.clone()));
                        for error in &entry_result.errors {
                            if in_merged_span(error.source_info.start.line) {
                                continue;
                            }
                            record_error(error);
                        }
                    }
                }
                // Multiple results for the *same* entry are retries: attempts
                // is the retried entry's count — never the sum across a
                // multi-entry step (a 2-entry step is 1 attempt, not 2). Only
                // the final result's errors decide (earlier ones retried).
                let attempts = per_entry_results.values().copied().max().unwrap_or(0);
                let final_failed = if merged {
                    !errors.is_empty()
                } else {
                    last_result_failed(&result, parsed, entries, &in_merged_span)
                };
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
                let detail = if matches!(status, Status::Failed | Status::Warned) {
                    let location = span.map_or_else(String::new, |[a, _]| {
                        format!(" (artifact {}.hurl:{a})", self.artifact.slug)
                    });
                    Some(format!("{}{location}", cap_detail(&errors.join("; "))))
                } else if merged && !reached {
                    Some("not run (its request entry did not run)".to_owned())
                } else if entries.is_empty() && !merged {
                    Some(NO_ENTRIES_DETAIL.to_owned())
                } else {
                    None
                };
                // A refused saveAs promotion upgrades a green step to a
                // warning: the author must see the value never persisted.
                let (status, detail) = match refused_promotions.get(index) {
                    Some(names) => {
                        let refusal = format!(
                            "saveAs: global refused for {}: captured value equals a secret — secrets never persist to .proef-state.json (ADR-0005)",
                            names
                                .iter()
                                .map(|n| format!("`{n}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                        let status = if status == Status::Passed {
                            Status::Warned
                        } else {
                            status
                        };
                        let detail = Some(match detail {
                            Some(existing) if !existing.is_empty() => {
                                format!("{existing}; {refusal}")
                            }
                            _ => refusal,
                        });
                        (status, detail)
                    }
                    None => (status, detail),
                };
                if status == Status::Failed {
                    failed = true;
                    let message = detail.clone().unwrap_or_default();
                    // A user mistake outranks an assert result: with broken
                    // input the verdict is unusable until the author fixes
                    // the test (exit 2 over 1 over 3, ADR-0009).
                    batch_error = batch_error.or(Some(if user_fault {
                        EngineError::user_input(message)
                    } else if assert_failure {
                        EngineError::assert_failed(message)
                    } else {
                        EngineError::infra(message)
                    }));
                }
                // Reproduce hint only for failures — the redacted curl of the
                // failing request; passing steps carry none (exec prints it
                // under a failed step).
                let reproduce_hint = if matches!(status, Status::Failed | Status::Warned) {
                    last_entry.map(|e| self.redactions.apply(&e.curl_cmd.to_string()))
                } else {
                    None
                };
                // Flaky-failure detail: a step that ultimately passed but
                // retried carries the messages from its earlier, failed
                // attempts — the final attempt is clean, so `errors` holds
                // exactly those. Already redacted by `render_error`; JUnit
                // surfaces them as <flakyFailure>.
                let attempt_details = if status == Status::Passed && attempts > 1 {
                    errors.iter().map(|e| cap_detail(e)).collect()
                } else {
                    Vec::new()
                };
                outcomes.push(StepOutcome {
                    step: step.step.clone(),
                    status,
                    attempts: attempts.max(u32::from(reached)),
                    duration,
                    detail: detail.clone(),
                    attempt_details: attempt_details.clone(),
                    reproduce_hint,
                    fragment: step.fragment.clone(),
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
                    &attempt_details,
                    // Moved into the outcome just pushed — read back rather
                    // than cloned; the outcome and the event must agree.
                    outcomes.last().and_then(|o| o.reproduce_hint.as_deref()),
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
                // `?` on any of these: a `{{…}}` placeholder (or an infinite
                // count) means this batch cannot be estimated, and the
                // orchestrator's documented fallback is a better answer than a
                // confident under-count that abandons a healthy scenario.
                let (retries, interval) = entry_retry(entry)?;
                let timeout = entry_timeout(entry, default_timeout)?;
                let delay = entry_delay(entry)?;
                // Saturating throughout: user-authored durations must never
                // panic the budget math (they cap at Duration::MAX instead).
                let attempts = retries.saturating_add(1);
                let per_run = timeout
                    .saturating_mul(attempts)
                    .saturating_add(interval.saturating_mul(retries))
                    .saturating_add(delay.saturating_mul(attempts));
                // `repeat:` runs the whole entry (with its retries) N times.
                budget = budget.saturating_add(per_run.saturating_mul(entry_repeat(entry)?));
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

/// Bound a failure detail before it enters any sink (R18 wave-1). hurl's
/// rendered assert error quotes the actual response, so a failed assert on a
/// large body would otherwise ride full-size into the record, `JUnit`, the
/// HTML report and the GitHub summary at once — every sink pays, and none
/// can un-pay. Middle-cut like Robot Framework's 40-line rule: the head
/// carries the assertion, the tail carries the closing context, the marker
/// says what was elided and where the full truth lives (re-running the
/// artifact). Cuts at char boundaries; short details pass through untouched.
fn cap_detail(text: &str) -> String {
    const MAX_LINES: usize = 40;
    const MAX_BYTES: usize = 8 * 1024;
    let over_lines = text.lines().count() > MAX_LINES;
    if !over_lines && text.len() <= MAX_BYTES {
        return text.to_owned();
    }
    let keep = MAX_BYTES / 2;
    let head_end = text
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= keep)
        .last()
        .unwrap_or(0);
    let tail_start = text
        .char_indices()
        .map(|(i, _)| i)
        .find(|&i| i >= text.len().saturating_sub(keep / 4))
        .unwrap_or(text.len());
    let head: String = text[..head_end]
        .lines()
        .take(MAX_LINES.saturating_sub(8))
        .collect::<Vec<_>>()
        .join("\n");
    let tail_lines: Vec<&str> = text[tail_start..].lines().collect();
    let tail = tail_lines[tail_lines.len().saturating_sub(6)..].join("\n");
    let elided = text.len().saturating_sub(head.len() + tail.len());
    format!("{head}\n[… {elided} bytes elided — re-run the artifact for the full output]\n{tail}")
}

/// Detail attached to a step whose payload lowered to zero hurl entries.
const NO_ENTRIES_DETAIL: &str = "no hurl entries to execute (comment-only payload?)";

fn skipped_outcome(step: &proef_core::step::LoweredStep) -> StepOutcome {
    StepOutcome {
        step: step.step.clone(),
        status: Status::Skipped,
        attempts: 0,
        duration: Duration::ZERO,
        detail: None,
        attempt_details: Vec::new(),
        reproduce_hint: None,
        fragment: step.fragment.clone(),
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
    attempt_details: &[String],
    reproduce_hint: Option<&str>,
) {
    events.emit(&Event::StepFinished {
        scenario: Arc::clone(scenario),
        engine: Arc::from(batch.engine.as_str()),
        step: step.step.clone(),
        status,
        attempts,
        duration_ms,
        captures: captures.to_vec(),
        fragment: step.fragment.clone(),
        detail: detail.map(ToOwned::to_owned),
        attempt_details: attempt_details.to_vec(),
        reproduce_hint: reproduce_hint.map(ToOwned::to_owned),
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

/// Did the *final* attempt of any of these entries fail? Errors on lines for
/// which `excluded` holds belong to a merged-asserts step (§2.7) and never
/// fail the host request.
fn last_result_failed(
    result: &runner::HurlResult,
    parsed: &HurlFile,
    entries: &[usize],
    excluded: &impl Fn(usize) -> bool,
) -> bool {
    for &entry_index in entries {
        let Some(parsed_entry) = parsed.entries.get(entry_index) else {
            continue;
        };
        let last = result
            .entries
            .iter()
            .rfind(|er| line_in_entry(parsed_entry, er.source_info.start.line));
        if last.is_some_and(|er| {
            er.errors
                .iter()
                .any(|e| !excluded(e.source_info.start.line))
        }) {
            return true;
        }
    }
    false
}

/// Where a hurl runner error folds in the seam taxonomy (ADR-0009).
#[derive(Clone, Copy, PartialEq, Eq)]
enum HurlErrorClass {
    /// The test's expectation failed against the live response → exit 1.
    Assert,
    /// A mistake in the test's own text the author must fix → exit 2.
    UserInput,
    /// Connection, native library, or other environment trouble → exit 3.
    Infra,
}

fn classify_error(error: &runner::RunnerError) -> HurlErrorClass {
    use runner::RunnerErrorKind as K;
    match &error.kind {
        // Static authoring mistakes — wrong independent of any response:
        // the author fixes the test, not the environment (exit 2).
        K::TemplateVariableNotDefined { .. }
        | K::QueryInvalidJsonpathExpression { .. }
        | K::InvalidRegex
        | K::InvalidUrl { .. }
        | K::InvalidOptionValue { .. }
        | K::FileReadAccess { .. }
        | K::UnauthorizedFileAccess { .. }
        | K::FilterInvalidEncoding(_)
        | K::FilterInvalidFormatSpecifier(_) => HurlErrorClass::UserInput,
        // Query failures over the *response* are test failures: a backend
        // answering malformed JSON/XML (or missing the queried header)
        // failed the test's expectation — proef and the environment are
        // fine (ADR-0009: exit 1, never 3). Everything else defers to
        // hurl's own assert-context flag.
        K::AssertBodyDiffError { .. }
        | K::AssertBodyValueError { .. }
        | K::AssertFailure { .. }
        | K::AssertHeaderValueError { .. }
        | K::AssertStatus { .. }
        | K::AssertVersion { .. }
        | K::QueryInvalidJson
        | K::QueryInvalidXml
        | K::QueryHeaderNotFound => HurlErrorClass::Assert,
        _ if error.assert => HurlErrorClass::Assert,
        _ => HurlErrorClass::Infra,
    }
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

/// Captures fold into the scalar [`WorldValue`] **lossily**: `JSONPath` lists
/// and objects, `Bytes`, and `Date` all become their hurl `Display` string
/// (and `BigInteger` deliberately stays a string — the serde
/// `arbitrary_precision` constraint bans numeric round-trips through
/// untagged enums). Pack authors capture scalars; anything structured is a
/// string on arrival in the World.
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

/// Every option of an entry's `[Options]` sections, flattened — the one
/// traversal behind the per-entry accessors below.
fn entry_options(entry: &hurl_core::ast::Entry) -> impl Iterator<Item = &OptionKind> {
    entry
        .request
        .sections
        .iter()
        .filter_map(|section| match &section.value {
            SectionValue::Options(options) => Some(options.iter().map(|option| &option.kind)),
            _ => None,
        })
        .flatten()
}

/// Per-entry `[Options]` retry (finite by pack lint) and interval.
/// `(retries, interval)` for an entry, or `None` when a `{{…}}` placeholder
/// makes it unknowable.
///
/// A placeholder resolves inside hurl at run time (ADR-0005's second tier), so
/// no static estimate can see it. Reporting "no retries" for one — which is
/// what falling through to the default did — under-counts the budget by the
/// whole retry loop, and the watchdog then abandons a scenario that was
/// retrying exactly as authored, attributing it to the environment (exit 3).
/// `None` propagates to [`EngineSession::batch_budget`], whose contract already
/// covers this: the orchestrator falls back to `default_batch_budget` rather
/// than to a confidently wrong number.
fn entry_retry(entry: &hurl_core::ast::Entry) -> Option<(u32, Duration)> {
    let mut retries = 0u32;
    let mut interval = Duration::from_secs(1);
    for kind in entry_options(entry) {
        match kind {
            OptionKind::Retry(CountOption::Literal(Count::Finite(count))) => {
                retries = u32::try_from(*count).unwrap_or(u32::MAX);
            }
            OptionKind::RetryInterval(DurationOption::Literal(duration)) => {
                interval = ast_duration(duration);
            }
            // Unestimatable: a placeholder resolves inside hurl at run time,
            // and an infinite count is unbounded by definition (the pack lint
            // rejects it; this is the runtime backstop).
            OptionKind::Retry(
                CountOption::Literal(Count::Infinite) | CountOption::Placeholder(_),
            )
            | OptionKind::RetryInterval(DurationOption::Placeholder(_)) => return None,
            _ => {}
        }
    }
    Some((retries, interval))
}

/// Per-entry `[Options] repeat:` — hurl runs the entry that many times; the
/// budget must scale with it or legitimate long repeats get abandoned and
/// blamed on the environment. Default 1; `repeat: -1` saturates (the load
/// lint rejects it, this is the runtime backstop).
fn entry_repeat(entry: &hurl_core::ast::Entry) -> Option<u32> {
    let mut repeat = 1u32;
    for kind in entry_options(entry) {
        match kind {
            OptionKind::Repeat(CountOption::Literal(Count::Finite(count))) => {
                repeat = u32::try_from(*count).unwrap_or(u32::MAX);
            }
            OptionKind::Repeat(
                CountOption::Literal(Count::Infinite) | CountOption::Placeholder(_),
            ) => return None,
            _ => {}
        }
    }
    Some(repeat)
}

/// Per-entry `[Options] delay:` — an uninterruptible sleep hurl applies per
/// attempt; it must be part of the budget or the watchdog kills the scenario.
/// The entry's `delay:`, or `None` when it is a placeholder — see
/// [`entry_retry`] for why an unknown value must not read as zero.
fn entry_delay(entry: &hurl_core::ast::Entry) -> Option<Duration> {
    let mut delay = Duration::ZERO;
    for kind in entry_options(entry) {
        match kind {
            OptionKind::Delay(DurationOption::Literal(duration)) => delay = ast_duration(duration),
            OptionKind::Delay(DurationOption::Placeholder(_)) => return None,
            _ => {}
        }
    }
    Some(delay)
}

/// The entry's effective timeout, or `None` when a placeholder makes it
/// unknowable. `default` is used when the option is simply absent — so `None`
/// carries one meaning here, "cannot estimate", and not two.
fn entry_timeout(entry: &hurl_core::ast::Entry, default: Duration) -> Option<Duration> {
    let mut timeout = default;
    for kind in entry_options(entry) {
        match kind {
            OptionKind::MaxTime(DurationOption::Literal(duration)) => {
                timeout = ast_duration(duration);
            }
            OptionKind::MaxTime(DurationOption::Placeholder(_)) => return None,
            _ => {}
        }
    }
    Some(timeout)
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

#[cfg(test)]
mod budget_tests {
    #![allow(clippy::unwrap_used)]

    use super::{entry_delay, entry_repeat, entry_retry};

    fn first_entry(text: &str) -> hurl_core::ast::Entry {
        hurl_core::parser::parse_hurl_file(text)
            .unwrap()
            .entries
            .into_iter()
            .next()
            .unwrap()
    }

    /// A `{{…}}` timing option is unknowable before hurl runs, so the estimate
    /// must say so. It used to fall through to the default and report "no
    /// retries", which under-counts the budget by the whole retry loop — the
    /// watchdog then abandons a scenario that was retrying exactly as authored
    /// and reports it as an environment fault.
    #[test]
    fn a_templated_timing_option_is_unestimatable_not_zero() {
        let literal = first_entry("GET http://x\n[Options]\nretry: 3\nHTTP 200\n");
        assert_eq!(
            entry_retry(&literal).map(|(retries, _)| retries),
            Some(3),
            "a literal count is still counted exactly"
        );

        let templated = first_entry("GET http://x\n[Options]\nretry: {{n}}\nHTTP 200\n");
        assert_eq!(
            entry_retry(&templated),
            None,
            "a templated retry must not read as zero retries"
        );

        let templated_delay = first_entry("GET http://x\n[Options]\ndelay: {{d}}\nHTTP 200\n");
        assert_eq!(entry_delay(&templated_delay), None);

        let templated_repeat = first_entry("GET http://x\n[Options]\nrepeat: {{r}}\nHTTP 200\n");
        assert_eq!(entry_repeat(&templated_repeat), None);
    }

    /// An entry with no timing options at all is fully estimatable — the
    /// change must not make every batch fall back to the default.
    #[test]
    fn a_plain_entry_is_still_estimatable() {
        let plain = first_entry("GET http://x\nHTTP 200\n");
        assert_eq!(entry_retry(&plain).map(|(r, _)| r), Some(0));
        assert_eq!(entry_delay(&plain), Some(std::time::Duration::ZERO));
        assert_eq!(entry_repeat(&plain), Some(1));
    }

    /// Short details pass through byte-identical — the cap must never touch
    /// the common case.
    #[test]
    fn a_short_detail_is_untouched() {
        let s = "assert status == 200 failed (got 500)";
        assert_eq!(super::cap_detail(s), s);
    }

    /// An oversized single-line detail (a quoted response body) is
    /// middle-cut with the elision marker; head and tail both survive.
    #[test]
    fn an_oversized_detail_is_middle_cut() {
        let body: String = "x".repeat(20 * 1024);
        let s = format!("assert body == … failed; actual: {body}END");
        let capped = super::cap_detail(&s);
        assert!(capped.len() < 10 * 1024, "bounded: {}", capped.len());
        assert!(capped.starts_with("assert body"), "head survives");
        assert!(capped.ends_with("END"), "tail survives");
        assert!(capped.contains("bytes elided"), "the marker names the cut");
    }

    /// A many-line detail is cut by lines even when small in bytes, and the
    /// head and tail never overlap (head ≤ 32 lines, tail = last 6, trigger
    /// ≥ 41 lines).
    #[test]
    fn a_many_line_detail_is_cut_by_lines() {
        let s: String = (1..=60)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let capped = super::cap_detail(&s);
        assert!(capped.contains("line 1\n"), "head survives");
        assert!(capped.ends_with("line 60"), "tail survives");
        assert!(capped.contains("elided"), "marker present");
        assert!(!capped.contains("line 40\nline 41"), "the middle is gone");
    }

    /// Cuts land on char boundaries — multibyte content at the cut point
    /// must not panic.
    #[test]
    fn the_cut_respects_char_boundaries() {
        let s = "é".repeat(10 * 1024);
        let capped = super::cap_detail(&s);
        assert!(capped.contains("elided"));
    }
}
