//! `proef explain [run-id]` — summarize a run from its record. The JSONL event
//! stream **is** the record (ADR-0008): explain reads through
//! `crate::record::parse_record`, the same fold `diff`'s `read_record` uses,
//! so a truncated record is never mistaken for a complete one.

use std::collections::BTreeMap;
use std::path::Path;

use proef_core::error::ExitCode;
use proef_core::event::Event;
use proef_core::step::Status;

use crate::record::{self, RunCompletion};

/// Explain the named run (or the latest) from `.proef-runs/`.
pub fn explain(runs_root: &Path, run_id: Option<&str>) -> ExitCode {
    let Some(record_dir) = record::resolve_dir(runs_root, run_id) else {
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
    // A pre-0.6.0 record's `run_finished` totals counted every phase, not the
    // suite — so reading them under today's meaning reports the wrong verdict
    // with full confidence. Fall back to counting the scenarios present, the
    // same path a truncated record already takes.
    let (passed, failed, skipped) =
        if let Some(totals) = rec.totals.filter(|_| !rec.legacy_multi_pair) {
            (totals.passed, totals.failed, totals.skipped)
        } else {
            let count = |want: &[Status]| {
                rec.scenarios
                    .values()
                    .filter(|run| want.contains(&run.status))
                    .count()
            };
            // `Warned` counts with `Passed`, exactly as the live path does
            // (`RunSummary::passed` is "passed, warnings allowed"). Counting
            // only the three other variants dropped a warned scenario from
            // every column, so a fallback total silently disagreed with the
            // run it was reconstructing — and `optional:` steps exist
            // precisely so a scenario can warn and still pass.
            (
                count(&[Status::Passed, Status::Warned]),
                count(&[Status::Failed]),
                count(&[Status::Skipped]),
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
    if rec.legacy_multi_pair {
        crate::render::errln!(
            "warning: this record predates 0.6.0 — it carries one `run_finished` per phase, and\n                      those totals counted every phase rather than the suite alone. The counts below are\n                      recomputed from the scenarios present; phase labelling is not available for it."
        );
    }

    if rec
        .scenarios
        .values()
        .any(|run| run.status == Status::Failed)
    {
        // Per-step failure detail (file:line, message) isn't carried by the
        // record's `StepRun` (attempts/duration only), so it comes from the
        // same raw events already in hand.
        let failures = failure_detail(&events);
        // Labelled per block, from the record's own `phase` field — not per
        // report. Deriving it from `failed == 0` was right only while *every*
        // failure was a phase failure; the moment a suite failure joined one,
        // both labels vanished and the reader saw `1 failed` above two
        // indistinguishable blocks. The disambiguation disappeared exactly
        // where it was needed.
        for (key, run) in &rec.scenarios {
            if run.status == Status::Failed {
                let label = match run.phase.as_deref() {
                    Some(phase) => format!("failed ({phase} — excluded from the totals above)"),
                    None => "failed".to_owned(),
                };
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
            fragment,
            ..
        } = event
        else {
            continue;
        };
        let why = detail
            .as_deref()
            .map(|d| format!("\n      {d}"))
            .unwrap_or_default();
        // ADR-0018 accepted "a test spans three files" as a cost *on the
        // condition that `explain` and go-to-definition earn it back*. A
        // `ref:` step's request text lives in neither the feature nor the
        // pack, so a post-mortem that stops at the Gherkin line leaves the
        // reader grepping for the file that actually failed. Printed after
        // the reason, because the reason is what they came for. The
        // `file.hurl#name` spelling is the one `ref:` itself accepts, so this
        // line pastes straight back into a pack.
        let via = fragment
            .as_deref()
            .map(|name| format!("\n      via {name}"))
            .unwrap_or_default();
        failures
            .entry((step.file.to_string(), scenario.to_string()))
            .or_default()
            .push(format!(
                "  ✗ {}:{} — {} ({attempts} attempt(s)){why}{via}",
                step.file, step.line, step.text
            ));
    }
    failures
}
