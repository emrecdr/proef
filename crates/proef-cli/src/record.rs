//! Reading run records (`.proef-runs/<id>/events.jsonl`) — the JSONL event
//! stream IS the record (ADR-0008), so there is no second format to parse.
//! Shared by `explain` and `--rerun` (#8); `proef diff` (#12) will reuse it too.

use std::path::{Path, PathBuf};

use proef_core::event::Event;
use proef_core::step::Status;

/// The newest run dir (uuid-v7 names sort chronologically). Filters to real run
/// dirs so that under `runs-dir = "."` a stray `suite/`/`target/` sorting after
/// the uuid range cannot shadow the real latest run.
pub fn latest_run(runs_root: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs_root)
        .ok()?
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
    dirs.pop()
}

/// Resolve a record dir: the named run under `runs_root`, else the latest.
pub fn resolve_dir(runs_root: &Path, run_id: Option<&str>) -> Option<PathBuf> {
    match run_id {
        Some(id) => Some(runs_root.join(id)),
        None => latest_run(runs_root),
    }
}

/// The `(file, scenario)` identity of every scenario that failed in a record —
/// the run-wide identity ADR-0008 added `file` for (names are unique only within
/// one feature file). Used by `--rerun` to re-run just the prior failures.
pub fn failed_scenarios(record_dir: &Path) -> Result<Vec<(String, String)>, String> {
    let events_path = record_dir.join("events.jsonl");
    let text = std::fs::read_to_string(&events_path)
        .map_err(|err| format!("cannot read {}: {err}", events_path.display()))?;
    let mut failed = Vec::new();
    for line in text.lines() {
        if let Ok(Event::ScenarioFinished {
            scenario,
            file,
            status,
            ..
        }) = serde_json::from_str::<Event>(line)
            && status == Status::Failed
        {
            failed.push((file.to_string(), scenario.to_string()));
        }
    }
    Ok(failed)
}
