//! `proef explain [run-id]` — summarize a run from its record. The JSONL event
//! stream **is** the record (ADR-0008): explain replays it, no second format.

use std::path::{Path, PathBuf};

use proef_core::error::ExitCode;
use proef_core::event::Event;
use proef_core::report::RunTotals;
use proef_core::step::Status;

/// Explain the named run (or the latest) from `.proef-runs/`.
pub fn explain(runs_dir: &str, run_id: Option<&str>) -> ExitCode {
    let runs_root = PathBuf::from(runs_dir);
    let record_dir = if let Some(run_id) = run_id {
        runs_root.join(run_id)
    } else if let Some(dir) = latest_run(&runs_root) {
        dir
    } else {
        eprintln!("error: no run records under {}", runs_root.display());
        return ExitCode::UserError;
    };
    let events_path = record_dir.join("events.jsonl");
    let text = match std::fs::read_to_string(&events_path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("error: cannot read {}: {err}", events_path.display());
            return ExitCode::UserError;
        }
    };

    let mut totals = RunTotals::default();
    let mut run_id_seen = String::new();
    let mut failures: Vec<String> = Vec::new();
    let mut scenario_status: Vec<(String, Status)> = Vec::new();
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Event>(line) else {
            continue;
        };
        totals.observe(&event);
        match &event {
            Event::RunStarted { run_id, .. } => run_id_seen = run_id.to_string(),
            Event::StepFinished {
                step,
                status: Status::Failed,
                attempts,
                ..
            } => {
                failures.push(format!(
                    "  ✗ {}:{} — {} ({attempts} attempt(s))",
                    step.file, step.line, step.text
                ));
            }
            Event::ScenarioFinished { scenario, status } => {
                scenario_status.push((scenario.to_string(), *status));
            }
            _ => {}
        }
    }

    println!("run {run_id_seen} — {}", record_dir.display());
    println!(
        "{} passed · {} failed · {} skipped · {} step(s), {} attempt(s)",
        totals.passed, totals.failed, totals.skipped, totals.steps, totals.attempts
    );
    for (scenario, status) in &scenario_status {
        if *status == Status::Failed {
            println!("\nfailed: {scenario}");
        }
    }
    for failure in &failures {
        println!("{failure}");
    }
    if totals.failed > 0 {
        println!(
            "\nartifacts: {} · console record: {}",
            record_dir.join("artifacts").display(),
            record_dir.join("run.log").display()
        );
    }
    ExitCode::Success
}

/// The newest run dir (uuid-v7 names sort chronologically).
fn latest_run(runs_root: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs_root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs.pop()
}
