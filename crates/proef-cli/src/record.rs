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
                ..
            } => {
                let key = (file.to_string(), scenario.to_string());
                let steps = pending.remove(&key).unwrap_or_default();
                record.insert(
                    key,
                    ScenarioRun {
                        status: *status,
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
        totals,
    }
}

/// Read a full run record from `<record_dir>/events.jsonl` — a thin file-IO
/// wrapper over [`parse_record`]. Callers that also need the raw events
/// (`report`) should read and parse the file themselves and call
/// `parse_record` directly instead, rather than paying for — and risking a
/// second, possibly-inconsistent view from — a second read of a live run.
pub fn read_record(record_dir: &Path) -> Result<Record, String> {
    let events_path = record_dir.join("events.jsonl");
    let text = std::fs::read_to_string(&events_path)
        .map_err(|err| format!("cannot read {}: {err}", events_path.display()))?;
    let events: Vec<Event> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    Ok(parse_record(&events))
}

/// The `(file, scenario)` identity of every scenario that failed in a record —
/// used by `--rerun` to re-run just the prior failures. A projection of
/// [`read_record`] so there is one record reader, not two.
pub fn failed_scenarios(record_dir: &Path) -> Result<Vec<(String, String)>, String> {
    Ok(read_record(record_dir)?
        .scenarios
        .into_iter()
        .filter(|(_, run)| run.status == Status::Failed)
        .map(|(key, _)| key)
        .collect())
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
            detail: None,
            attempt_details: Vec::new(),
        }
    }

    fn scenario_finished(scenario: &str, file: &str, status: Status) -> Event {
        Event::ScenarioFinished {
            scenario: Arc::from(scenario),
            file: Arc::from(file),
            status,
            timestamp_ms: None,
            worker: None,
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
}
