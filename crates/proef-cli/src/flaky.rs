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

/// The statistical guards a verdict applies — the 2026-field discipline that
/// separates a real flakiness signal from noise (0.18 survey §6). Every value
/// is a pure parameter of the fold, so the whole classification stays a
/// function of (records, thresholds).
#[derive(Clone, Copy)]
pub struct FlakyThresholds {
    /// The minimum observed runs before a scenario is classified at all.
    /// Below it, `InsufficientData` — a verdict on thin data is worse than
    /// none (the industry default is 10; Trunk refuses a call at n=8).
    pub min_samples: usize,
    /// The length of the trailing all-clean run that resolves a `Flaky` or
    /// `Latent` scenario back to `Healthy` — the anti-flap hysteresis. A
    /// scenario that flapped historically holds its flag until it has earned
    /// this many consecutive clean passes (Trunk's recovery window).
    pub recovery_runs: usize,
    /// A run whose share of failing suite scenarios exceeds this is an
    /// environment outage, not evidence about any one scenario, and its
    /// observations are excluded entirely (Trunk's Infrastructure Failure
    /// Protection — the failure mode most likely to bite an HTTP runner).
    pub outage_rate: f64,
}

impl Default for FlakyThresholds {
    fn default() -> Self {
        Self {
            min_samples: 10,
            recovery_runs: 5,
            outage_rate: 0.8,
        }
    }
}

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

    /// The trailing run is all clean (passed, no retry) for `n` observations —
    /// the hysteresis signal that a historically-unstable scenario has earned
    /// its way back to `Healthy`. `false` when there are fewer than `n`
    /// observations, so a scenario cannot resolve before it has had the
    /// chance to prove it.
    fn clean_tail(&self, n: usize) -> bool {
        n > 0
            && self.runs.len() >= n
            && self.runs[self.runs.len() - n..]
                .iter()
                .all(|o| !o.failed && !o.retried)
    }

    /// The classification, from the derived counts and the statistical
    /// guards. Transition-counting rather than fail-rate is the load-bearing
    /// choice: F,F,P,P is a fix that stuck (one transition), not a flake —
    /// fail-rate cannot tell those apart. On top of that (0.18 survey §6): a
    /// minimum-sample floor refuses a verdict on thin data, and hysteresis
    /// holds a flag until a clean recovery tail, so a scenario cannot flap
    /// `flaky`↔`healthy` between adjacent runs.
    fn verdict(&self, t: &FlakyThresholds) -> Verdict {
        // A verdict on thin data is noise wearing a table — refuse to
        // classify below the floor (the outage guard has already excluded
        // non-evidence runs, so this counts real observations).
        if self.observed() < t.min_samples {
            return Verdict::InsufficientData;
        }
        // Quarantine is asked first because it changes what a result *means*,
        // not just how urgent it is. A quarantined scenario's failures gate
        // nothing, so nobody is looking at them: always-failing under
        // quarantine is a test that has been switched off and left in the
        // suite, which is the failure mode quarantine itself is prone to and
        // the one no pass/fail history can show.
        if self.quarantined {
            if self.fails() == self.observed() {
                return Verdict::Disabled;
            }
            if self.fails() == 0 && self.pass_on_retry() == 0 {
                return Verdict::Recovered;
            }
        }
        // Broken outranks flaky and is checked before the recovery tail:
        // failing every run is a consistent problem, not instability, and a
        // clean tail cannot apply to a scenario that never passed.
        if self.fails() == self.observed() {
            return Verdict::Broken;
        }
        // Hysteresis: a scenario that flapped or passed-only-on-retry holds
        // that flag until it has earned a trailing clean run of
        // `recovery_runs`, at which point it resolves to Healthy. Without
        // this, a scenario hovering at the boundary flips verdict every run.
        let recovered = self.clean_tail(t.recovery_runs);
        if self.transitions() >= 2 && !recovered {
            Verdict::Flaky
        } else if self.pass_on_retry() > 0 && !recovered {
            Verdict::Latent
        } else {
            Verdict::Healthy
        }
    }
}

/// The verdict, ordered by how urgently a human should look at it — the
/// listing sorts by this, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
    /// Seen in fewer than `min-samples` runs — not enough data to judge.
    InsufficientData,
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
            Self::InsufficientData => "insufficient data — below the sample floor",
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
            Self::InsufficientData => "insufficient-data",
            Self::Healthy => "healthy",
        }
    }
}

/// Fold the retained records and render verdicts. Exit `0` — the command is
/// informational, like `explain`; a store with fewer than two records is a
/// user error (`2`), the same refusal `diff` gives, because a verdict over
/// one run would be noise wearing a table.
pub fn flaky(
    runs_root: &Path,
    output_json: bool,
    by: Option<&str>,
    thresholds: FlakyThresholds,
) -> ExitCode {
    let runs = record::all_runs(runs_root);
    if runs.len() < 2 {
        crate::render::errln!(
            "error: need at least two runs for a flakiness verdict; found {} under {}",
            runs.len(),
            runs_root.display()
        );
        return ExitCode::UserError;
    }

    // Keyed by (context, file, scenario). The default context is the run's
    // input fingerprint (0.18 survey §6): a pack or `proef.toml` edit changes
    // what a scenario *is*, so runs of different inputs must not share a
    // window. Under `--by` the caller's grouping wins instead.
    let mut histories: BTreeMap<(String, Key), History> = BTreeMap::new();
    let mut unreadable = 0usize;
    let mut outages = 0usize;
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
        // A run where too many suite scenarios failed is an environment
        // outage (a fixture or staging incident), not evidence about any one
        // scenario — exclude it wholesale, or a single outage would mark the
        // whole suite broken.
        if is_outage(&rec, thresholds.outage_rate) {
            outages += 1;
            continue;
        }
        let context = match by {
            Some(key) => run_context(&rec, key),
            // The fingerprint sidecar (`inputs.json`); absent on records that
            // predate the field, which then share the empty-string window —
            // the old single-bucket behaviour, so old records still fold.
            None => read_fingerprint(dir).unwrap_or_default(),
        };
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

    // The two-run floor re-applies over runs that were actually *evidence* —
    // readable and not an outage. With enough of either excluded the
    // survivors can dip below it, and a verdict over one run is the noise the
    // floor exists to refuse.
    let counted = runs.len() - unreadable - outages;
    if counted < 2 {
        crate::render::errln!(
            "error: need at least two usable runs for a flakiness verdict; \
             {counted} usable of {} under {} ({unreadable} unreadable, {outages} outage)",
            runs.len(),
            runs_root.display()
        );
        return ExitCode::UserError;
    }
    if unreadable > 0 {
        crate::render::errln!(
            "note: verdicts cover {counted} of {} runs ({unreadable} unreadable, listed above)",
            runs.len()
        );
    }
    if outages > 0 {
        crate::render::errln!(
            "note: {outages} run(s) excluded as environment outages \
             (over {:.0}% of suite scenarios failed) — not evidence about any one scenario",
            thresholds.outage_rate * 100.0
        );
    }

    // Classify once, here, then sort and render on the stored verdict. The
    // verdict is a fold over the whole run slice, so recomputing it inside the
    // comparator (twice per comparison) and again at each render site is the
    // same answer paid for many times over.
    let mut rows: Vec<((String, Key), History, Verdict)> = histories
        .into_iter()
        .map(|(key, history)| {
            let verdict = history.verdict(&thresholds);
            (key, history, verdict)
        })
        .collect();
    rows.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));

    if output_json {
        for ((context, (file, scenario)), h, verdict) in &rows {
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
                "verdict": verdict.key(),
            });
            crate::render::outln!("{object}");
        }
        return ExitCode::Success;
    }
    render_table(&rows, runs.len(), runs_root, by);
    ExitCode::Success
}

/// Is this run an environment outage — a run where the share of failing suite
/// scenarios exceeds `rate`? Such a run says nothing about any one scenario's
/// stability (the environment fell over), so its observations are excluded.
/// A run with no suite scenarios (an aborted setup) is not an outage — there
/// is nothing to have failed.
fn is_outage(record: &record::Record, rate: f64) -> bool {
    let mut total = 0u32;
    let mut failed = 0u32;
    for run in record.scenarios.values() {
        if !run.is_suite() || run.status == Status::Skipped {
            continue;
        }
        total += 1;
        if run.status == Status::Failed {
            failed += 1;
        }
    }
    if total == 0 {
        // No suite scenarios (an aborted setup): nothing to have failed.
        return false;
    }
    // `failed/total > rate` ⟺ `failed > rate*total` (no division); the counts
    // are small, so `f64::from` is a lossless widening — no cast for clippy to
    // flag, and no `Vec` allocated just to count.
    f64::from(failed) > rate * f64::from(total)
}

/// The run's input fingerprint from its `inputs.json` sidecar, or `None` when
/// the file is absent (a record that predates the field) or unreadable. A
/// bounded read: the sidecar is a few dozen bytes, so no ceiling is needed
/// beyond the filesystem's.
fn read_fingerprint(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("inputs.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("fingerprint")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
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
    rows: &[((String, Key), History, Verdict)],
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
        .map(|((context, key), _, _)| match by {
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
    for ((_, h, verdict), name) in rows.iter().zip(&labels) {
        crate::render::outln!(
            "{name:width$}  {:>4}  {:>5}  {:>11}  {:>13}  {:>6}  {}",
            h.observed(),
            h.fails(),
            h.transitions(),
            h.pass_on_retry(),
            h.p95_ms(),
            verdict.word(),
        );
    }
    let flagged = rows
        .iter()
        .filter(|(_, _, verdict)| matches!(verdict, Verdict::Flaky | Verdict::Latent))
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
        for ((_, key), _, verdict) in rows {
            per_scenario.entry(key).or_default().insert(verdict.key());
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
        .filter(|(_, _, verdict)| matches!(verdict, Verdict::Disabled))
        .count();
    if disabled > 0 {
        crate::render::outln!(
            "{disabled} quarantined scenario(s) failed every run — nothing is watching \
             them fail; fix or delete rather than leave them switched on and hidden"
        );
    }
    let recovered = rows
        .iter()
        .filter(|(_, _, verdict)| matches!(verdict, Verdict::Recovered))
        .count();
    if recovered > 0 {
        crate::render::outln!(
            "{recovered} quarantined scenario(s) were green throughout — drop the \
             `@quarantine` so they gate again"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{FlakyThresholds, History, Observation, Verdict};

    /// Build a history from a pass/fail/retry string: `.` pass, `F` fail,
    /// `r` pass-on-retry. Oldest first.
    fn history(pattern: &str, quarantined: bool) -> History {
        let runs = pattern
            .chars()
            .map(|c| Observation {
                failed: c == 'F',
                retried: c == 'r',
                duration_ms: 1,
            })
            .collect();
        History { runs, quarantined }
    }

    fn t(min_samples: usize, recovery_runs: usize) -> FlakyThresholds {
        FlakyThresholds {
            min_samples,
            recovery_runs,
            outage_rate: 0.8,
        }
    }

    /// The floor: below `min_samples` a scenario is insufficient-data, whatever
    /// its pattern; at the floor it classifies (0.18 survey §6).
    #[test]
    fn the_sample_floor_refuses_a_verdict_on_thin_data() {
        // Nine flapping runs, floor 10 → insufficient-data.
        let nine = history("F.F.F.F.F", false);
        assert_eq!(nine.verdict(&t(10, 5)), Verdict::InsufficientData);
        // Ten → the flapper is now classifiable.
        let ten = history("F.F.F.F.F.", false);
        assert_eq!(ten.verdict(&t(10, 5)), Verdict::Flaky);
    }

    /// Hysteresis: a flapper holds its flag until a trailing clean run of
    /// `recovery_runs`; one short of it still reads flaky.
    #[test]
    fn hysteresis_holds_a_flag_until_the_recovery_tail() {
        // Flapped early, then a clean tail of exactly 5 → resolves to healthy.
        let recovered = history("F.F.F.....", false);
        assert_eq!(
            recovered.verdict(&t(2, 5)),
            Verdict::Healthy,
            "a full clean recovery tail resolves the flag"
        );
        // A clean tail of only 4 (one short) → the flag holds.
        let holding = history("F.F.FF....", false);
        assert_eq!(
            holding.verdict(&t(2, 5)),
            Verdict::Flaky,
            "one short of the recovery tail keeps the flag"
        );
    }

    /// Broken outranks flaky and is unaffected by the recovery tail — failing
    /// every run is a consistent problem, and no clean tail can apply.
    #[test]
    fn broken_is_not_flaky_and_ignores_recovery() {
        let broken = history("FFFFFFFFFF", false);
        assert_eq!(broken.verdict(&t(10, 5)), Verdict::Broken);
    }

    /// Quarantine lifecycle still classifies above the floor.
    #[test]
    fn quarantine_states_survive_the_floor() {
        let disabled = history("FFFFFFFFFF", true);
        assert_eq!(disabled.verdict(&t(10, 5)), Verdict::Disabled);
        let recovered = history("..........", true);
        assert_eq!(recovered.verdict(&t(10, 5)), Verdict::Recovered);
    }

    /// The outage guard: a run failing over `outage_rate` of its suite
    /// scenarios is excluded; a run at or below it is evidence.
    #[test]
    fn an_outage_run_is_excluded_from_evidence() {
        use crate::record::{Record, RunCompletion, ScenarioRun};
        use proef_core::step::Status;
        use std::collections::BTreeMap;

        fn run(status: Status) -> ScenarioRun {
            ScenarioRun {
                status,
                phase: None,
                reason: None,
                tags: Vec::new(),
                steps: BTreeMap::new(),
            }
        }
        let record = |statuses: &[Status]| Record {
            env: None,
            metadata: BTreeMap::new(),
            rerun_of: None,
            completion: RunCompletion::Completed,
            legacy_multi_pair: false,
            totals: None,
            scenarios: statuses
                .iter()
                .enumerate()
                .map(|(i, s)| ((format!("f{i}"), format!("s{i}")), run(*s)))
                .collect(),
        };
        // 3 of 4 failed = 75% ≤ 80% → not an outage.
        let ok = record(&[
            Status::Failed,
            Status::Failed,
            Status::Failed,
            Status::Passed,
        ]);
        assert!(!super::is_outage(&ok, 0.8));
        // 5 of 5 failed = 100% > 80% → outage.
        let down = record(&[Status::Failed; 5]);
        assert!(super::is_outage(&down, 0.8));
        // No suite scenarios (an aborted setup) is never an outage.
        let empty = record(&[]);
        assert!(!super::is_outage(&empty, 0.8));
    }
}
