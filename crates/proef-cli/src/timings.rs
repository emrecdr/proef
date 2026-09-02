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
//! writes `timings.json` into each run directory, CI archives that one file, and
//! each matrix job points `--shard-weights` at the same copy. The assignment is
//! then a pure function of (selected scenarios, that file).
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

/// One scenario's measured cost: the sum of its steps' durations.
///
/// The sum rather than a wall-clock span, deliberately. The span includes time
/// the scenario spent waiting for a worker, which is a property of the *run's*
/// scheduling and not of the scenario — feeding it back into the next split
/// would let one crowded run's queueing distort the next one's balance.
pub type Weights = BTreeMap<(String, String), u64>;

/// Render the sidecar for a finished run.
///
/// Keyed by `(file, scenario)` — the same identity `--shard` hashes and `diff`
/// and `--rerun` key on — so a weights file and a shard filter can never
/// disagree about what a scenario *is*.
#[must_use]
pub fn render(summary: &RunSummary) -> String {
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for outcome in &summary.outcomes {
        let ms: u64 = outcome
            .steps
            .iter()
            .map(|step| u64::try_from(step.duration.as_millis()).unwrap_or(u64::MAX))
            .sum();
        rows.push(serde_json::json!({
            "file": outcome.file.as_ref(),
            "scenario": outcome.name.as_ref(),
            "ms": ms,
        }));
    }
    // Sorted by identity, not by completion order: the file is an input to a
    // deterministic split, and two runs of the same suite should differ only
    // where the measurements did.
    rows.sort_by(|a, b| {
        (a["file"].as_str(), a["scenario"].as_str())
            .cmp(&(b["file"].as_str(), b["scenario"].as_str()))
    });
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
    let mut assigned = BTreeMap::new();
    for (identity, ms) in ordered {
        // The lightest shard; the lowest index among equals, so an all-equal
        // set deals round-robin rather than piling onto whichever `min_by_key`
        // happened to see first.
        let target = (0..buckets).min_by_key(|&i| (load[i], i)).unwrap_or(0);
        load[target] = load[target].saturating_add(*ms);
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
