//! `proef explain [run-id]` — summarize a run from its record. The JSONL event
//! stream **is** the record (ADR-0008): explain reads through
//! `crate::record::read_record`, the reader `diff` already uses, so a
//! truncated record is never mistaken for a complete one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use proef_core::error::ExitCode;
use proef_core::event::Event;
use proef_core::step::Status;

use crate::record::{self, RunCompletion};

/// Explain the named run (or the latest) from `.proef-runs/`.
pub fn explain(runs_dir: &str, run_id: Option<&str>) -> ExitCode {
    let runs_root = PathBuf::from(runs_dir);
    let Some(record_dir) = record::resolve_dir(&runs_root, run_id) else {
        crate::render::errln!("error: no run records under {}", runs_root.display());
        return ExitCode::UserError;
    };
    let rec = match record::read_record(&record_dir) {
        Ok(rec) => rec,
        Err(err) => {
            crate::render::errln!("error: {err}");
            return ExitCode::UserError;
        }
    };

    // Headline totals come from the scenarios the record actually holds, not
    // from `RunFinished` — a truncated record has no `RunFinished` but may
    // still hold scenarios that ran to completion.
    let count = |want: Status| {
        rec.scenarios
            .values()
            .filter(|run| run.status == want)
            .count()
    };
    let (passed, failed, skipped) = (
        count(Status::Passed),
        count(Status::Failed),
        count(Status::Skipped),
    );
    let all_steps = || rec.scenarios.values().flat_map(|run| run.steps.values());
    let steps = all_steps().count();
    let attempts: u64 = all_steps().map(|step| u64::from(step.attempts)).sum();

    crate::render::outln!(
        "run {} — {}",
        record_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default(),
        record_dir.display()
    );
    if rec.completion == RunCompletion::Incomplete {
        crate::render::outln!("⚠ run incomplete — no run_finished; results are partial");
    }
    crate::render::outln!(
        "{passed} passed · {failed} failed · {skipped} skipped · {steps} step(s), {attempts} attempt(s)"
    );

    if failed > 0 {
        // Per-step failure detail (file:line, message) isn't carried by the
        // record's `StepRun` (attempts/duration only), so it still comes from
        // the raw events — read only now that there is a failure to explain.
        let failures = failure_detail(&record_dir);
        for (key, run) in &rec.scenarios {
            if run.status == Status::Failed {
                let scenario = &key.1;
                crate::render::outln!("\nfailed: {scenario}");
                for line in failures.get(scenario).into_iter().flatten() {
                    crate::render::outln!("{line}");
                }
            }
        }
        crate::render::outln!(
            "\nartifacts: {} · console record: {}",
            record_dir.join("artifacts").display(),
            record_dir.join("run.log").display()
        );
    }
    ExitCode::Success
}

/// `scenario -> per-step failure lines` (`file:line`, message, attempts) —
/// the detail the record's `StepRun` doesn't carry, read from the raw events.
fn failure_detail(record_dir: &Path) -> BTreeMap<String, Vec<String>> {
    let text = std::fs::read_to_string(record_dir.join("events.jsonl")).unwrap_or_default();
    let mut failures: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in text.lines() {
        let Ok(Event::StepFinished {
            scenario,
            step,
            status: Status::Failed,
            attempts,
            detail,
            ..
        }) = serde_json::from_str::<Event>(line)
        else {
            continue;
        };
        let why = detail
            .as_deref()
            .map(|d| format!("\n      {d}"))
            .unwrap_or_default();
        failures
            .entry(scenario.to_string())
            .or_default()
            .push(format!(
                "  ✗ {}:{} — {} ({attempts} attempt(s)){why}",
                step.file, step.line, step.text
            ));
    }
    failures
}
