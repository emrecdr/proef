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
    format!(
        r#"{{"event":"step_finished","scenario":"{name}","engine":"hurl","step":{{"file":"f.feature","line":3,"text":"the step runs"}},"status":"{status}","attempts":{attempts},"duration_ms":5,"captures":[]}}
{{"event":"scenario_finished","scenario":"{name}","file":"f.feature","status":"{status}"}}
"#
    )
}

/// Write run `n` (uuid-v7-shaped name, so `all_runs` orders it by suffix)
/// holding the given scenario rows.
fn write_run(root: &Path, n: usize, body: &str, cancelled: bool) {
    let dir = root.join(format!(".proef-runs/0198f3c1-0000-7000-8000-{n:012}"));
    std::fs::create_dir_all(&dir).unwrap();
    let head = format!(r#"{{"event":"run_started","schema":1,"run_id":"r{n}"}}"#);
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
