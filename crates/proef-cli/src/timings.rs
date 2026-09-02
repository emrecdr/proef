//! Per-scenario timings, and the duration-balanced shard split they enable.
//!
//! `--shard I/N` assigns by a frozen hash of a scenario's identity, which
//! guarantees the property that made hash-mode worth choosing: adding one
//! scenario never re-buckets the others. What it cannot do is balance by *time*.
//! Scenario durations in an API suite differ by orders of magnitude, and a CI
//! matrix finishes when its slowest shard finishes — so a count-balanced split
//! routinely leaves runners idle.
//!
//! # Why the weights come from a file rather than the local record store
//!
//! proef already retains up to `[run] keep-runs` records carrying every step's
//! duration, so the obvious design is "weight by the newest local record". That
//! is **silently incorrect for the only case sharding exists to serve.** Each
//! shard of a CI matrix runs on a *different machine*, each with its own
//! (usually empty) `runs-dir`. Every job would compute a different weight table,
//! therefore a different assignment — and scenarios would run twice or not at
//! all while the suite still reported green. Nothing about that failure
//! announces itself.
//!
//! So the weights are one file, named explicitly, shared by every job: proef
//! writes `timings.json` into the run directory of every run that reaches its
//! suite, CI archives that one file, and each matrix job points
//! `--shard-weights` at the same copy. The assignment is then a pure function of
//! (selected scenarios, that file).
//!
//! A run whose `[run] setup` aborted writes none — it has no suite to weigh, and
//! a file naming *setup* scenarios would be worse than none at all: those
//! identities never appear in a suite run, so they would absorb bucket load on
//! behalf of scenarios that never run.
//!
//! # What the split gives up, and what it keeps
//!
//! Longest-processing-time-first is not stable under insertion: adding a slow
//! scenario can move others. That is the point — it is what balancing *means* —
//! and it is the one property hash mode had that this trades away. It is opt-in
//! for exactly that reason.
//!
//! A scenario the file does not mention **falls back to the frozen hash**. The
//! two rules partition the set rather than competing for it: a scenario is
//! either in the table or not, so it lands in exactly one shard either way. A
//! test added after the timings were captured therefore still runs exactly once,
//! which is the property that must never bend.

use std::collections::BTreeMap;
use std::path::Path;

use proef_core::runner::RunSummary;

/// The sidecar's schema version. Bumped only on a breaking shape change; a
/// reader that does not recognise the version refuses rather than guesses.
const TIMINGS_SCHEMA: u32 = 1;

/// Each scenario's measured cost in milliseconds, keyed by `(file, scenario)`.
///
/// Cost is [`ScenarioOutcome::cost`](proef_core::runner::ScenarioOutcome::cost)
/// — the sum of a scenario's step durations rather than its wall-clock span,
/// which is where that choice is argued. The same definition backs `JUnit`'s
/// times and the HTML report's "Slowest" ranking, so a weights file can never
/// disagree with a report about what a scenario costs.
pub type Weights = BTreeMap<(String, String), u64>;

/// Render the sidecar for a finished run.
///
/// Keyed by `(file, scenario)` — the same identity `--shard` hashes and `diff`
/// and `--rerun` key on — so a weights file and a shard filter can never
/// disagree about what a scenario *is*.
#[must_use]
pub fn render(summary: &RunSummary) -> String {
    // Built as the very `Weights` map `read` returns, which buys two things: the
    // writer and the reader cannot drift apart about the shape, and rows come
    // out ordered by identity rather than by completion order without a sort —
    // the file is an input to a deterministic split, so two runs of the same
    // suite must differ only where the measurements did.
    let weights: Weights = summary
        .outcomes
        .iter()
        .map(|outcome| {
            let ms = u64::try_from(outcome.cost().as_millis()).unwrap_or(u64::MAX);
            ((outcome.file.to_string(), outcome.name.to_string()), ms)
        })
        .collect();
    let rows: Vec<serde_json::Value> = weights
        .iter()
        .map(|((file, scenario), ms)| {
            serde_json::json!({ "file": file, "scenario": scenario, "ms": ms })
        })
        .collect();
    let body = serde_json::json!({ "schema": TIMINGS_SCHEMA, "scenarios": rows });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&body).unwrap_or_default()
    )
}

/// Read a weights file. Errors are the caller's to report as user errors: a
/// `--shard-weights` that cannot be read is a typo'd path or a stale artifact,
/// and silently falling back to the hash would hand back the unbalanced split
/// the flag was passed to avoid — without saying so.
pub fn read(path: &Path) -> Result<Weights, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("cannot read shard weights `{}`: {err}", path.display()))?;
    let body: serde_json::Value = serde_json::from_str(&text)
        .map_err(|err| format!("`{}` is not valid JSON: {err}", path.display()))?;
    let schema = body.get("schema").and_then(serde_json::Value::as_u64);
    if schema != Some(u64::from(TIMINGS_SCHEMA)) {
        return Err(format!(
            "`{}` declares schema {} — this proef reads {TIMINGS_SCHEMA}",
            path.display(),
            schema.map_or_else(|| "none".to_owned(), |v| v.to_string())
        ));
    }
    let rows = body
        .get("scenarios")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("`{}` has no `scenarios` array", path.display()))?;
    let mut weights = Weights::new();
    for row in rows {
        let (Some(file), Some(scenario), Some(ms)) = (
            row.get("file").and_then(serde_json::Value::as_str),
            row.get("scenario").and_then(serde_json::Value::as_str),
            row.get("ms").and_then(serde_json::Value::as_u64),
        ) else {
            continue; // a row proef did not write; skip rather than fail the run
        };
        weights.insert((file.to_owned(), scenario.to_owned()), ms);
    }
    Ok(weights)
}

/// Which of `count` shards each weighted scenario belongs to, 0-based.
///
/// Longest-processing-time-first: heaviest scenario to the lightest shard,
/// repeatedly. It is the classic greedy approximation and it is good enough here
/// — the bound is 4/3 of optimal, against a count-split that has no bound at
/// all when one scenario dominates.
///
/// Ties break on identity, never on input order, so the assignment is a pure
/// function of the weights file and does not depend on how the caller happened
/// to enumerate scenarios.
#[must_use]
pub fn assign(weights: &Weights, count: u32) -> BTreeMap<(String, String), u32> {
    let mut ordered: Vec<(&(String, String), &u64)> = weights.iter().collect();
    ordered.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

    let buckets = count.max(1) as usize;
    let mut load = vec![0u64; buckets];
    let mut taken = vec![0usize; buckets];
    let mut assigned = BTreeMap::new();
    for (identity, ms) in ordered {
        // Lightest shard, then fewest scenarios, then lowest index.
        //
        // The count is not a nicety — without it a **zero-cost** scenario
        // inverts the whole flag. Costs are whole milliseconds, so anything
        // sub-millisecond stores as `0`, which is routine for a fast suite;
        // adding `0` never moves `load`, so shard 0 stayed the minimum
        // forever and every such scenario piled into it. An all-zero file
        // put the entire suite in shard 0 and left every other shard
        // selecting nothing — the exact opposite of balancing, from the flag
        // whose purpose is balance. Counting assignments keeps the deal
        // round-robin when the weights cannot separate the scenarios, which
        // is also the right answer: equal cost, equal share.
        let target = (0..buckets)
            .min_by_key(|&i| (load[i], taken[i], i))
            .unwrap_or(0);
        load[target] = load[target].saturating_add(*ms);
        taken[target] += 1;
        assigned.insert(identity.clone(), u32::try_from(target).unwrap_or(0));
    }
    assigned
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{Weights, assign, read};

    fn weights(rows: &[(&str, &str, u64)]) -> Weights {
        rows.iter()
            .map(|(f, s, ms)| (((*f).to_owned(), (*s).to_owned()), *ms))
            .collect()
    }

    /// The whole point: one dominant scenario gets a shard to itself rather
    /// than being averaged in by a count-split.
    #[test]
    fn the_heaviest_scenario_lands_alone() {
        let w = weights(&[
            ("f.feature", "slow", 10_000),
            ("f.feature", "a", 100),
            ("f.feature", "b", 100),
            ("f.feature", "c", 100),
        ]);
        let split = assign(&w, 2);
        let slow = split[&("f.feature".to_owned(), "slow".to_owned())];
        for name in ["a", "b", "c"] {
            assert_ne!(
                split[&("f.feature".to_owned(), name.to_owned())],
                slow,
                "{name} should not share the shard carrying the 10s scenario"
            );
        }
    }

    /// Every weighted scenario lands in exactly one shard, and every shard index
    /// is in range. This is the property whose absence would run a scenario
    /// twice or not at all — the failure that reports green.
    #[test]
    fn the_split_is_a_total_function_into_range() {
        let w = weights(&[
            ("a.feature", "one", 5),
            ("a.feature", "two", 900),
            ("b.feature", "three", 40),
            ("b.feature", "four", 40),
            ("c.feature", "five", 1),
        ]);
        for count in 1..=6u32 {
            let split = assign(&w, count);
            assert_eq!(split.len(), w.len(), "every scenario assigned at {count}");
            assert!(
                split.values().all(|&shard| shard < count),
                "shard index out of range at {count}"
            );
        }
    }

    /// The assignment must not depend on enumeration order — a matrix job that
    /// discovered scenarios in a different order must compute the same split.
    #[test]
    fn equal_weights_break_ties_on_identity_not_order() {
        let forward = weights(&[("f", "a", 10), ("f", "b", 10), ("f", "c", 10)]);
        let mut reversed: Vec<_> = forward.iter().collect();
        reversed.reverse();
        let rebuilt: Weights = reversed.into_iter().map(|(k, v)| (k.clone(), *v)).collect();
        assert_eq!(assign(&forward, 2), assign(&rebuilt, 2));
    }

    /// **Zero-cost scenarios must still spread.** Costs are whole
    /// milliseconds, so every sub-millisecond scenario stores as `0` — routine
    /// for a fast suite. Balancing on load alone never advanced past shard 0
    /// for those, so an all-zero file put the whole suite in one shard and
    /// left the others selecting nothing: the flag doing the opposite of its
    /// purpose, silently, with the partition still exact so nothing complained.
    #[test]
    fn zero_cost_scenarios_spread_instead_of_piling_into_shard_zero() {
        let w = weights(&[
            ("f.feature", "a", 0),
            ("f.feature", "b", 0),
            ("f.feature", "c", 0),
            ("f.feature", "d", 0),
        ]);
        let split = assign(&w, 2);
        let mut per_shard = [0usize; 2];
        for shard in split.values() {
            per_shard[*shard as usize] += 1;
        }
        assert_eq!(
            per_shard,
            [2, 2],
            "an all-zero weights file must still deal evenly: {split:?}"
        );

        // Mixed: the one real cost dominates, and the zeros fill the rest
        // rather than all landing beside each other.
        let mixed = weights(&[
            ("f.feature", "heavy", 900),
            ("f.feature", "z1", 0),
            ("f.feature", "z2", 0),
            ("f.feature", "z3", 0),
        ]);
        let split = assign(&mixed, 3);
        let heavy = split[&("f.feature".to_owned(), "heavy".to_owned())];
        let zeros: Vec<u32> = ["z1", "z2", "z3"]
            .iter()
            .map(|n| split[&("f.feature".to_owned(), (*n).to_owned())])
            .collect();
        assert!(
            zeros.iter().all(|s| *s != heavy),
            "nothing should join the 900ms shard while empty shards exist: {split:?}"
        );
        assert_eq!(
            zeros
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            2,
            "the zeros fill both remaining shards rather than stacking on one: {split:?}"
        );
    }

    #[test]
    fn a_weights_file_of_the_wrong_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.json");
        std::fs::write(&path, r#"{"schema": 99, "scenarios": []}"#).unwrap();
        let err = read(&path).expect_err("schema 99 is not readable");
        assert!(err.contains("schema 99"), "{err}");

        std::fs::write(&path, "not json").unwrap();
        assert!(read(&path).is_err(), "malformed JSON must not pass");

        // A missing file is an error, never a silent fall back to the
        // unbalanced split the flag was passed to avoid.
        assert!(read(&dir.path().join("absent.json")).is_err());
    }

    /// Round-trip: what `render` writes, `read` reads.
    #[test]
    fn a_rendered_sidecar_reads_back_identically() {
        use proef_core::runner::RunSummary;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("timings.json");
        let summary = RunSummary {
            outcomes: Vec::new(),
            passed: 0,
            failed: 0,
            skipped: 0,
            cancelled: false,
        };
        std::fs::write(&path, super::render(&summary)).unwrap();
        assert!(
            read(&path).unwrap().is_empty(),
            "an empty run has no weights"
        );
    }
}
