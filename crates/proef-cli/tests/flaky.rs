//! `proef flaky` — the fold, exercised over synthesized histories (R3-2).
//!
//! Records are written directly as `events.jsonl` files under uuid-named run
//! dirs — the JSONL stream *is* the record (ADR-0008), so hand-written events
//! are as real as executed ones, and the histories can express in four files
//! what a live suite would need dozens of runs to produce. The four classes
//! mirror the research prototype that validated the verdict set against real
//! records: a flapper, a pass-only-on-retry, a steady pass, and an
//! always-fail.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use assert_cmd::Command;

fn proef(cwd: &Path) -> Command {
    let mut cmd = Command::cargo_bin("proef").unwrap();
    cmd.current_dir(cwd).env("NO_COLOR", "1");
    cmd
}

/// One scenario's rows in one record: a step (carrying attempts/duration) and
/// the scenario verdict.
fn scenario_events(name: &str, status: &str, attempts: u32) -> String {
    tagged_scenario_events(name, status, attempts, &[])
}

/// The same, with the scenario's accumulated tags — what `@quarantine`
/// verdicts read.
fn tagged_scenario_events(name: &str, status: &str, attempts: u32, tags: &[&str]) -> String {
    let tags = if tags.is_empty() {
        String::new()
    } else {
        format!(
            r#","tags":[{}]"#,
            tags.iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    format!(
        r#"{{"event":"step_finished","scenario":"{name}","engine":"hurl","step":{{"file":"f.feature","line":3,"text":"the step runs"}},"status":"{status}","attempts":{attempts},"duration_ms":5,"captures":[]}}
{{"event":"scenario_finished","scenario":"{name}","file":"f.feature","status":"{status}"{tags}}}
"#
    )
}

/// Write run `n` (uuid-v7-shaped name, so `all_runs` orders it by suffix)
/// holding the given scenario rows.
fn write_run(root: &Path, n: usize, body: &str, cancelled: bool) {
    write_run_in(root, n, body, cancelled, "");
}

/// The same, with extra fields spliced into the `run_started` head — the
/// `env`/`metadata` provenance `--by` segments on (ADR-0020).
fn write_run_in(root: &Path, n: usize, body: &str, cancelled: bool, head_extra: &str) {
    let dir = root.join(format!(".proef-runs/0198f3c1-0000-7000-8000-{n:012}"));
    std::fs::create_dir_all(&dir).unwrap();
    let head = format!(r#"{{"event":"run_started","schema":1,"run_id":"r{n}"{head_extra}}}"#);
    let tail = format!(
        r#"{{"event":"run_finished","passed":0,"failed":0,"skipped":0,"cancelled":{cancelled}}}"#
    );
    std::fs::write(dir.join("events.jsonl"), format!("{head}\n{body}{tail}\n")).unwrap();
}

/// The research prototype's history, in four files: `flappy` fails on runs 2
/// and 4 (three transitions), `retried` always passes on attempt 3, `steady`
/// just passes, `broken` always fails — plus `settled`, which failed twice
/// and then was fixed (F,F,P,P): one transition, two fails. `settled` is the
/// case that separates transition-counting from a naive fail-rate — a
/// fail-rate classifier calls it flaky, and the first draft of these tests
/// could not tell the two classifiers apart (the mutation survived) because
/// no history disagreed between them.
fn seeded_history(cwd: &Path) {
    std::fs::write(cwd.join("proef.toml"), "[run]\nsuite = \"suite\"\n").unwrap();
    for n in 1..=4 {
        let flappy_status = if n == 2 || n == 4 { "failed" } else { "passed" };
        let settled_status = if n <= 2 { "failed" } else { "passed" };
        let body = format!(
            "{}{}{}{}{}",
            scenario_events("flappy", flappy_status, 1),
            scenario_events("retried", "passed", 3),
            scenario_events("steady", "passed", 1),
            scenario_events("broken", "failed", 1),
            scenario_events("settled", settled_status, 1),
        );
        write_run(cwd, n, &body, false);
    }
}

#[test]
fn the_four_verdict_classes_are_told_apart() {
    let cwd = tempfile::tempdir().unwrap();
    seeded_history(cwd.path());

    let assert = proef(cwd.path()).args(["flaky"]).assert().code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();

    let verdict_of = |name: &str| -> String {
        out.lines()
            .find(|l| l.contains(&format!(":: {name}")))
            .unwrap_or_else(|| panic!("no row for {name}:\n{out}"))
            .to_owned()
    };
    assert!(verdict_of("flappy").contains("FLAKY"), "{out}");
    assert!(
        verdict_of("retried").contains("passes only on retry"),
        "{out}"
    );
    assert!(verdict_of("steady").contains("healthy"), "{out}");
    assert!(
        verdict_of("broken").contains("broken, not flaky"),
        "an always-fail is a different problem than a flake: {out}"
    );
    // Fixed-and-stayed-fixed is one transition — NOT flaky. This is the row
    // that makes the classifier transition-counting rather than fail-rate:
    // `settled` has two fails in four runs, and a fail-rate mutant calls it
    // flaky while the real rule calls it healthy.
    assert!(verdict_of("settled").contains("healthy"), "{out}");
    // The detect step hands off to the quarantine step the tag already owns.
    assert!(out.contains("@quarantine"), "{out}");
}

/// `--format json` emits one machine-readable object per scenario, with the
/// counts the table derives its verdicts from.
#[test]
fn json_output_carries_the_counts_behind_the_verdict() {
    let cwd = tempfile::tempdir().unwrap();
    seeded_history(cwd.path());

    let assert = proef(cwd.path())
        .args(["flaky", "--format", "json"])
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let rows: Vec<serde_json::Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is one JSON object"))
        .collect();
    assert_eq!(rows.len(), 5, "{out}");
    let row = |name: &str| -> &serde_json::Value {
        rows.iter()
            .find(|r| r["scenario"] == name)
            .unwrap_or_else(|| panic!("no row for {name}"))
    };
    assert_eq!(row("flappy")["verdict"], "flaky");
    assert_eq!(row("flappy")["transitions"], 3);
    assert_eq!(row("retried")["verdict"], "latent");
    assert_eq!(row("retried")["pass_on_retry"], 4);
    assert_eq!(row("broken")["verdict"], "broken");
    assert_eq!(row("broken")["fails"], 4);
    assert_eq!(row("steady")["verdict"], "healthy");
    assert_eq!(row("settled")["verdict"], "healthy");
    assert_eq!(row("settled")["transitions"], 1);
}

/// A cancellation-skipped scenario is not evidence: a run that never reached
/// it must not count toward its history — and must not turn a steady pass
/// into a transition.
#[test]
fn a_skipped_row_is_not_stability_evidence() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("proef.toml"), "[run]\nsuite = \"suite\"\n").unwrap();
    // Runs 1 and 3: steady passes. Run 2: cancelled before `steady` ran.
    write_run(
        cwd.path(),
        1,
        &scenario_events("steady", "passed", 1),
        false,
    );
    // Run 2: cancelled before `steady` ran — a scenario-level skipped row.
    write_run(
        cwd.path(),
        2,
        "{\"event\":\"scenario_finished\",\"scenario\":\"steady\",\"file\":\"f.feature\",\"status\":\"skipped\"}\n",
        true,
    );
    write_run(
        cwd.path(),
        3,
        &scenario_events("steady", "passed", 1),
        false,
    );

    let assert = proef(cwd.path())
        .args(["flaky", "--format", "json"])
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let row: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(row["runs"], 2, "the skipped row must not count: {out}");
    assert_eq!(row["transitions"], 0, "{out}");
    assert_eq!(row["verdict"], "healthy", "{out}");
}

/// One run is noise wearing a table — the same refusal `diff` gives.
#[test]
fn fewer_than_two_runs_is_a_user_error() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("proef.toml"), "[run]\nsuite = \"suite\"\n").unwrap();
    write_run(cwd.path(), 1, &scenario_events("only", "passed", 1), false);
    let assert = proef(cwd.path()).args(["flaky"]).assert().code(2);
    let err = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(err.contains("need at least two runs"), "{err}");
}

/// Quarantine's own failure mode: a tagged scenario that fails every run is
/// *switched off*, not flaky — and nothing else in the tool can say so.
///
/// Its failures gate nothing by design, so no exit code, no summary and no CI
/// job ever reports them. Without the tag in view `flaky` called it "broken"
/// alongside untagged always-failures, which reads as "someone will notice
/// this" — and for a quarantined scenario, nobody will. The other direction
/// matters as much: a quarantined scenario that has gone green is a tag
/// nobody removed, still suppressing the next real regression.
#[test]
fn a_quarantined_scenario_is_told_apart_from_a_merely_broken_one() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("proef.toml"), "[run]\nsuite = \"suite\"\n").unwrap();
    for n in 1..=3 {
        let body = format!(
            "{}{}{}",
            // Quarantined and never passing: hidden, not flaky.
            tagged_scenario_events("hidden", "failed", 1, &["quarantine"]),
            // Quarantined and green throughout: the tag outlived the problem.
            tagged_scenario_events("cured", "passed", 1, &["quarantine", "slow"]),
            // Untagged and always failing: broken, and visible.
            scenario_events("broken", "failed", 1),
        );
        write_run(cwd.path(), n, &body, false);
    }

    let assert = proef(cwd.path()).args(["flaky"]).assert().code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let row = |name: &str| -> String {
        out.lines()
            .find(|l| l.contains(&format!(":: {name}")))
            .unwrap_or_else(|| panic!("no row for {name}:\n{out}"))
            .to_owned()
    };

    assert!(
        row("hidden").contains("DISABLED"),
        "a quarantined always-failure is switched off, not broken: {out}"
    );
    assert!(
        row("broken").contains("broken, not flaky"),
        "an untagged always-failure keeps its own verdict: {out}"
    );
    assert!(
        row("cured").contains("the @quarantine can come off"),
        "a quarantined scenario that stopped failing should lose the tag: {out}"
    );
    // Both hand-offs name what to do, since "DISABLED" alone is a label.
    assert!(out.contains("nothing is watching them fail"), "{out}");
    assert!(out.contains("drop the `@quarantine`"), "{out}");

    // The machine surface carries the fact the verdict turns on, so a CI job
    // can gate on it without parsing the table.
    let assert = proef(cwd.path())
        .args(["flaky", "--format", "json"])
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let rows: Vec<serde_json::Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is one JSON object"))
        .collect();
    let row = |name: &str| -> &serde_json::Value {
        rows.iter()
            .find(|r| r["scenario"] == name)
            .unwrap_or_else(|| panic!("no row for {name}"))
    };
    assert_eq!(row("hidden")["verdict"], "disabled");
    assert_eq!(row("hidden")["quarantined"], true);
    assert_eq!(row("cured")["verdict"], "recovered");
    assert_eq!(row("broken")["verdict"], "broken");
    assert_eq!(row("broken")["quarantined"], false);
}

/// A scenario that flaps in one environment and is rock solid in another is
/// not flaky — it is context-dependent, and the fix is in the environment.
///
/// A single merged history cannot reach that conclusion: pooled together, the
/// staging failures and the prod passes look exactly like one flapping test.
/// `--by` splits the fold on the provenance the record already carries.
#[test]
fn splitting_by_context_separates_a_flapper_from_an_environment() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("proef.toml"), "[run]\nsuite = \"suite\"\n").unwrap();
    // Four runs alternating environment. `envy` fails only in staging;
    // `steady` passes everywhere.
    for n in 1..=4 {
        let staging = n % 2 == 1;
        let env = if staging { "staging" } else { "prod" };
        let body = format!(
            "{}{}",
            scenario_events("envy", if staging { "failed" } else { "passed" }, 1),
            scenario_events("steady", "passed", 1),
        );
        write_run_in(
            cwd.path(),
            n,
            &body,
            false,
            &format!(r#","env":"{env}","metadata":{{"runner":"{env}-box"}}"#),
        );
    }

    // Pooled, `envy` looks like a classic flapper.
    let assert = proef(cwd.path()).args(["flaky"]).assert().code(0);
    let pooled = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        pooled
            .lines()
            .find(|l| l.contains(":: envy"))
            .is_some_and(|l| l.contains("FLAKY")),
        "merged history cannot see the split: {pooled}"
    );

    // Split by environment, each context is internally consistent — and the
    // callout names the scenario whose verdict depends on where it ran.
    let assert = proef(cwd.path())
        .args(["flaky", "--by", "env"])
        .assert()
        .code(0);
    let split = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let row = |ctx: &str, name: &str| -> String {
        split
            .lines()
            .find(|l| l.contains(&format!("[{ctx}] ")) && l.contains(&format!(":: {name}")))
            .unwrap_or_else(|| panic!("no {ctx}/{name} row:\n{split}"))
            .to_owned()
    };
    assert!(
        row("staging", "envy").contains("broken, not flaky"),
        "in staging it always fails: {split}"
    );
    assert!(
        row("prod", "envy").contains("healthy"),
        "in prod it always passes: {split}"
    );
    assert!(
        split.contains("behave differently per context"),
        "the split itself is the finding: {split}"
    );
    assert!(
        !split.contains("[staging] suite/f.feature :: steady  ")
            || row("staging", "steady").contains("healthy"),
        "a context-independent scenario is healthy everywhere: {split}"
    );

    // An arbitrary `[meta]` key segments the same way `env` does.
    let assert = proef(cwd.path())
        .args(["flaky", "--by", "runner", "--format", "json"])
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let rows: Vec<serde_json::Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).expect("one JSON object per line"))
        .collect();
    let envy: Vec<&serde_json::Value> = rows.iter().filter(|r| r["scenario"] == "envy").collect();
    assert_eq!(envy.len(), 2, "one row per context: {out}");
    assert!(
        envy.iter().any(|r| r["context"]["runner"] == "staging-box"),
        "the context rides in the machine surface: {out}"
    );

    // A run that never set the key is its own bucket, not silently merged.
    let assert = proef(cwd.path())
        .args(["flaky", "--by", "absent", "--format", "json"])
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(out.contains(r#""absent":"(unset)""#), "{out}");
}
