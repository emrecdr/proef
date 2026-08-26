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

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use proef_core::error::ExitCode;
use proef_core::step::Status;

use crate::record::{self, Key, label};

/// One observed run of one scenario — everything a verdict reads.
struct Observation {
    failed: bool,
    /// Some step needed more than one attempt.
    retried: bool,
    duration_ms: u64,
}

/// One scenario's observed history: the runs that actually reached it, oldest
/// first. Everything a verdict needs is derived at read time — counts held
/// beside the observations they summarize were four fields of redundant state
/// and a fold-body state machine, for sums a bounded slice answers directly.
#[derive(Default)]
struct History {
    runs: Vec<Observation>,
    /// The scenario carried `@quarantine` in at least one observed run.
    ///
    /// Per scenario rather than per run because that is the question being
    /// asked — "is this one hidden?" — and a tag added midway through the
    /// window still means every failure since has been invisible.
    quarantined: bool,
}

impl History {
    fn observed(&self) -> usize {
        self.runs.len()
    }

    fn fails(&self) -> usize {
        self.runs.iter().filter(|o| o.failed).count()
    }

    /// Consecutive observed runs whose verdict differs — the flap count.
    fn transitions(&self) -> usize {
        self.runs
            .windows(2)
            .filter(|w| w[0].failed != w[1].failed)
            .count()
    }

    fn pass_on_retry(&self) -> usize {
        self.runs.iter().filter(|o| !o.failed && o.retried).count()
    }

    /// Nearest-rank p95 of the observed durations (`sla::percentile`, the
    /// crate's one implementation of the statistic).
    fn p95_ms(&self) -> u64 {
        let mut sorted: Vec<u64> = self.runs.iter().map(|o| o.duration_ms).collect();
        sorted.sort_unstable();
        crate::sla::percentile(&sorted, 95).unwrap_or(0)
    }

    /// The classification, from the derived counts. Transition-counting rather
    /// than fail-rate is the load-bearing choice: F,F,P,P is a fix that stuck
    /// (one transition), not a flake — fail-rate cannot tell those apart.
    fn verdict(&self) -> Verdict {
        // Quarantine is asked first because it changes what a result *means*,
        // not just how urgent it is. A quarantined scenario's failures gate
        // nothing, so nobody is looking at them: always-failing under
        // quarantine is a test that has been switched off and left in the
        // suite, which is the failure mode quarantine itself is prone to and
        // the one no pass/fail history can show.
        if self.quarantined && self.observed() >= 2 {
            if self.fails() == self.observed() {
                return Verdict::Disabled;
            }
            if self.fails() == 0 && self.pass_on_retry() == 0 {
                return Verdict::Recovered;
            }
        }
        if self.transitions() >= 2 {
            Verdict::Flaky
        } else if self.pass_on_retry() > 0 {
            Verdict::Latent
        } else if self.observed() >= 2 && self.fails() == self.observed() {
            Verdict::Broken
        } else if self.observed() < 2 {
            Verdict::New
        } else {
            Verdict::Healthy
        }
    }
}

/// The verdict, ordered by how urgently a human should look at it — the
/// listing sorts by this, worst first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    /// Quarantined and failing every observed run — switched off, not flaky.
    Disabled,
    /// Verdict flapped between runs: the quarantine candidate.
    Flaky,
    /// Green, but only ever after retries — one backoff change from red.
    Latent,
    /// Failed in every observed run: broken, not flaky.
    Broken,
    /// Quarantined but green throughout the window — the tag can come off.
    Recovered,
    /// Seen in fewer than two runs — no history to judge yet.
    New,
    Healthy,
}

impl Verdict {
    fn word(self) -> &'static str {
        match self {
            Self::Disabled => "DISABLED — quarantined and failing every run",
            Self::Flaky => "FLAKY — quarantine candidate (@quarantine)",
            Self::Latent => "passes only on retry (latent)",
            Self::Broken => "always failing (broken, not flaky)",
            Self::Recovered => "green throughout — the @quarantine can come off",
            Self::New => "new — not enough history",
            Self::Healthy => "healthy",
        }
    }

    /// The machine spelling: the stable first word, not the human hint.
    fn key(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Flaky => "flaky",
            Self::Latent => "latent",
            Self::Broken => "broken",
            Self::Recovered => "recovered",
            Self::New => "new",
            Self::Healthy => "healthy",
        }
    }
}

/// Fold the retained records and render verdicts. Exit `0` — the command is
/// informational, like `explain`; a store with fewer than two records is a
/// user error (`2`), the same refusal `diff` gives, because a verdict over
/// one run would be noise wearing a table.
pub fn flaky(runs_root: &Path, output_json: bool, by: Option<&str>) -> ExitCode {
    let runs = record::all_runs(runs_root);
    if runs.len() < 2 {
        crate::render::errln!(
            "error: need at least two runs for a flakiness verdict; found {} under {}",
            runs.len(),
            runs_root.display()
        );
        return ExitCode::UserError;
    }

    // Keyed by (context, file, scenario). Without `--by` every run lands in
    // one unnamed context, which is exactly the old single-bucket fold.
    let mut histories: BTreeMap<(String, Key), History> = BTreeMap::new();
    let mut unreadable = 0usize;
    for dir in &runs {
        let rec = match record::read_record(dir) {
            Ok(rec) => rec,
            Err(err) => {
                // A fold over history degrades, it does not abort: a single
                // half-written dir (a concurrent `proef test` between
                // `create_dir` and its first write, a rotation race, a
                // partial download) used to discard every readable record
                // beside it and tell the user nothing about the rest.
                crate::render::errln!("warning: skipping unreadable run: {err}");
                unreadable += 1;
                continue;
            }
        };
        let context = by.map_or_else(String::new, |key| run_context(&rec, key));
        for (key, run) in rec.scenarios {
            if !run.is_suite() || run.status == Status::Skipped {
                // A phase is not a suite scenario (ADR-0014); a skipped row is
                // a run that never reached it — neither is stability evidence.
                continue;
            }
            let history = histories.entry((context.clone(), key)).or_default();
            history.quarantined |= run
                .tags
                .iter()
                .any(|tag| tag == crate::front::reserved::QUARANTINE);
            history.runs.push(Observation {
                failed: run.status == Status::Failed,
                retried: run.steps.values().any(|s| s.attempts > 1),
                duration_ms: run
                    .steps
                    .values()
                    .fold(0u64, |acc, s| acc.saturating_add(s.duration_ms)),
            });
        }
    }

    // The two-run floor re-applies over what was actually *readable* — with
    // enough unreadable dirs the survivors can dip below it, and a verdict
    // over one run is the noise the floor exists to refuse.
    let readable = runs.len() - unreadable;
    if readable < 2 {
        crate::render::errln!(
            "error: need at least two readable runs for a flakiness verdict; \
             {readable} readable of {} under {}",
            runs.len(),
            runs_root.display()
        );
        return ExitCode::UserError;
    }
    if unreadable > 0 {
        crate::render::errln!(
            "note: verdicts cover {readable} of {} runs ({unreadable} unreadable, listed above)",
            runs.len()
        );
    }

    let mut rows: Vec<((String, Key), History)> = histories.into_iter().collect();
    rows.sort_by(|a, b| {
        a.1.verdict()
            .cmp(&b.1.verdict())
            .then_with(|| a.0.cmp(&b.0))
    });

    if output_json {
        for ((context, (file, scenario)), h) in &rows {
            let object = serde_json::json!({
                "context": by.map(|key| serde_json::json!({ key: context })),
                "file": file,
                "scenario": scenario,
                "runs": h.observed(),
                "fails": h.fails(),
                "transitions": h.transitions(),
                "pass_on_retry": h.pass_on_retry(),
                "p95_ms": h.p95_ms(),
                "quarantined": h.quarantined,
                "verdict": h.verdict().key(),
            });
            crate::render::outln!("{object}");
        }
        return ExitCode::Success;
    }
    render_table(&rows, runs.len(), runs_root, by);
    ExitCode::Success
}

/// The context value a run belongs to under `--by <key>`: the active `--env`
/// when the key is the reserved word `env`, else the `[meta]`/`--meta` value.
///
/// A run that never set the key is its own bucket rather than being folded in
/// with the runs that did — merging them would average a context that was
/// never observed, which is the opposite of what splitting was asked for.
fn run_context(record: &record::Record, key: &str) -> String {
    let value = if key == "env" {
        record.env.clone()
    } else {
        record.metadata.get(key).cloned()
    };
    value.unwrap_or_else(|| "(unset)".to_owned())
}

/// The human listing: header, one row per scenario worst-first, and the
/// quarantine hand-off when anything was flagged.
fn render_table(
    rows: &[((String, Key), History)],
    runs: usize,
    runs_root: &Path,
    by: Option<&str>,
) {
    crate::render::outln!(
        "flakiness over {runs} run(s) under {} (window = [run] keep-runs)\n",
        runs_root.display()
    );
    // Labels once, width from the labels themselves — the spelling and the
    // width can then never disagree about the separator. Under `--by` the
    // context leads the label, so the same scenario's contexts sort together.
    let labels: Vec<String> = rows
        .iter()
        .map(|((context, key), _)| match by {
            Some(_) => format!("[{context}] {}", label(key)),
            None => label(key),
        })
        .collect();
    let width = labels.iter().map(String::len).max().unwrap_or(0).max(8);
    crate::render::outln!(
        "{:width$}  {:>4}  {:>5}  {:>11}  {:>13}  {:>6}  verdict",
        "scenario",
        "runs",
        "fails",
        "transitions",
        "pass-on-retry",
        "p95 ms",
    );
    for ((_, h), name) in rows.iter().zip(&labels) {
        crate::render::outln!(
            "{name:width$}  {:>4}  {:>5}  {:>11}  {:>13}  {:>6}  {}",
            h.observed(),
            h.fails(),
            h.transitions(),
            h.pass_on_retry(),
            h.p95_ms(),
            h.verdict().word(),
        );
    }
    let flagged = rows
        .iter()
        .filter(|(_, h)| matches!(h.verdict(), Verdict::Flaky | Verdict::Latent))
        .count();
    if flagged > 0 {
        crate::render::outln!(
            "\n{flagged} scenario(s) flagged — tag a flapper `@quarantine` to keep it \
             running without gating the exit code while it is fixed"
        );
    }
    // The finding `--by` exists for. A scenario whose verdict *differs*
    // between contexts is not flaky — it is context-dependent, which points at
    // the environment rather than at the test, and is the one conclusion a
    // single merged history can never reach.
    if by.is_some() {
        let mut per_scenario: BTreeMap<&Key, BTreeSet<&'static str>> = BTreeMap::new();
        for ((_, key), history) in rows {
            per_scenario
                .entry(key)
                .or_default()
                .insert(history.verdict().key());
        }
        let split: Vec<String> = per_scenario
            .iter()
            .filter(|(_, verdicts)| verdicts.len() > 1)
            .map(|(key, verdicts)| {
                format!(
                    "  {} — {}",
                    label(key),
                    verdicts.iter().copied().collect::<Vec<_>>().join(" / ")
                )
            })
            .collect();
        if !split.is_empty() {
            crate::render::outln!(
                "\n{} scenario(s) behave differently per context — look at the \
                 environment, not the test:",
                split.len()
            );
            for line in split {
                crate::render::outln!("{line}");
            }
        }
    }

    // The other end of the same pipeline. Quarantine is a holding pen, and the
    // two ways out of it are the two things this can say: it never recovered,
    // or it did.
    let disabled = rows
        .iter()
        .filter(|(_, h)| h.verdict() == Verdict::Disabled)
        .count();
    if disabled > 0 {
        crate::render::outln!(
            "{disabled} quarantined scenario(s) failed every run — nothing is watching \
             them fail; fix or delete rather than leave them switched on and hidden"
        );
    }
    let recovered = rows
        .iter()
        .filter(|(_, h)| h.verdict() == Verdict::Recovered)
        .count();
    if recovered > 0 {
        crate::render::outln!(
            "{recovered} quarantined scenario(s) were green throughout — drop the \
             `@quarantine` so they gate again"
        );
    }
}
