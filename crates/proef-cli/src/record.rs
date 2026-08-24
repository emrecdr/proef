//! Reading run records (`.proef-runs/<id>/events.jsonl`) — the JSONL event
//! stream IS the record (ADR-0008), so there is no second format to parse.
//! Shared by `explain`, `--rerun`, and `proef diff`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use proef_core::event::Event;
use proef_core::step::Status;

/// Every run dir under `runs_root`, oldest→newest. uuid-v7 names sort
/// chronologically, so lexical order *is* time order. Filters to real run dirs
/// so that under `runs-dir = "."` a stray `suite/`/`target/` sorting after the
/// uuid range cannot masquerade as a run.
pub fn all_runs(runs_root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs_root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(crate::fsutil::is_run_id)
        })
        .collect();
    dirs.sort();
    dirs
}

/// The newest run dir, or `None` when there are no run records.
pub fn latest_run(runs_root: &Path) -> Option<PathBuf> {
    all_runs(runs_root).pop()
}

/// Resolve a record dir: the named run under `runs_root`, else the latest.
pub fn resolve_dir(runs_root: &Path, run_id: Option<&str>) -> Option<PathBuf> {
    match run_id {
        Some(id) => Some(runs_root.join(id)),
        None => latest_run(runs_root),
    }
}

/// One positional, resolved to a record: a run id under `root`, or a **path**
/// — the record directory itself, or its events `.jsonl` under any name.
///
/// The path form is what a CI baseline flow actually has in hand: a record
/// downloaded from the base branch's artifacts lands wherever the download
/// step put it, not under this checkout's `runs-dir`. Before this, `diff`
/// joined every argument onto `runs-dir`, so a path answered with
/// `.proef-runs/.proef-runs/…/events.jsonl/events.jsonl: No such file` — the
/// argument mangled into the complaint, and nothing saying paths were the
/// problem.
///
/// Beside [`resolve_dir`] because the two answer halves of one question — how
/// a positional names a record — and `diff` was carrying this half alone.
/// Whether `explain`/`report` also accept paths is a product call; where the
/// primitive lives is not, and the next command to grow the flow calls this
/// rather than copying it.
///
/// Disambiguation is existence-on-disk, tried as a path first: a run id that
/// *also* exists as a local file would be pathological (`runs-dir` may be `.`,
/// but run dirs are uuid-named). A file is passed through as-is — a downloaded
/// baseline keeps whatever name the download step gave it, and [`read_record`]
/// takes the events file directly — and so is a directory, which is the
/// record-dir form the run ids resolve to.
pub fn locate(root: &Path, arg: &str) -> Result<PathBuf, String> {
    let as_path = Path::new(arg);
    if as_path.exists() {
        return Ok(as_path.to_path_buf());
    }
    // Not on disk: a run id under runs-root — unless it was *spelled* as a
    // path, where "id under root" is never what the caller meant and the
    // joined complaint would name a directory they did not type.
    if arg.contains(['/', '\\']) {
        return Err(format!(
            "`{arg}` does not exist (looked for a run directory or an events .jsonl file)"
        ));
    }
    Ok(root.join(arg))
}

/// A record's scenario identity, `(file, scenario name)` — the run-wide key
/// ADR-0008 added `file` for.
pub type Key = (String, String);

/// The one display spelling of a [`Key`]: `file :: scenario`. `diff` and
/// `flaky` both list record identities; two spellings of the same identity in
/// two listings is exactly the drift a shared formatter exists to prevent.
pub fn label(key: &Key) -> String {
    format!("{} :: {}", key.0, key.1)
}

/// One scenario's outcome in a run record: aggregate status plus its steps.
/// Steps are keyed `(text, ordinal)` for diffing — the authored `line` shifts
/// when a file is edited above it, so text is the stable identity, and the
/// 0-based occurrence ordinal disambiguates two steps that share text
/// (macro-expanded steps commonly do) so neither is lost (`proef diff`).
#[derive(Debug, Clone)]
pub struct ScenarioRun {
    /// Aggregate scenario status.
    pub status: Status,
    /// Steps keyed by `(authored text, 0-based occurrence ordinal within the scenario)`.
    pub steps: BTreeMap<(String, usize), StepRun>,
    /// `[run]` lifecycle phase (`"setup"`/`"teardown"`), `None` for a suite
    /// scenario. Read straight off the record instead of re-derived from
    /// `proef.toml`, which is what let `explain`, `--rerun` and `diff` each
    /// disagree about which scenarios were phases.
    pub phase: Option<String>,
}

impl ScenarioRun {
    /// Is this an ordinary suite scenario — not a `[run] setup`/`teardown`
    /// phase? The ADR-0014 projection every record consumer applies (`diff`,
    /// `--rerun`, `flaky`); spelled once here after being inlined three ways.
    pub fn is_suite(&self) -> bool {
        self.phase.is_none()
    }
}

/// One step's diffable metrics: the `attempts`/`duration_ms` that make a diff a
/// flakiness/perf-regression detector.
#[derive(Debug, Clone)]
pub struct StepRun {
    /// Number of attempts made (>1 means the engine retried — a flaky signal).
    pub attempts: u32,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
}

/// Whether a run record represents a complete run — read from the tail
/// `RunFinished` event (ADR-0008). A truncated/died run has no `RunFinished`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunCompletion {
    /// Ended with `RunFinished { cancelled: false }`.
    Completed,
    /// Ended with `RunFinished { cancelled: true }`.
    Cancelled,
    /// No `RunFinished` — the run was truncated or the process died.
    Incomplete,
}

/// The main-suite scenario totals carried by the tail `RunFinished` event
/// (ADR-0014: setup/teardown are excluded, so this is the run's own verdict —
/// the same numbers the console `summary:` line, `JUnit`, `--output json`, TAP,
/// the SLA gate, and the exit code all report).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunTotals {
    /// Scenarios that passed.
    pub passed: usize,
    /// Scenarios that failed.
    pub failed: usize,
    /// Scenarios that were skipped.
    pub skipped: usize,
}

/// A full run record: every scenario outcome plus whether the run completed.
#[derive(Debug, Clone)]
pub struct Record {
    /// `(file, scenario) -> outcome`. Populated from every `scenario_finished`
    /// in the stream, `[run] setup`/`teardown` scenarios included — the record
    /// keeps their events, even though `totals` excludes them.
    pub scenarios: BTreeMap<(String, String), ScenarioRun>,
    /// Whether the run reached its tail `RunFinished`.
    pub completion: RunCompletion,
    /// The tail `RunFinished`'s own totals — `None` exactly when `completion
    /// == RunCompletion::Incomplete` (no tail event to read them from).
    pub totals: Option<RunTotals>,
    /// The record carries more than one `RunFinished`, so it predates 0.6.0.
    ///
    /// Before 0.6.0 each phase emitted its own head/tail pair and the totals
    /// counted every phase; since 0.6.0 there is exactly one pair and the
    /// totals are the suite verdict (ADR-0014). The `schema` field cannot tell
    /// them apart — that change was semantic and never bumped it — but the
    /// structure can, unambiguously. Read it and say so rather than reporting
    /// the old numbers under the new meaning: a reader must be able to consume
    /// a record *or* detect that it cannot, never quietly do neither.
    pub legacy_multi_pair: bool,
}

/// Fold already-parsed events into a full run record: the `(file, scenario)
/// -> ScenarioRun` map plus completion. The `(file, scenario)` key is the
/// run-wide identity ADR-0008 added `file` for; records that predate the
/// field key under `file = ""`.
///
/// Takes `&[Event]` rather than a path so a caller that also needs the raw
/// events for something else (`report`'s `render_html`) can read and parse
/// the file exactly once and derive both from the same in-memory events —
/// two reads of a live run's `events.jsonl` can otherwise race and disagree
/// on whether the tail `RunFinished` had landed yet.
pub fn parse_record(events: &[Event]) -> Record {
    // Steps stream in before their `ScenarioFinished`; buffer them by
    // `(step.file, scenario)` and attach to each scenario as it closes.
    let mut pending: BTreeMap<(String, String), BTreeMap<(String, usize), StepRun>> =
        BTreeMap::new();
    // Occurrence ordinal per (file, scenario, step text), so identical-text
    // steps get distinct keys instead of overwriting each other.
    let mut seen: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    let mut record: BTreeMap<(String, String), ScenarioRun> = BTreeMap::new();
    let mut completion = RunCompletion::Incomplete;
    let mut totals: Option<RunTotals> = None;
    let mut legacy_multi_pair = false;
    for event in events {
        match event {
            Event::StepFinished {
                scenario,
                step,
                attempts,
                duration_ms,
                ..
            } => {
                let text_key = step.text.to_string();
                let ord = {
                    let counter = seen
                        .entry((
                            step.file.to_string(),
                            scenario.to_string(),
                            text_key.clone(),
                        ))
                        .or_insert(0);
                    let n = *counter;
                    *counter += 1;
                    n
                };
                pending
                    .entry((step.file.to_string(), scenario.to_string()))
                    .or_default()
                    .insert(
                        (text_key, ord),
                        StepRun {
                            attempts: *attempts,
                            duration_ms: *duration_ms,
                        },
                    );
            }
            Event::ScenarioFinished {
                scenario,
                file,
                status,
                phase,
                ..
            } => {
                let key = (file.to_string(), scenario.to_string());
                let steps = pending.remove(&key).unwrap_or_default();
                record.insert(
                    key,
                    ScenarioRun {
                        status: *status,
                        phase: phase.as_ref().map(ToString::to_string),
                        steps,
                    },
                );
            }
            Event::RunFinished {
                passed,
                failed,
                skipped,
                cancelled,
            } => {
                legacy_multi_pair = totals.is_some();
                completion = if *cancelled {
                    RunCompletion::Cancelled
                } else {
                    RunCompletion::Completed
                };
                totals = Some(RunTotals {
                    passed: *passed,
                    failed: *failed,
                    skipped: *skipped,
                });
            }
            _ => {}
        }
    }
    Record {
        scenarios: record,
        completion,
        legacy_multi_pair,
        totals,
    }
}

/// Read a full run record — a thin file-IO wrapper over [`parse_record`].
/// Callers that also need the raw events (`report`) should read and parse the
/// file themselves and call `parse_record` directly instead, rather than
/// paying for — and risking a second, possibly-inconsistent view from — a
/// second read of a live run.
///
/// Takes a record **directory** (`<record>/events.jsonl` is read) or the
/// events **file** itself, under any name. The JSONL stream *is* the record
/// (ADR-0008), so a `baseline.jsonl` a CI job downloaded from another branch's
/// artifacts is as much a record as a directory this checkout wrote — `diff`
/// accepts both spellings, and the two must mean the same thing here rather
/// than each caller re-deciding.
pub fn read_record(record_dir: &Path) -> Result<Record, String> {
    let events_path = if record_dir.is_file() {
        record_dir.to_path_buf()
    } else {
        record_dir.join("events.jsonl")
    };
    let text = std::fs::read_to_string(&events_path)
        .map_err(|err| format!("cannot read {}: {err}", events_path.display()))?;
    let events: Vec<Event> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    Ok(parse_record(&events))
}

/// What `--rerun` should run from a prior record, and how much of it is
/// unfinished rather than failed.
pub struct RerunCandidates {
    /// `(file, scenario)` identities to run.
    pub scenarios: Vec<(String, String)>,
    /// How many of those are scenarios the base run **never completed** — its
    /// cancellation-skipped tail. Zero on a completed record. Nonzero is worth
    /// a console note: the developer is continuing a partial run, not merely
    /// retrying failures.
    pub never_ran: usize,
}

/// The scenarios `--rerun` should run: every failure — plus, on a **cancelled**
/// record, every scenario the run never completed. A projection of
/// [`read_record`] so there is one record reader, not two.
///
/// The union is what makes fail-fast honest end to end. `--max-fail` (and
/// Ctrl-C) stop a run early; the never-reached scenarios record as
/// scenario-level `Skipped` so they are not silently absent — but a rerun
/// that then filtered to `Failed` alone ran only the old failures and
/// reported green with most of the suite still never executed. Reproduced
/// live: stop at 2 of 6, fix, rerun → `2 passed · 0 failed`, exit 0, four
/// scenarios untested in either run. Stop → fix → continue is the workflow
/// fail-fast exists for, so "continue" must mean the unfinished work too.
///
/// Scoped by construction: scenario-level `Skipped` is emitted only under
/// cancellation (the queue drain, and an in-flight scenario interrupted with
/// no failed step — `runner.rs`), so on a completed record the union *is* the
/// failure set and behavior is unchanged. Phases stay invisible to `--rerun`
/// (ADR-0014): they are not in the pool `build_specs` filters, so returning
/// one produced a run that matched nothing and blamed `--tags`/`--scenario`
/// the user never passed. A phase re-runs by re-running the suite.
pub fn rerun_candidates(record_dir: &Path) -> Result<RerunCandidates, String> {
    let record = read_record(record_dir)?;
    let cancelled = record.completion == RunCompletion::Cancelled;
    let mut scenarios = Vec::new();
    let mut never_ran = 0;
    for (key, run) in record.scenarios {
        if !run.is_suite() {
            continue;
        }
        match run.status {
            Status::Failed => scenarios.push(key),
            Status::Skipped if cancelled => {
                never_ran += 1;
                scenarios.push(key);
            }
            _ => {}
        }
    }
    Ok(RerunCandidates {
        scenarios,
        never_ran,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use proef_core::event::Event;
    use proef_core::step::{Status, StepRef};
    use std::sync::Arc;

    /// Write `events` as one JSON object per line into `<dir>/events.jsonl`.
    fn write_events(dir: &Path, events: &[Event]) {
        let body: String = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.join("events.jsonl"), body).unwrap();
    }

    fn step(file: &str, text: &str) -> StepRef {
        StepRef {
            file: Arc::from(file),
            line: 1,
            text: Arc::from(text),
        }
    }

    fn step_finished(scenario: &str, s: StepRef, attempts: u32, duration_ms: u64) -> Event {
        Event::StepFinished {
            scenario: Arc::from(scenario),
            engine: Arc::from("hurl"),
            step: s,
            status: Status::Passed,
            attempts,
            duration_ms,
            captures: Vec::new(),
            fragment: None,
            detail: None,
            attempt_details: Vec::new(),
            reproduce_hint: None,
        }
    }

    fn scenario_finished(scenario: &str, file: &str, status: Status) -> Event {
        Event::ScenarioFinished {
            scenario: Arc::from(scenario),
            file: Arc::from(file),
            status,
            timestamp_ms: None,
            worker: None,
            phase: None,
        }
    }

    #[test]
    fn duplicate_text_steps_are_kept_distinct() {
        let tmp = tempfile::tempdir().unwrap();
        // One scenario, two steps with IDENTICAL text but different metrics.
        write_events(
            tmp.path(),
            &[
                step_finished("S", step("f.feature", "GET /x"), 1, 10),
                step_finished("S", step("f.feature", "GET /x"), 3, 40),
                scenario_finished("S", "f.feature", Status::Passed),
            ],
        );
        let record = read_record(tmp.path()).unwrap();
        let run = record
            .scenarios
            .get(&("f.feature".to_string(), "S".to_string()))
            .unwrap();
        // Both occurrences survive (pre-fix the second overwrote the first → len 1).
        assert_eq!(run.steps.len(), 2, "both same-text steps must be retained");
        assert_eq!(
            run.steps.get(&("GET /x".to_string(), 0)).unwrap().attempts,
            1
        );
        assert_eq!(
            run.steps.get(&("GET /x".to_string(), 1)).unwrap().attempts,
            3
        );
    }

    #[test]
    fn same_scenario_name_across_files_does_not_contaminate_ordinals() {
        let tmp = tempfile::tempdir().unwrap();
        // Two different feature files, each with a scenario named "S" that has
        // ONE step "GET /x" — scenario names are unique only within a file
        // (proef_core::event, Event::ScenarioFinished's `file` doc comment).
        write_events(
            tmp.path(),
            &[
                step_finished("S", step("a.feature", "GET /x"), 1, 10),
                scenario_finished("S", "a.feature", Status::Passed),
                step_finished("S", step("b.feature", "GET /x"), 1, 10),
                scenario_finished("S", "b.feature", Status::Passed),
            ],
        );
        let record = read_record(tmp.path()).unwrap();
        // Each file's scenario S has ONE "GET /x" → each must be ordinal 0.
        let a = record
            .scenarios
            .get(&("a.feature".to_string(), "S".to_string()))
            .unwrap();
        let b = record
            .scenarios
            .get(&("b.feature".to_string(), "S".to_string()))
            .unwrap();
        assert!(
            a.steps.contains_key(&("GET /x".to_string(), 0)),
            "a.feature step must be ord 0"
        );
        assert!(
            b.steps.contains_key(&("GET /x".to_string(), 0)),
            "b.feature single-occurrence step must be ord 0, not contaminated by a.feature"
        );
    }

    /// A pre-0.6.0 record carries one `run_finished` per phase. Its totals
    /// counted every phase, not the suite, so reading them under today's
    /// meaning reports the wrong verdict with full confidence. The structure
    /// says so unambiguously even though `schema` does not — that change was
    /// semantic and never bumped it.
    #[test]
    fn a_multi_pair_record_is_detected_as_predating_0_6_0() {
        let one_pair = parse_record(&[Event::RunFinished {
            passed: 1,
            failed: 0,
            skipped: 0,
            cancelled: false,
        }]);
        assert!(!one_pair.legacy_multi_pair);

        let legacy = parse_record(&[
            Event::RunFinished {
                passed: 1,
                failed: 0,
                skipped: 0,
                cancelled: false,
            },
            Event::RunFinished {
                passed: 0,
                failed: 1,
                skipped: 0,
                cancelled: false,
            },
        ]);
        assert!(
            legacy.legacy_multi_pair,
            "more than one run_finished means the record predates 0.6.0"
        );
    }
}
