//! `--shard I/N` — stable hash-mode sharding (R3-3).
//!
//! Hash-by-identity rather than index-slicing is the whole design: the
//! triage measured the alternative (adding one scenario re-buckets 2 of 3
//! under slicing, none under hashing), so the load-bearing property here is
//! **stability** — a scenario's shard never depends on which other scenarios
//! exist. The assignment itself is frozen by a unit test beside
//! `shard_bucket`; these tests pin the CLI semantics over a live suite.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::Path;

use assert_cmd::Command;
use proef_fixture::Fixture;

fn proef_in(dir: &Path, fixture: &Fixture) -> Command {
    let mut cmd = Command::cargo_bin("proef").unwrap();
    cmd.current_dir(dir)
        .env("NO_COLOR", "1")
        .env("PROEF_BASE_URL", &fixture.base_url);
    cmd
}

/// A suite of `names` passing scenarios in one feature file.
fn project(root: &Path, names: &[&str]) {
    std::fs::create_dir_all(root.join("suite/packs")).unwrap();
    std::fs::write(
        root.join("proef.toml"),
        "[run]\nsuite = \"suite\"\n\n[url]\nbase = \"${env:PROEF_BASE_URL}\"\n",
    )
    .unwrap();
    let mut scenarios = String::new();
    for n in names {
        use std::fmt::Write as _;
        let _ = write!(scenarios, "  Scenario: {n}\n    When health is checked\n");
    }
    std::fs::write(
        root.join("suite/case.feature"),
        format!("Feature: F\n{scenarios}"),
    )
    .unwrap();
    std::fs::write(
        root.join("suite/packs/p.yaml"),
        "macros:\n  ok:\n    match: health is checked\n    steps:\n      - hurl: |\n          \
         GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();
}

/// The scenario names a run executed, read from its console tree.
fn ran(assert: &assert_cmd::assert::Assert) -> BTreeSet<String> {
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    out.lines()
        .filter_map(|l| l.trim().strip_prefix("Scenario: "))
        .map(|rest| rest.split_whitespace().next().unwrap_or("").to_owned())
        .collect()
}

const SIX: [&str; 6] = ["s1", "s2", "s3", "s4", "s5", "s6"];

/// The shards partition the selection: disjoint, and their union is exactly
/// the un-sharded set — no scenario lost, none run twice.
#[test]
fn shards_partition_the_suite() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    project(cwd.path(), &SIX);

    let one = ran(&proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--shard", "1/2"])
        .assert());
    let two = ran(&proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--shard", "2/2"])
        .assert());

    assert!(one.is_disjoint(&two), "shards overlap: {one:?} ∩ {two:?}");
    let union: BTreeSet<_> = one.union(&two).cloned().collect();
    let all: BTreeSet<String> = SIX.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(union, all, "the shards must cover the whole suite");

    // Determinism: the same shard twice is the same set.
    let again = ran(&proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--shard", "1/2"])
        .assert());
    assert_eq!(one, again, "a shard must be reproducible");
}

/// The headline property — the reason hash mode is the only mode: adding a
/// scenario never moves the existing ones between shards.
#[test]
fn adding_a_scenario_rebuckets_nothing() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    project(cwd.path(), &SIX);
    let before = ran(&proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--shard", "1/2"])
        .assert());

    // PREPENDED, not appended, on purpose: appending leaves every existing
    // position untouched, so even index-slicing passes — the first draft of
    // this test proved that by surviving a slicing mutation. Insertion shifts
    // every position after it, which is exactly the case the measurement was
    // about: slicing re-buckets the shifted tail, hashing moves nothing.
    let mut seven = vec!["s0-the-new-one"];
    seven.extend(SIX);
    project(cwd.path(), &seven);
    let after = ran(&proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--shard", "1/2"])
        .assert());

    // Every original scenario is in shard 1 after exactly iff it was before;
    // only the newcomer may appear.
    let after_originals: BTreeSet<_> = after
        .iter()
        .filter(|n| !n.starts_with("s0"))
        .cloned()
        .collect();
    assert_eq!(
        before, after_originals,
        "inserting s0 must not move any existing scenario between shards"
    );
}

/// An empty shard of a non-empty selection is a fact, not a mistake: a small
/// suite spread over a big matrix must not fail its idle jobs. An empty
/// *selection* keeps the loud typo'd-filter refusal — sharding must not blunt
/// it.
#[test]
fn an_empty_shard_is_a_note_and_an_empty_selection_stays_loud() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    project(cwd.path(), &["only"]);

    // One scenario, two shards: exactly one shard holds it, the other is
    // legitimately idle. Which is which is fixed by the frozen hash — assert
    // the shape, not the assignment.
    let mut empties = 0;
    for shard in ["1/2", "2/2"] {
        let assert = proef_in(cwd.path(), &fixture)
            .args(["test", "suite", "--shard", shard])
            .assert()
            .code(0);
        let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        if out.contains("nothing to run in this shard") {
            empties += 1;
            assert!(
                out.contains("selected 0 of 1"),
                "the note must carry the counts: {out}"
            );
        }
    }
    assert_eq!(empties, 1, "exactly one of two shards should be idle");

    // A filter that selects nothing is still exit 2, sharded or not.
    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--tags", "no-such-tag", "--shard", "1/2"])
        .assert()
        .code(2);
}

/// `--shard` composes with the other selectors: it partitions the *filtered*
/// set (the pinned filter→shard order), so every matrix job slicing the same
/// expression partitions one agreed-on set.
#[test]
fn shard_partitions_the_filtered_set() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[run]\nsuite = \"suite\"\n\n[url]\nbase = \"${env:PROEF_BASE_URL}\"\n",
    )
    .unwrap();
    // Three tagged, three not.
    let mut scenarios = String::new();
    for n in 1..=6 {
        use std::fmt::Write as _;
        let tag = if n <= 3 { "  @picked\n" } else { "" };
        let _ = write!(
            scenarios,
            "{tag}  Scenario: s{n}\n    When health is checked\n"
        );
    }
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        format!("Feature: F\n{scenarios}"),
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  ok:\n    match: health is checked\n    steps:\n      - hurl: |\n          \
         GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    let mut union: BTreeSet<String> = BTreeSet::new();
    for shard in ["1/2", "2/2"] {
        let set = ran(&proef_in(cwd.path(), &fixture)
            .args(["test", "suite", "--tags", "picked", "--shard", shard])
            .assert());
        union.extend(set);
    }
    let picked: BTreeSet<String> = (1..=3).map(|n| format!("s{n}")).collect();
    assert_eq!(
        union, picked,
        "the shards must partition exactly the tag-filtered set"
    );
}
