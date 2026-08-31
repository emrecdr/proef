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
///
/// `machine` swaps the prose for one JSON object carrying the same facts.
/// It exists because the alternative was worse: a run directory is
/// `artifacts/ + events.jsonl + run.log` and holds no structured summary, so
/// anything analysing a run it did not launch had to fold `events.jsonl`
/// itself — and that fold is exactly the one proef's own two copies disagreed
/// on three ways before `report::suite_totals` unified them. Handing the
/// canonical answer over is cheaper than inviting everyone to re-derive it.
pub fn explain(runs_root: &Path, run_id: Option<&str>, machine: bool) -> ExitCode {
    let Some(record_dir) = record::resolve_dir(runs_root, run_id) else {
        crate::render::errln!("error: no run records under {}", runs_root.display());
        return ExitCode::UserError;
    };
    // Through `record::read_events`, not a bare `read_to_string`: that is
    // where the record-size ceiling lives, and a reader that opens the file
    // itself simply does not have it.
    let events: Vec<Event> = match record::read_events(&record_dir) {
        Ok(events) => events,
        Err(err) => {
            crate::render::errln!("error: {err}");
            return ExitCode::UserError;
        }
    };
    let rec = record::parse_record(&events);

    // A complete/cancelled record's tail `RunFinished` carries the run's own
    // verdict — main-suite scenarios only, `[run] setup`/`teardown` excluded
    // (ADR-0014) — so reading it here is what keeps this headline agreeing
    // with the console `summary:` line, JUnit, `--format json`, TAP, the SLA
    // gate, and the exit code. A *truncated* record has no `RunFinished` to
    // read (`rec.totals` is `None`), so fall back to counting the scenarios
    // the record actually holds — the only totals a dead run can offer.
    // A pre-0.6.0 record's `run_finished` totals counted every phase, not the
    // suite — so reading them under today's meaning reports the wrong verdict
    // with full confidence. Fall back to counting the scenarios present, the
    // same path a truncated record already takes.
    let (passed, failed, skipped) = proef_core::report::suite_totals(
        rec.totals
            .map(|totals| (totals.passed, totals.failed, totals.skipped)),
        rec.legacy_multi_pair,
        rec.scenarios
            .values()
            .map(|run| (run.is_suite(), Some(run.status))),
    );
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

    let id = record_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    if machine {
        return machine_summary(
            &events,
            &rec,
            record_dir.as_path(),
            id,
            (passed, failed, skipped, steps, attempts),
        );
    }

    crate::render::outln!("run {id} — {}", record_dir.display());
    if let Some(env) = &rec.env {
        crate::render::outln!("env: {env}");
    }
    for (key, value) in &rec.metadata {
        crate::render::outln!("meta {key}: {value}");
    }
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

    print_skipped(&rec);

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

/// The same summary as one JSON object on stdout.
///
/// Mirrors the prose field for field rather than modelling a richer view: the
/// prose is the contract a reader already knows, and a machine surface that
/// says something *different* is a second answer to one question. Emitted with
/// `outln!`, so a closed pipe reaches the exit code like every other sink.
fn machine_summary(
    events: &[Event],
    rec: &record::Record,
    record_dir: &Path,
    id: &str,
    totals: (usize, usize, usize, usize, u64),
) -> ExitCode {
    let (passed, failed, skipped, steps, attempts) = totals;
    let failures = failure_detail(events);
    let scenario_rows = |want: Status| -> Vec<serde_json::Value> {
        rec.scenarios
            .iter()
            .filter(|(_, run)| run.status == want)
            .map(|(key, run)| {
                let mut row = serde_json::json!({
                    "file": key.0,
                    "scenario": key.1,
                    "phase": run.phase,
                    "tags": run.tags,
                });
                if want == Status::Failed {
                    row["detail"] =
                        serde_json::json!(failures.get(key).cloned().unwrap_or_default());
                } else if let Some(reason) = &run.reason {
                    row["reason"] = serde_json::json!(reason);
                }
                row
            })
            .collect()
    };
    let body = serde_json::json!({
        "run_id": id,
        "dir": record_dir.display().to_string(),
        "env": rec.env,
        "metadata": rec.metadata,
        // The two conditions the prose warns about, as booleans a gate can
        // read: a truncated record whose totals are counted rather than read,
        // and a pre-0.6.0 one whose recorded totals meant something else.
        "complete": rec.completion != RunCompletion::Incomplete,
        "legacy_per_phase_totals": rec.legacy_multi_pair,
        "passed": passed,
        "failed": failed,
        "skipped": skipped,
        "steps": steps,
        "attempts": attempts,
        "failures": scenario_rows(Status::Failed),
        "skips": scenario_rows(Status::Skipped),
        "artifacts": record_dir.join("artifacts").display().to_string(),
        "log": record_dir.join("run.log").display().to_string(),
    });
    match serde_json::to_string_pretty(&body) {
        Ok(text) => {
            crate::render::outln!("{text}");
            ExitCode::Success
        }
        Err(err) => {
            crate::render::errln!("error: cannot render the summary as JSON: {err}");
            ExitCode::SystemError
        }
    }
}

/// `(file, scenario) -> per-step failure lines` (`file:line`, message,
/// Skipped scenarios with their reasons — an authored `@skip` is a
/// deliberate, versioned act and the post-mortem should say so; a mechanical
/// skip explains a cancelled run's shape. Reason-less rows (pre-field
/// records) stay silent rather than inventing prose.
fn print_skipped(rec: &record::Record) {
    let skipped_with_reason: Vec<_> = rec
        .scenarios
        .iter()
        .filter(|(_, run)| run.status == Status::Skipped && run.reason.is_some())
        .collect();
    if !skipped_with_reason.is_empty() {
        crate::render::outln!("");
        for (key, run) in &skipped_with_reason {
            crate::render::outln!(
                "skipped: {} — {}",
                key.1,
                run.reason.as_deref().unwrap_or_default()
            );
        }
    }
}

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
            reproduce_hint,
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
        // The record now carries what the console printed at run time — the
        // post-mortem tool must not know less than the live console did.
        let reproduce = reproduce_hint
            .as_deref()
            .map(|hint| format!("\n      reproduce: {hint}"))
            .unwrap_or_default();
        failures
            .entry((step.file.to_string(), scenario.to_string()))
            .or_default()
            .push(format!(
                "  ✗ {}:{} — {} ({attempts} attempt(s)){why}{via}{reproduce}",
                step.file, step.line, step.text
            ));
    }
    failures
}
