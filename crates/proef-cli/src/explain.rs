//! `proef explain [run-id]` — summarize a run from its record. The JSONL event
//! stream **is** the record (ADR-0008): explain reads through
//! `crate::record::parse_record`, the same fold `diff`'s `read_record` uses,
//! so a truncated record is never mistaken for a complete one.

use std::collections::BTreeMap;
use std::path::PathBuf;

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
    let events_path = record_dir.join("events.jsonl");
    let text = match std::fs::read_to_string(&events_path) {
        Ok(text) => text,
        Err(err) => {
            crate::render::errln!("error: cannot read {}: {err}", events_path.display());
            return ExitCode::UserError;
        }
    };
    let events: Vec<Event> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let rec = record::parse_record(&events);

    // A complete/cancelled record's tail `RunFinished` carries the run's own
    // verdict — main-suite scenarios only, `[run] setup`/`teardown` excluded
    // (ADR-0014) — so reading it here is what keeps this headline agreeing
    // with the console `summary:` line, JUnit, `--output json`, TAP, the SLA
    // gate, and the exit code. A *truncated* record has no `RunFinished` to
    // read (`rec.totals` is `None`), so fall back to counting the scenarios
    // the record actually holds — the only totals a dead run can offer.
    let (passed, failed, skipped) = if let Some(totals) = rec.totals {
        (totals.passed, totals.failed, totals.skipped)
    } else {
        let count = |want: Status| {
            rec.scenarios
                .values()
                .filter(|run| run.status == want)
                .count()
        };
        (
            count(Status::Passed),
            count(Status::Failed),
            count(Status::Skipped),
        )
    };
    // Step/attempt totals are unrelated to the suite-only scenario counts
    // above — they count every step in the stream, `[run] setup`/`teardown`
    // included, straight from the raw events rather than `rec.scenarios`: a
    // step only attaches there once its `ScenarioFinished` arrives
    // (`record::parse_record`), so a scenario still in flight when a
    // truncated stream ends would otherwise vanish from the headline —
    // exactly the case a post-mortem tool exists to report on.
    let (mut steps, mut attempts) = (0usize, 0u64);
    for event in &events {
        if let Event::StepFinished {
            attempts: step_attempts,
            ..
        } = event
        {
            steps += 1;
            attempts += u64::from(*step_attempts);
        }
    }

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

    // Gate on whether the record holds ANY failed scenario, not on the
    // suite-only `failed` above: `[run] setup`/`teardown` scenarios can fail
    // while the suite itself is untouched, so `failed == 0` does not mean
    // nothing broke — it means nothing in the *suite* broke (ADR-0014). A
    // post-mortem tool that goes silent on a phase failure is the exact bug
    // this branch exists to eliminate.
    if rec
        .scenarios
        .values()
        .any(|run| run.status == Status::Failed)
    {
        // Per-step failure detail (file:line, message) isn't carried by the
        // record's `StepRun` (attempts/duration only), so it comes from the
        // same raw events already in hand.
        let failures = failure_detail(&events);
        // Whenever the headline itself reads `0 failed`, every failure listed
        // below is necessarily one the headline excludes on purpose — a
        // setup/teardown fault, never a suite failure the headline forgot
        // (a counted suite failure would have made `failed` nonzero). Label
        // it so the two lines read as consistent, not contradictory.
        let label = if failed == 0 {
            "failed (setup/teardown — excluded from the totals above)"
        } else {
            "failed"
        };
        for (key, run) in &rec.scenarios {
            if run.status == Status::Failed {
                crate::render::outln!("\n{label}: {}", key.1);
                for line in failures.get(key).into_iter().flatten() {
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

/// `(file, scenario) -> per-step failure lines` (`file:line`, message,
/// attempts) — the detail the record's `StepRun` doesn't carry. Keyed by
/// `(file, scenario)`, matching `Record::scenarios`' run-wide identity
/// (ADR-0008): two same-named scenarios in different files must not share
/// each other's failure lines.
fn failure_detail(events: &[Event]) -> BTreeMap<(String, String), Vec<String>> {
    let mut failures: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for event in events {
        let Event::StepFinished {
            scenario,
            step,
            status: Status::Failed,
            attempts,
            detail,
            ..
        } = event
        else {
            continue;
        };
        let why = detail
            .as_deref()
            .map(|d| format!("\n      {d}"))
            .unwrap_or_default();
        failures
            .entry((step.file.to_string(), scenario.to_string()))
            .or_default()
            .push(format!(
                "  ✗ {}:{} — {} ({attempts} attempt(s)){why}",
                step.file, step.line, step.text
            ));
    }
    failures
}
