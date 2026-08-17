//! `proef flaky` — flakiness verdicts over the retained run history.
//!
//! The 2026 discipline is a pipeline — detect → quarantine → resolve — and
//! proef already owns the middle step: a `@quarantine` tag that runs a
//! scenario without letting its failure gate the exit code. This command is
//! the missing *detect*: a fold over the run records `runs-dir` already
//! retains, so the history window is `[run] keep-runs` — a knob that already
//! exists — and no new state is written anywhere.
//!
//! Three signals, each from fields the record already carries (ADR-0008):
//!
//! - **flapping** — the scenario's verdict changed between consecutive
//!   observed runs more than once. One transition is a regression or a fix;
//!   two or more is instability. Transition-counting rather than a fail-rate
//!   is what separates *flaky* from *broken* — a scenario failing every run
//!   is consistently broken, which is a different problem with a different
//!   owner.
//! - **passes only on retry** — the scenario is green but some step needed
//!   more than one attempt. This is the *latent* flake: one backoff change or
//!   one retry-budget cut from red, and structurally invisible to any tool
//!   that only sees pass/fail history. The record keeps per-step attempt
//!   counts, so proef sees it.
//! - **always failing** — every observed run failed. Reported so the listing
//!   is complete, and labelled broken rather than flaky on purpose.
//!
//! A cancellation-skipped scenario is **not evidence**: a run that never
//! reached it says nothing about its stability, so skipped rows do not count
//! toward that scenario's history (`observed`). `[run] setup`/`teardown`
//! phases are excluded the way `--rerun` and `diff` exclude them (ADR-0014).

use std::collections::BTreeMap;
use std::path::Path;

use proef_core::error::ExitCode;
use proef_core::step::Status;

use crate::record;

/// One scenario's fold over the observed history.
#[derive(Default)]
struct History {
    observed: u32,
    fails: u32,
    transitions: u32,
    pass_on_retry: u32,
    durations: Vec<u64>,
    last_failed: Option<bool>,
}

/// The verdict, ordered by how urgently a human should look at it — the
/// listing sorts by this, worst first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    /// Verdict flapped between runs: the quarantine candidate.
    Flaky,
    /// Green, but only ever after retries — one backoff change from red.
    Latent,
    /// Failed in every observed run: broken, not flaky.
    Broken,
    /// Seen in fewer than two runs — no history to judge yet.
    New,
    Healthy,
}

impl Verdict {
    fn word(self) -> &'static str {
        match self {
            Self::Flaky => "FLAKY — quarantine candidate (@quarantine)",
            Self::Latent => "passes only on retry (latent)",
            Self::Broken => "always failing (broken, not flaky)",
            Self::New => "new — not enough history",
            Self::Healthy => "healthy",
        }
    }

    /// The machine spelling: the stable first word, not the human hint.
    fn key(self) -> &'static str {
        match self {
            Self::Flaky => "flaky",
            Self::Latent => "latent",
            Self::Broken => "broken",
            Self::New => "new",
            Self::Healthy => "healthy",
        }
    }
}

/// Fold the retained records and render verdicts. Exit `0` — the command is
/// informational, like `explain`; a store with fewer than two records is a
/// user error (`2`), the same refusal `diff` gives, because a verdict over
/// one run would be noise wearing a table.
pub fn flaky(runs_root: &Path, output_json: bool) -> ExitCode {
    let runs = record::all_runs(runs_root);
    if runs.len() < 2 {
        crate::render::errln!(
            "error: need at least two runs for a flakiness verdict; found {} under {}",
            runs.len(),
            runs_root.display()
        );
        return ExitCode::UserError;
    }

    let mut histories: BTreeMap<(String, String), History> = BTreeMap::new();
    for dir in &runs {
        let rec = match record::read_record(dir) {
            Ok(rec) => rec,
            Err(err) => {
                crate::render::errln!("error: {err}");
                return ExitCode::UserError;
            }
        };
        for (key, run) in rec.scenarios {
            if run.phase.is_some() || run.status == Status::Skipped {
                // A phase is not a suite scenario (ADR-0014); a skipped row is
                // a run that never reached it — neither is stability evidence.
                continue;
            }
            let entry = histories.entry(key).or_default();
            let failed = run.status == Status::Failed;
            entry.observed += 1;
            if failed {
                entry.fails += 1;
            }
            if entry.last_failed.is_some_and(|last| last != failed) {
                entry.transitions += 1;
            }
            entry.last_failed = Some(failed);
            if !failed && run.steps.values().any(|s| s.attempts > 1) {
                entry.pass_on_retry += 1;
            }
            entry
                .durations
                .push(run.steps.values().map(|s| s.duration_ms).sum());
        }
    }

    let mut rows: Vec<((String, String), Verdict, History)> = histories
        .into_iter()
        .map(|(key, h)| {
            let verdict = if h.transitions >= 2 {
                Verdict::Flaky
            } else if h.pass_on_retry > 0 {
                Verdict::Latent
            } else if h.observed >= 2 && h.fails == h.observed {
                Verdict::Broken
            } else if h.observed < 2 {
                Verdict::New
            } else {
                Verdict::Healthy
            };
            (key, verdict, h)
        })
        .collect();
    rows.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

    if output_json {
        for ((file, scenario), verdict, h) in &rows {
            let object = serde_json::json!({
                "file": file,
                "scenario": scenario,
                "runs": h.observed,
                "fails": h.fails,
                "transitions": h.transitions,
                "pass_on_retry": h.pass_on_retry,
                "p95_ms": p95(&h.durations),
                "verdict": verdict.key(),
            });
            crate::render::outln!("{object}");
        }
        return ExitCode::Success;
    }
    render_table(&rows, runs.len(), runs_root);
    ExitCode::Success
}

/// The human listing: header, one row per scenario worst-first, and the
/// quarantine hand-off when anything was flagged.
fn render_table(rows: &[((String, String), Verdict, History)], runs: usize, runs_root: &Path) {
    crate::render::outln!(
        "flakiness over {runs} run(s) under {} (window = [run] keep-runs)\n",
        runs_root.display()
    );
    let width = rows
        .iter()
        .map(|((file, name), ..)| file.len() + name.len() + 4)
        .max()
        .unwrap_or(8)
        .max(8);
    crate::render::outln!(
        "{:width$}  {:>4}  {:>5}  {:>11}  {:>13}  {:>6}  verdict",
        "scenario",
        "runs",
        "fails",
        "transitions",
        "pass-on-retry",
        "p95 ms",
    );
    for ((file, name), verdict, h) in rows {
        crate::render::outln!(
            "{:width$}  {:>4}  {:>5}  {:>11}  {:>13}  {:>6}  {}",
            format!("{file} :: {name}"),
            h.observed,
            h.fails,
            h.transitions,
            h.pass_on_retry,
            p95(&h.durations),
            verdict.word(),
        );
    }
    let flagged = rows
        .iter()
        .filter(|(_, v, _)| matches!(v, Verdict::Flaky | Verdict::Latent))
        .count();
    if flagged > 0 {
        crate::render::outln!(
            "\n{flagged} scenario(s) flagged — tag a flapper `@quarantine` to keep it \
             running without gating the exit code while it is fixed"
        );
    }
}

/// Nearest-rank p95 over the observed durations (max at small history sizes —
/// exact by definition of nearest-rank, not an approximation).
fn p95(durations: &[u64]) -> u64 {
    if durations.is_empty() {
        return 0;
    }
    let mut sorted = durations.to_vec();
    sorted.sort_unstable();
    let rank = (sorted.len() * 95).div_ceil(100);
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}
