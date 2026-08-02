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
/// Keyed for diffing by step `text` — the authored `line` shifts when a file is
/// edited above it, so text is the stable identity (`proef diff`).
#[derive(Debug, Clone)]
pub struct ScenarioRun {
    /// Aggregate scenario status.
    pub status: Status,
    /// Steps keyed by authored text; last write wins on a duplicate-text step.
    pub steps: BTreeMap<String, StepRun>,
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

/// Read a full run record into a `(file, scenario) -> ScenarioRun` map. The
/// `(file, scenario)` key is the run-wide identity ADR-0008 added `file` for;
/// records that predate the field key under `file = ""`.
pub fn read_record(record_dir: &Path) -> Result<BTreeMap<(String, String), ScenarioRun>, String> {
    let events_path = record_dir.join("events.jsonl");
    let text = std::fs::read_to_string(&events_path)
        .map_err(|err| format!("cannot read {}: {err}", events_path.display()))?;
    // Steps stream in before their `ScenarioFinished`; buffer them by
    // `(step.file, scenario)` and attach to each scenario as it closes.
    let mut pending: BTreeMap<(String, String), BTreeMap<String, StepRun>> = BTreeMap::new();
    let mut record: BTreeMap<(String, String), ScenarioRun> = BTreeMap::new();
    for line in text.lines() {
        match serde_json::from_str::<Event>(line) {
            Ok(Event::StepFinished {
                scenario,
                step,
                attempts,
                duration_ms,
                ..
            }) => {
                pending
                    .entry((step.file.to_string(), scenario.to_string()))
                    .or_default()
                    .insert(
                        step.text.to_string(),
                        StepRun {
                            attempts,
                            duration_ms,
                        },
                    );
            }
            Ok(Event::ScenarioFinished {
                scenario,
                file,
                status,
            }) => {
                let key = (file.to_string(), scenario.to_string());
                let steps = pending.remove(&key).unwrap_or_default();
                record.insert(key, ScenarioRun { status, steps });
            }
            _ => {}
        }
    }
    Ok(record)
}

/// The `(file, scenario)` identity of every scenario that failed in a record —
/// used by `--rerun` to re-run just the prior failures. A projection of
/// [`read_record`] so there is one record reader, not two.
pub fn failed_scenarios(record_dir: &Path) -> Result<Vec<(String, String)>, String> {
    Ok(read_record(record_dir)?
        .into_iter()
        .filter(|(_, run)| run.status == Status::Failed)
        .map(|(key, _)| key)
        .collect())
}
