//! The execution integration suite (M3 acceptance, TESTING-STRATEGY):
//! the reference corpus runs green against the fixture with prose unchanged
//! (US-1); failures map to feature line + artifact span; `optional:` warns and
//! continues; World/global chains across scenarios; cookies survive batch
//! splits; runaway scenarios are bounded; parallel runs are deterministic
//! under event normalization; the executed artifact is byte-identical to the
//! emitted one (ADR-0010).

#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(
    clippy::doc_markdown,
    clippy::case_sensitive_file_extension_comparisons
)]
#![allow(clippy::assigning_clones)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use assert_cmd::Command;
use predicates::prelude::*;
use proef_fixture::{API_TOKEN, Fixture};

/// The minimal `proef.toml` the inline-macro fixture tests need: just `base`,
/// sourced from `PROEF_BASE_URL` (resolved recursively). Written into each such
/// test's CWD so their `${url:base}` resolves — one spelling, not copy-pasted per
/// test. (The reference-corpus test copies the real project proef.toml instead,
/// since it exercises the full `[url]` endpoint catalog.)
const BASE_URL_CONFIG: &str = "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn proef_in(dir: &Path, fixture: &Fixture) -> Command {
    // Variables live in proef.toml, never in the .feature: give the suite its
    // base URL (env-overridable) via config, written once per temp CWD.
    let config = dir.join("proef.toml");
    if !config.exists() {
        std::fs::write(&config, BASE_URL_CONFIG).unwrap();
    }
    let mut cmd = Command::cargo_bin("proef").unwrap();
    cmd.current_dir(dir)
        .env("NO_COLOR", "1")
        .env("PROEF_BASE_URL", &fixture.base_url)
        .env("PROEF_SECRET_APITOKEN", API_TOKEN);
    cmd
}

/// A fresh temp CWD carrying the project's real proef.toml — for tests that run
/// the shipped `tests/features` corpus, which needs its full `[url]` endpoint
/// catalog (not the minimal `BASE_URL_CONFIG`). `base = ${env:PROEF_BASE_URL:-…}`
/// bends to the fixture via `proef_in`'s env, so no rewrite is needed; pre-writing
/// it also makes `proef_in` keep it (it only writes when the file is absent).
fn corpus_cwd() -> tempfile::TempDir {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::copy(
        workspace_root().join("proef.toml"),
        cwd.path().join("proef.toml"),
    )
    .unwrap();
    cwd
}

/// Normalized view of an events.jsonl: scenario → (status, step-finished count).
fn normalize_events(events_path: &Path) -> BTreeMap<String, (String, usize)> {
    let text = std::fs::read_to_string(events_path).unwrap();
    let mut scenarios: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for line in text.lines() {
        let event: serde_json::Value = serde_json::from_str(line).unwrap();
        match event["event"].as_str().unwrap() {
            "step_finished" => {
                let scenario = event["scenario"].as_str().unwrap().to_owned();
                scenarios.entry(scenario).or_default().1 += 1;
            }
            "scenario_finished" => {
                let scenario = event["scenario"].as_str().unwrap().to_owned();
                scenarios.entry(scenario).or_default().0 =
                    event["status"].as_str().unwrap().to_owned();
            }
            _ => {}
        }
    }
    scenarios
}

fn latest_run_dir(cwd: &Path) -> PathBuf {
    let mut runs: Vec<PathBuf> = std::fs::read_dir(cwd.join(".proef-runs"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    runs.sort();
    runs.pop().unwrap()
}

/// US-1 + ADR-0010: the reference corpus runs green; the executed artifacts are
/// byte-identical to a separate emission with the same run id and env.
#[test]
fn reference_corpus_runs_green_with_same_bytes_artifacts() {
    let fixture = Fixture::start().unwrap();
    let cwd = corpus_cwd();
    let corpus = workspace_root().join("tests/features");

    let assert = proef_in(cwd.path(), &fixture)
        .args([
            "test",
            &corpus.display().to_string(),
            "--jobs",
            "4",
            "--output",
            "json",
        ])
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let json_line = stdout.lines().last().unwrap();
    let summary: serde_json::Value = serde_json::from_str(json_line).unwrap();
    assert_eq!(summary["passed"], 12, "{stdout}");
    assert_eq!(summary["failed"], 0, "{stdout}");
    let run_id = summary["run_id"].as_str().unwrap().to_owned();

    // The JSONL record is the event stream (ADR-0008).
    let run_dir = latest_run_dir(cwd.path());
    let events = normalize_events(&run_dir.join("events.jsonl"));
    assert_eq!(events.len(), 12, "{events:?}");
    assert!(
        events.values().all(|(status, _)| status == "passed"),
        "{events:?}"
    );

    // run.log mirrors the console record (§11).
    let log = std::fs::read_to_string(run_dir.join("run.log")).unwrap();
    assert!(log.contains("summary: 12 passed"), "{log}");

    // Attempt counts are the flake-proof axis (TESTING-STRATEGY §5): the
    // fixture makes items visible on the 2nd poll, so the retried step
    // reports exactly 2 attempts — and a multi-entry step never inflates
    // its count past its retries.
    let raw_events = std::fs::read_to_string(run_dir.join("events.jsonl")).unwrap();
    assert!(
        raw_events.contains("\"attempts\":2"),
        "the eventually-visible step retries exactly once: {raw_events}"
    );
    assert!(
        !raw_events.contains("\"attempts\":3"),
        "no step needs a third attempt against the fixture: {raw_events}"
    );
    // Live progress (ADR-0001): every entry attempt is on the record as it
    // starts, and the retried entry's second attempt is visible as retry 1.
    assert!(
        raw_events.contains("\"event\":\"entry_running\""),
        "live entry progress reaches the record: {raw_events}"
    );
    assert!(
        raw_events.contains("\"retry\":1"),
        "the retried entry's second attempt is on the record: {raw_events}"
    );
    assert!(
        !raw_events.contains("\"retry\":2"),
        "no entry needs a third attempt: {raw_events}"
    );

    // Same-bytes: run-dir artifacts == a fresh emission under the same run id.
    let emitted = cwd.path().join("emitted");
    proef_in(cwd.path(), &fixture)
        .args([
            "artifacts",
            &corpus.display().to_string(),
            "-o",
            &emitted.display().to_string(),
            "--run-id",
            &run_id,
        ])
        .assert()
        .code(0);
    let mut compared = 0;
    for entry in std::fs::read_dir(run_dir.join("artifacts"))
        .unwrap()
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".hurl") {
            let executed = std::fs::read(entry.path()).unwrap();
            let emitted_bytes = std::fs::read(emitted.join(&name)).unwrap();
            assert_eq!(
                executed, emitted_bytes,
                "artifact `{name}` must be the same bytes"
            );
            compared += 1;
        }
    }
    assert_eq!(compared, 12, "all executed artifacts compared");
}

/// `--output tap` emits a TAP v13 stream to stdout — one test point per
/// scenario, derived from the run's outcomes — while the human report moves to
/// stderr. The green corpus is a `1..12` plan of twelve passing points.
#[test]
fn output_tap_emits_a_tap_stream_for_the_run() {
    let fixture = Fixture::start().unwrap();
    let cwd = corpus_cwd();
    let corpus = workspace_root().join("tests/features");

    let assert = proef_in(cwd.path(), &fixture)
        .args([
            "test",
            &corpus.display().to_string(),
            "--jobs",
            "4",
            "--output",
            "tap",
        ])
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    // stdout is TAP only (the human report went to stderr).
    assert!(stdout.starts_with("TAP version 13\n1..12\n"), "{stdout}");
    let points = stdout
        .lines()
        .filter(|line| line.starts_with("ok "))
        .count();
    assert_eq!(points, 12, "twelve passing points: {stdout}");
    assert!(
        !stdout.contains("not ok"),
        "no failures in the green corpus: {stdout}"
    );
}

/// Failure UX (US-1/G6): a failing assert names the feature line and the
/// artifact span.
#[test]
fn failure_maps_to_feature_line_and_artifact_span() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n  Scenario: health asserts the wrong status\n    When the health endpoint is checked\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  health:\n    match: the health endpoint is checked\n    steps:\n      - hurl: |\n          GET ${url:base}/health\n          HTTP 500\n",
    )
    .unwrap();

    let junit_path = cwd.path().join("report.xml");
    let assert = proef_in(cwd.path(), &fixture)
        .args([
            "test",
            "suite",
            "--junit",
            &junit_path.display().to_string(),
        ])
        .assert()
        .code(1);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("case.feature:4"), "feature line: {stderr}");
    assert!(stderr.contains(".hurl:"), "artifact span: {stderr}");
    // The failed step carries a reproduce hint: the curl of the failing request.
    assert!(
        stderr.contains("curl: curl "),
        "curl reproduce hint: {stderr}"
    );
    // JUnit well-formedness (US-8): round-parse the XML instead of substring
    // checks — a consumer-shaped guarantee (well-formed, one testcase, one
    // failure element).
    let junit = std::fs::read_to_string(&junit_path).unwrap();
    assert!(junit.contains("failures=\"1\""), "{junit}");
    let mut reader = quick_xml::Reader::from_str(&junit);
    let (mut testcases, mut failures) = (0u32, 0u32);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(
                quick_xml::events::Event::Start(element) | quick_xml::events::Event::Empty(element),
            ) => match element.name().as_ref() {
                b"testcase" => testcases += 1,
                b"failure" => failures += 1,
                _ => {}
            },
            Ok(_) => {}
            Err(err) => panic!("JUnit XML must parse: {err}"),
        }
    }
    assert_eq!(testcases, 1, "{junit}");
    assert_eq!(failures, 1, "{junit}");
}

/// R3-6: test identity in JUnit is `classname` + `name` — Jenkins keys its
/// history on the pair and GitLab's MR widget diffs head against base by it.
/// The old single `name` embedded `file:line`, so an edit above a scenario
/// re-identified every test below it. `classname` now carries the file,
/// `name` the scenario alone; `file` rides as an attribute (GitLab links it);
/// `time` sits on suite and root (GitLab reads both and ignores the counts).
/// `timestamp`/`hostname` are deliberately absent — GitLab ignores both,
/// Jenkins substitutes its build clock and never reads hostname, and naming
/// the machine would undo R12-1.
#[test]
fn junit_identity_is_classname_plus_stable_name() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n  Scenario: the health check passes\n    When the health endpoint is checked\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  health:\n    match: the health endpoint is checked\n    steps:\n      - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    let junit_path = cwd.path().join("report.xml");
    proef_in(cwd.path(), &fixture)
        .args([
            "test",
            "suite",
            "--junit",
            &junit_path.display().to_string(),
        ])
        .assert()
        .code(0);
    let junit = std::fs::read_to_string(&junit_path).unwrap();
    assert!(
        junit.contains(r#"classname="suite/case.feature""#),
        "classname carries the file: {junit}"
    );
    assert!(
        junit.contains(r#"name="the health check passes""#),
        "name is the scenario alone — no file:line, so identity survives edits: {junit}"
    );
    assert!(
        junit.contains(r#"file="suite/case.feature""#),
        "GitLab reads a file attribute for source linking: {junit}"
    );
    assert!(
        junit
            .lines()
            .any(|l| l.contains("<testsuite ") && l.contains(" time=")),
        "suite time is present (GitLab reads it): {junit}"
    );
    assert!(
        !junit.contains("hostname") && !junit.contains("timestamp"),
        "no machine name (R12-1) and no wall clock: {junit}"
    );
}

/// `--run-id` pins the injected run id on the run path — the JSON summary echoes
/// it. Because `${fake:…}` keys on the run id, re-running with the same id
/// reproduces the same fake data (the determinism itself is proven by the
/// byte-identical artifact-corpus test; here we assert the flag is honored).
#[test]
fn pinned_run_id_is_honored() {
    let fixture = Fixture::start().unwrap();
    let cwd = corpus_cwd();
    let corpus = workspace_root().join("tests/features");
    let assert = proef_in(cwd.path(), &fixture)
        .args([
            "test",
            &corpus.display().to_string(),
            "--tags",
            "breadth",
            "--run-id",
            "pinned-seed-001",
            "--output",
            "json",
        ])
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let json: serde_json::Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(json["run_id"], "pinned-seed-001", "{stdout}");
}

/// `--rerun` re-runs only the scenarios that failed in the prior run: a mixed
/// pass/fail suite runs both once, then `--rerun` runs just the failure.
#[test]
fn rerun_reruns_only_the_prior_failures() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: passing\n    When health is checked\n  \
         Scenario: failing\n    When health is wrongly expected to 500\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  ok:\n    match: health is checked\n    steps:\n      - hurl: |\n          \
         GET ${url:base}/health\n          HTTP 200\n  bad:\n    match: health is wrongly \
         expected to 500\n    steps:\n      - hurl: |\n          GET ${url:base}/health\n          \
         HTTP 500\n",
    )
    .unwrap();

    // Run 1: one scenario passes, one fails → exit 1.
    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(1);
    // --rerun: only the prior failure runs (the passing scenario is not re-run).
    let assert = proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--rerun", "--output", "json"])
        .assert()
        .code(1);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let json: serde_json::Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(json["failed"], 1, "only the failure reruns: {stdout}");
    assert_eq!(
        json["passed"], 0,
        "the passing scenario is not rerun: {stdout}"
    );
    assert_eq!(json["skipped"], 0, "{stdout}");
}

/// `@quarantine`: a tagged scenario's test-failure does not gate the run (exit
/// 0) — the same failure without the tag gates normally (exit 1).
#[test]
fn quarantine_tag_does_not_gate_the_exit_code() {
    let fixture = Fixture::start().unwrap();
    let pack = "macros:\n  bad:\n    match: health is wrongly expected to 500\n    steps:\n      \
                - hurl: |\n          GET ${url:base}/health\n          HTTP 500\n";
    let make = |tag: &str| {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("suite/packs")).unwrap();
        std::fs::write(
            dir.path().join("suite/case.feature"),
            format!(
                "Feature: F\n  {tag}\n  Scenario: flaky\n    When health is wrongly expected to 500\n"
            ),
        )
        .unwrap();
        std::fs::write(dir.path().join("suite/packs/p.yaml"), pack).unwrap();
        dir
    };
    // Fails (asserts 500, fixture returns 200) but is quarantined → exit 0.
    let quarantined = make("@quarantine");
    proef_in(quarantined.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(0);
    // The same failure without the tag gates normally → exit 1.
    let normal = make("@normal");
    proef_in(normal.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(1);
}

/// `proef diff` compares two run records by `(file, scenario)` identity: a
/// scenario that passed then failed is a `regressed` transition that
/// `--fail-on-regression` turns into exit 1; the reverse is a `fixed`
/// transition, and without the flag diff is informational (exit 0).
#[test]
fn diff_reports_regressions_and_fixes_between_runs() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: health\n    When health is checked\n",
    )
    .unwrap();
    let pack = |code: u16| {
        format!(
            "macros:\n  ok:\n    match: health is checked\n    steps:\n      - hurl: |\n          \
             GET ${{url:base}}/health\n          HTTP {code}\n"
        )
    };
    let pack_path = cwd.path().join("suite/packs/p.yaml");

    // Run "base": expects 200 (the fixture returns 200) → passes.
    std::fs::write(&pack_path, pack(200)).unwrap();
    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--run-id", "run-base"])
        .assert()
        .code(0);
    // Run "new": expects 500 → fails.
    std::fs::write(&pack_path, pack(500)).unwrap();
    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--run-id", "run-new"])
        .assert()
        .code(1);

    // base → new is a regression; --fail-on-regression gates it to exit 1.
    let assert = proef_in(cwd.path(), &fixture)
        .args(["diff", "run-base", "run-new", "--fail-on-regression"])
        .assert()
        .code(1);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.contains("1 regressed"), "{stdout}");
    assert!(stdout.contains("case.feature :: health"), "{stdout}");

    // The reverse is a fix, and without the flag diff stays informational (0).
    let assert = proef_in(cwd.path(), &fixture)
        .args(["diff", "run-new", "run-base"])
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.contains("1 fixed"), "{stdout}");

    // The CI-baseline flow (R3-4): the base is a *downloaded file*, not a run
    // in this checkout's runs-dir — outside the project, under a name the
    // download step chose. The JSONL stream is the record (ADR-0008), so the
    // file must mean exactly what the run id means. Before this, every
    // argument was joined onto runs-dir, so a path answered with
    // `.proef-runs/<path>/events.jsonl: No such file` — the argument mangled
    // into the complaint.
    let elsewhere = tempfile::tempdir().unwrap();
    let baseline = elsewhere.path().join("main-branch-baseline.jsonl");
    std::fs::copy(
        cwd.path().join(".proef-runs/run-base/events.jsonl"),
        &baseline,
    )
    .unwrap();
    let assert = proef_in(cwd.path(), &fixture)
        .args([
            "diff",
            baseline.to_str().unwrap(),
            "run-new",
            "--fail-on-regression",
        ])
        .assert()
        .code(1); // same regression, certified from the downloaded baseline
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.contains("1 regressed"), "{stdout}");
    assert!(
        stdout.contains("main-branch-baseline.jsonl"),
        "the header must name the file the caller passed: {stdout}"
    );

    // A record *directory* works the same way — the other spelling a script
    // has in hand after an artifact download preserves the layout.
    proef_in(cwd.path(), &fixture)
        .args([
            "diff",
            cwd.path().join(".proef-runs/run-base").to_str().unwrap(),
            "run-new",
        ])
        .assert()
        .code(0);

    // A path that does not exist names itself — never a runs-dir join of it.
    let assert = proef_in(cwd.path(), &fixture)
        .args(["diff", "no/such/baseline.jsonl", "run-new"])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains("`no/such/baseline.jsonl` does not exist"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(".proef-runs/no/such"),
        "the complaint must not mangle the path into runs-dir: {stderr}"
    );
}

/// `proef report` writes a self-contained HTML file into the run dir whose
/// `artifacts/` deep-links resolve to real files — proving the report's slug
/// derivation matches the emitter's on-disk artifact name.
#[test]
fn report_writes_self_contained_html_linking_real_artifacts() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: health\n    When health is checked\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  ok:\n    match: health is checked\n    steps:\n      - hurl: |\n          \
         GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--run-id", "run-rep"])
        .assert()
        .code(0);

    let assert = proef_in(cwd.path(), &fixture)
        .args(["report", "run-rep"])
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.contains("report.html"), "{stdout}");

    let run_dir = cwd.path().join(".proef-runs/run-rep");
    let html = std::fs::read_to_string(run_dir.join("report.html")).unwrap();
    assert!(html.starts_with("<!DOCTYPE html>"), "a standalone document");
    assert!(
        !html.contains("http://"),
        "self-contained, no external refs: {html}"
    );
    // Extract the (only) href — the artifact deep-link — and stat the target.
    let href = html
        .split("href=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("an artifact link");
    assert!(
        href.starts_with("artifacts/") && href.ends_with(".hurl"),
        "artifact link shape: {href}"
    );
    assert!(
        run_dir.join(href).exists(),
        "the report deep-links a real artifact: {href}"
    );
}

/// A step that passes only after a retry records its earlier failed attempt(s)
/// as `attempt_details` on the event stream, and JUnit surfaces them
/// as `<flakyFailure>` — the flaky pass is honest, not masked as a clean pass.
#[test]
fn flaky_pass_records_attempt_details_and_junit_flaky_failure() {
    let fixture = Fixture::start().unwrap();
    let cwd = corpus_cwd();
    let corpus = workspace_root().join("tests/features");
    let junit = cwd.path().join("out.junit.xml");
    // The corpus's eventually-visible search step retries once (attempts:2).
    proef_in(cwd.path(), &fixture)
        .args([
            "test",
            &corpus.display().to_string(),
            "--jobs",
            "4",
            "--junit",
            &junit.display().to_string(),
        ])
        .assert()
        .code(0);

    // The event stream carries the earlier-attempt failure detail.
    let run_dir = latest_run_dir(cwd.path());
    let events = std::fs::read_to_string(run_dir.join("events.jsonl")).unwrap();
    assert!(
        events.contains("attempt_details"),
        "the flaky step records its earlier-attempt detail: {events}"
    );
    // JUnit renders it as a <flakyFailure> under the passing test case.
    let xml = std::fs::read_to_string(&junit).unwrap();
    assert!(xml.contains("<flakyFailure"), "{xml}");
}

/// US-5: `optional:` failures warn and the scenario continues to green.
#[test]
fn optional_failure_warns_and_continues() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n  Scenario: optional probe fails but the run continues\n    When the flaky probe runs\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  probe:\n    match: the flaky probe runs\n    steps:\n      - name: doomed but optional\n        optional: true\n        hurl: |\n          GET ${url:base}/api/v1/admin/search/missing\n          HTTP 200\n      - name: the real check\n        hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(0);
}

/// ADR-0014: a failing suite-level setup aborts the run *before* the pool, as a
/// user error (exit 2, never a test failure); the suite never runs.
#[test]
fn setup_failure_aborts_the_run_as_a_user_error() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\nsetup = \"suite/setup.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/setup.feature"),
        "Feature: S\n  Scenario: broken setup\n    When setup probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: should not run\n    When the suite probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  setupProbe:\n    match: setup probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 500\n  \
         suiteProbe:\n    match: the suite probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    let assert = proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(2); // user error — a broken fixture is not a failing test (exit 1)
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("setup failed"), "{stderr}");
    let events = std::fs::read_to_string(latest_run_dir(cwd.path()).join("events.jsonl")).unwrap();
    assert!(events.contains("broken setup"), "setup ran: {events}");
    assert!(
        !events.contains("should not run"),
        "suite must not run: {events}"
    );
}

/// R12-3: the setup abort used to return before the CI-report block, so a job
/// gating on `--junit` saw *no file at all* — indistinguishable from proef
/// never running. The abort now writes the reports from the setup phase's own
/// summary: one testcase, failed, named by the setup feature. The exit code is
/// untouched — ADR-0014 decided 2, and the report is where the detail belongs.
#[test]
fn a_failed_setup_still_writes_the_junit_report() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\nsetup = \"suite/setup.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/setup.feature"),
        "Feature: S\n  Scenario: broken setup\n    When setup probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: should not run\n    When the suite probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  setupProbe:\n    match: setup probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 500\n  \
         suiteProbe:\n    match: the suite probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--junit", "report.xml"])
        .assert()
        .code(2);
    let junit = std::fs::read_to_string(cwd.path().join("report.xml")).unwrap();
    assert!(
        junit.contains(r#"failures="1""#),
        "the setup failure is a counted failure: {junit}"
    );
    assert!(
        junit.contains("setup.feature"),
        "the suite is named by the setup feature, so a reader knows which phase died: {junit}"
    );
    assert!(
        junit.contains("broken setup"),
        "the scenario is named: {junit}"
    );
    assert!(
        !junit.contains("should not run"),
        "nothing is fabricated for a pool that never ran: {junit}"
    );
}

/// The machine-body contract has no exceptions: a setup that fails to even
/// LOAD (here: the configured file does not exist) is still a terminating
/// path, and this arm was the last one returning zero stdout bytes under
/// `--output json`.
#[test]
fn a_setup_that_fails_to_load_still_emits_the_machine_body() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\nsetup = \"suite/missing.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: pool\n    When the suite probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  suiteProbe:\n    match: the suite probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    let assert = proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--output", "json"])
        .assert()
        .code(2);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let body: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|err| panic!("stdout must be one JSON body ({err}): [{stdout}]"));
    assert_eq!(body["exit_code"], 2, "{body}");
}

/// R17-2.4: the setup abort writes the CI files (#78) **and** the machine
/// body — `--output json` used to read zero bytes on this path, so four sinks
/// told three stories. Totals are suite-only zeros (ADR-0014); the exit code
/// carries the verdict.
#[test]
fn a_failed_setup_still_emits_the_machine_body() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\nsetup = \"suite/setup.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/setup.feature"),
        "Feature: S\n  Scenario: broken setup\n    When setup probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: should not run\n    When the suite probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  setupProbe:\n    match: setup probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 500\n  \
         suiteProbe:\n    match: the suite probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    let assert = proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--output", "json"])
        .assert()
        .code(2);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let body: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|err| panic!("stdout must be one JSON body ({err}): [{stdout}]"));
    assert_eq!(body["exit_code"], 2, "{body}");
    assert_eq!(
        body["failed"], 0,
        "suite totals stay ADR-0014 zeros: {body}"
    );
}

/// R17-2.5: a failed teardown appears in JUnit as its own suite — #78's rule
/// made symmetric (a phase appears in the reports when it fails). A gated
/// pipeline used to read a fully-passing report on an exit-3 run.
#[test]
fn a_failed_teardown_reaches_junit_as_its_own_suite() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\nteardown = \"suite/teardown.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/teardown.feature"),
        "Feature: T\n  Scenario: broken teardown\n    When teardown probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: main case\n    When the suite probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  teardownProbe:\n    match: teardown probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 500\n  \
         suiteProbe:\n    match: the suite probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--junit", "report.xml"])
        .assert()
        .code(3);
    let junit = std::fs::read_to_string(cwd.path().join("report.xml")).unwrap();
    assert!(
        junit.contains("teardown.feature") && junit.contains("broken teardown"),
        "the failed phase is its own suite: {junit}"
    );
    assert!(
        junit.contains("main case"),
        "the pool suite is still there: {junit}"
    );
}

/// ADR-0014: setup runs once before the pool and its `saveAs: global` reaches
/// the pool (the suite fetching `${global:recordId}` only succeeds if setup's
/// promotion merged first); teardown runs after; both are excluded from the
/// pool, so each runs exactly once.
#[test]
fn setup_shares_globals_teardown_runs_and_both_are_excluded() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\n\
         setup = \"suite/setup.feature\"\nteardown = \"suite/teardown.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/setup.feature"),
        "Feature: S\n  Scenario: provision\n    When the record is remembered\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: use it\n    When the remembered record is fetched\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/teardown.feature"),
        "Feature: T\n  Scenario: cleanup\n    When teardown probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  remember:\n    match: the record is remembered\n    steps:\n      \
         - saveAs: { recordId: global }\n        hurl: |\n          \
         GET ${url:base}/api/v1/admin/search/records\n          \
         Authorization: Bearer ${secret:apiToken}\n          HTTP 200\n          \
         [Captures]\n          recordId: jsonpath \"$[0].id\"\n  \
         fetch:\n    match: the remembered record is fetched\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/api/v1/records/${global:recordId}\n          \
         Authorization: Bearer ${secret:apiToken}\n          HTTP 200\n  \
         teardownProbe:\n    match: teardown probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--jobs", "1"])
        .assert()
        .code(0); // exit 0 proves setup's global reached the pool
    let events = std::fs::read_to_string(latest_run_dir(cwd.path()).join("events.jsonl")).unwrap();
    // Each phase's scenario ran exactly once — setup/teardown excluded from the pool.
    assert_eq!(
        events
            .matches(r#""event":"scenario_started","scenario":"provision""#)
            .count(),
        1,
        "setup must not also run in the pool: {events}"
    );
    assert!(events.contains(r#""scenario":"use it""#), "{events}");
    assert!(
        events.contains(r#""scenario":"cleanup""#),
        "teardown ran: {events}"
    );
}

/// A bare-filename `[run] setup` at the project root (no directory prefix) must
/// resolve its packs from the cwd, not from an empty derived base: `Path::parent()`
/// returns `Some("")` for a bare filename, and pre-fix that empty path was handed
/// to `read_dir` directly, aborting the run with "cannot read directory ".
#[test]
fn bare_filename_setup_at_project_root_resolves_packs() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\nsuite = \"suite\"\nsetup = \"setup.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: use it\n    When the suite probes health\n",
    )
    .unwrap();
    // Setup feature at the PROJECT ROOT — a bare filename, no directory prefix.
    std::fs::write(
        cwd.path().join("setup.feature"),
        "Feature: S\n  Scenario: provision\n    When setup probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  setupProbe:\n    match: setup probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n  \
         suiteProbe:\n    match: the suite probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test"])
        .assert()
        .code(0); // exit 0 proves the bare-filename setup's packs resolved from cwd
}

/// ADR-0014: a teardown failure is a distinct non-zero signal (exit 3, a
/// cleanup fault) — the suite's own green verdict still stands. This also
/// pins a console/JUnit disagreement that once existed here: with teardown's
/// failing scenario folded into `RunRecord`'s totals, the console `summary:`
/// line read "2 passed · 1 failed" (setup-less here, so suite (1 passed) +
/// teardown (1 failed)) while JUnit/`--output json`/TAP — which read the
/// suite's own `RunSummary` directly — said "1 passed, 0 failed". Suite-only
/// totals make the console line agree with those surfaces again.
/// A green suite (one passing scenario) with a `[run] teardown` that fails —
/// the shape that makes a phase-only failure visible: the suite's own verdict
/// is all-pass, so `RunFinished`'s suite-only totals (ADR-0014) legitimately
/// read `0 failed` while the teardown scenario itself still shows up Failed
/// in the record.
fn write_teardown_only_suite(cwd: &Path) {
    std::fs::create_dir_all(cwd.join("suite/packs")).unwrap();
    std::fs::write(
        cwd.join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\nteardown = \"suite/teardown.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.join("suite/teardown.feature"),
        "Feature: T\n  Scenario: broken cleanup\n    When teardown probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.join("suite/case.feature"),
        "Feature: F\n  Scenario: passes\n    When the suite probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.join("suite/packs/p.yaml"),
        "macros:\n  teardownProbe:\n    match: teardown probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 500\n  \
         suiteProbe:\n    match: the suite probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();
}

#[test]
fn teardown_failure_is_a_distinct_cleanup_fault() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    write_teardown_only_suite(cwd.path());

    let assert = proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(3); // cleanup fault, distinct from a test failure (1) or green (0)
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("teardown failed"), "{stderr}");
    assert!(
        stdout.contains("summary: 1 passed · 0 failed · 0 skipped"),
        "console summary must be the suite's own verdict, not blended with \
         teardown's failure: {stdout}"
    );
    let events = std::fs::read_to_string(latest_run_dir(cwd.path()).join("events.jsonl")).unwrap();
    assert!(
        events.contains(r#""scenario":"passes""#),
        "the suite itself ran: {events}"
    );
    assert!(
        events.contains(r#""event":"run_finished","passed":1,"failed":0,"skipped":0"#),
        "run_finished must be the suite's totals only, not folding in teardown's failure: {events}"
    );
}

/// `explain` must still name and detail a phase-only failure even though the
/// suite-only headline (ADR-0014) legitimately reads `0 failed` — a green
/// suite with a broken teardown is exactly the case a post-mortem tool exists
/// to surface, and the process exits non-zero for it (exit 3).
#[test]
fn explain_details_a_green_suite_with_a_failing_teardown() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    write_teardown_only_suite(cwd.path());

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(3);

    let assert = proef_in(cwd.path(), &fixture)
        .arg("explain")
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        out.contains("1 passed · 0 failed · 0 skipped"),
        "headline stays the suite's own (all-pass) verdict: {out}"
    );
    assert!(
        out.contains("broken cleanup"),
        "the failing teardown scenario must be named even though the \
         headline reads 0 failed: {out}"
    );
    assert!(
        out.contains("artifacts:"),
        "the failure detail block (artifact/log path) must still print: {out}"
    );
}

/// The setup-fails counterpart: the main suite never runs at all (ADR-0014 —
/// a broken setup aborts before the pool starts), so the headline is
/// `0 passed · 0 failed · 0 skipped`, yet `explain` must still name the
/// failing setup scenario instead of printing nothing (exit 2).
#[test]
fn explain_details_a_failing_setup() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\nsetup = \"suite/setup.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/setup.feature"),
        "Feature: S\n  Scenario: broken setup\n    When setup probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: should not run\n    When the suite probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  setupProbe:\n    match: setup probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 500\n  \
         suiteProbe:\n    match: the suite probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(2);

    let assert = proef_in(cwd.path(), &fixture)
        .arg("explain")
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        out.contains("0 passed · 0 failed · 0 skipped"),
        "headline is empty — the suite never ran: {out}"
    );
    assert!(
        out.contains("broken setup"),
        "the failing setup scenario must be named even though the headline \
         reads 0 failed: {out}"
    );
    assert!(
        out.contains("artifacts:"),
        "the failure detail block (artifact/log path) must still print: {out}"
    );
}

/// `proef report`'s HTML headline must agree with `explain` and the record's
/// own `run_finished` totals — a green suite with a failing teardown must not
/// read as `1 passed · 1 failed` in the HTML while every other surface says
/// `1 passed · 0 failed`.
#[test]
fn report_headline_matches_explain_on_a_failing_teardown() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    write_teardown_only_suite(cwd.path());

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(3);

    let out_html = cwd.path().join("report.html");
    proef_in(cwd.path(), &fixture)
        .args(["report", "-o", &out_html.display().to_string()])
        .assert()
        .code(0);
    let html = std::fs::read_to_string(&out_html).unwrap();
    assert!(
        html.contains(r#"<span class="count pass">1 passed</span>"#),
        "HTML headline must report the suite's own passed count: {html}"
    );
    assert!(
        html.contains(r#"<span class="count fail">0 failed</span>"#),
        "HTML headline must agree with explain/run_finished (0 failed), not \
         count the failing teardown block into the headline: {html}"
    );
    // The teardown scenario's own block must still render — dropping it
    // would lose real information — just not be folded into the headline.
    assert!(
        html.contains("broken cleanup"),
        "the failing teardown scenario's block must still render: {html}"
    );
}

/// ADR-0014: `[run] setup`/`teardown` names exactly one feature file. A
/// directory would run every feature under it as the phase *and* leave them
/// in the pool (`exclude_phase_features` matches a single file path), running
/// each scenario twice — reject it loudly instead.
///
/// This exercises real `test` execution, not `--dry-run`: `--dry-run` routes
/// to `commands::dry_run`, which validates only the suite path and never
/// looks at `[run] setup`/`teardown` at all — `run_phase` (where the guard
/// lives) is only reachable from a real run. The guard fires before
/// `run_phase` calls `front::run` or dispatches any batch, so nothing here
/// ever hits the network — no fixture server needed.
#[test]
fn directory_valued_setup_is_rejected_not_double_run() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // A suite with one ordinary feature.
    std::fs::create_dir_all(root.join("suite")).unwrap();
    std::fs::write(
        root.join("suite/main.feature"),
        "Feature: M\n  Scenario: S\n    When I noop\n",
    )
    .unwrap();
    // A DIRECTORY of setup features (the misconfiguration).
    std::fs::create_dir_all(root.join("setup")).unwrap();
    std::fs::write(
        root.join("setup/a.feature"),
        "Feature: A\n  Scenario: SA\n    When I noop\n",
    )
    .unwrap();
    // Minimal pack so `I noop` binds (mirror execute.rs's existing fixture packs).
    std::fs::create_dir_all(root.join("suite/packs")).unwrap();
    std::fs::write(
        root.join("suite/packs/p.yaml"),
        "macros:\n  noop:\n    match: \"I noop\"\n    steps:\n      - hurl: |\n          GET http://x\n",
    )
    .unwrap();
    std::fs::write(
        root.join("proef.toml"),
        "[run]\nsuite = \"suite\"\nsetup = \"setup\"\n",
    )
    .unwrap();

    Command::cargo_bin("proef")
        .unwrap()
        .current_dir(root)
        .env("NO_COLOR", "1")
        .args(["test"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "[run] setup must be a feature file, not a directory",
        ));
}

/// Companion to `directory_valued_setup_is_rejected_not_double_run`: a
/// single-FILE setup must still run once and be excluded from the pool — the
/// guard fires only for directories, not the good path. Real execution
/// (exit 0 proves the request actually went through), mirroring
/// `setup_shares_globals_teardown_runs_and_both_are_excluded` (the setup file
/// lives inside `suite/`, like that test — pack discovery walks down from a
/// phase file's parent directory, so a phase file needs to share a subtree
/// with its pack).
#[test]
fn single_file_setup_still_runs_once() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\nsuite = \"suite\"\nsetup = \"suite/setup.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: passes\n    When the suite probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/setup.feature"),
        "Feature: S\n  Scenario: provision\n    When the suite probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  suiteProbe:\n    match: the suite probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test"])
        .assert()
        .code(0);
    let events = std::fs::read_to_string(latest_run_dir(cwd.path()).join("events.jsonl")).unwrap();
    assert_eq!(
        events
            .matches(r#""event":"scenario_started","scenario":"provision""#)
            .count(),
        1,
        "setup must run exactly once, and not also in the pool: {events}"
    );
    assert!(events.contains(r#""scenario":"passes""#), "{events}");
}

/// US-4: `saveAs: global` persists across scenarios and lands in
/// `.proef-state.json` (atomic World persistence).
#[test]
fn world_chains_globals_across_scenarios_and_runs() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n  Scenario: a — capture the record id\n    When the record is resolved\n  Scenario: b — reuse the remembered id\n    When the remembered record is fetched\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  resolve:\n    match: the record is resolved\n    steps:\n      - saveAs: { recordId: global }\n        hurl: |\n          GET ${url:base}/api/v1/admin/search/records\n          Authorization: Bearer ${secret:apiToken}\n          HTTP 200\n          [Captures]\n          recordId: jsonpath \"$[0].id\"\n  fetch:\n    match: the remembered record is fetched\n    steps:\n      - hurl: |\n          GET ${url:base}/api/v1/records/${global:recordId}\n          Authorization: Bearer ${secret:apiToken}\n          HTTP 200\n",
    )
    .unwrap();

    // --jobs 1: scenario b lowers after a's promotion merged back.
    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--jobs", "1"])
        .assert()
        .code(0);
    let state = std::fs::read_to_string(cwd.path().join(".proef-state.json")).unwrap();
    assert!(state.contains("\"recordId\": \"r-1\""), "{state}");

    // ADR-0010 executed-bytes: the run-dir artifact holds the *strict*
    // lower-time resolution — scenario b's URL carries the promoted value.
    // A DryRun emission (`proef artifacts`) of the same corpus cannot know
    // it, so the two are observably different texts; emission-vs-emission
    // equality alone could never prove which bytes the engine executed.
    let run_dir = latest_run_dir(cwd.path());
    let executed: String = std::fs::read_dir(run_dir.join("artifacts"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "hurl"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .collect();
    assert!(
        executed.contains("/api/v1/records/r-1"),
        "executed artifact must carry the strict-resolved global: {executed}"
    );
    // Without the persisted state a dry-run emission cannot know the value
    // (with it, lower-time World reads legitimately resolve in any mode).
    std::fs::remove_file(cwd.path().join(".proef-state.json")).unwrap();
    proef_in(cwd.path(), &fixture)
        .args(["artifacts", "suite", "-o", "emitted", "--run-id", "pinned"])
        .assert()
        .code(0);
    let emitted: String = std::fs::read_dir(cwd.path().join("emitted"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "hurl"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .collect();
    assert!(
        !emitted.contains("r-1"),
        "a dry-run emission cannot know run-time globals: {emitted}"
    );
}

/// TECH-SPEC §5 SessionState: cookies round-trip across a forced batch split
/// (an optional step segments the scenario into three batches).
#[test]
fn cookies_survive_batch_splits() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n  Scenario: session cookie crosses the split\n    When the cookie session is exercised\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  cookies:\n    match: the cookie session is exercised\n    steps:\n      - name: obtain the session cookie\n        hurl: |\n          GET ${url:base}/cookie/set\n          HTTP 200\n      - name: unrelated optional probe (forces a batch split)\n        optional: true\n        hurl: |\n          GET ${url:base}/health\n          HTTP 200\n      - name: cookie must still be sent after the split\n        hurl: |\n          GET ${url:base}/cookie/check\n          HTTP 200\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(0);
}

/// ADR-0007 (bounded runtime): a hanging backend is cut off by the clamped
/// per-request timeout — the run ends promptly with a system fault, never
/// hanging on `/slow`.
#[test]
fn runaway_scenarios_are_bounded() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        format!("{BASE_URL_CONFIG}[http]\ntimeout-ms = 500\n"),
    )
    .unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n  Scenario: the slow endpoint cannot stall the run\n    When the slow endpoint is called\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  slow:\n    match: the slow endpoint is called\n    steps:\n      - hurl: |\n          GET ${url:base}/slow\n          HTTP 200\n",
    )
    .unwrap();

    let started = Instant::now();
    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(3);
    assert!(
        started.elapsed().as_secs() < 15,
        "bounded: took {:?}",
        started.elapsed()
    );
}

/// The reference event stream is snapshot-locked (TESTING-STRATEGY: event
/// streams complete the insta matrix; injected run-id and engine-measured
/// durations are filtered to stable placeholders).
#[test]
fn event_stream_snapshot_reference_run() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n  Scenario: reference\n    When the cookie session is exercised\n    Then the response status is 200\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  cookies:\n    match: the cookie session is exercised\n    steps:\n      - name: obtain the session cookie\n        hurl: |\n          GET ${url:base}/cookie/set\n          HTTP 200\n      - name: optional probe (forces a split)\n        optional: true\n        hurl: |\n          GET ${url:base}/health\n          HTTP 200\n      - name: cookie survives the split\n        hurl: |\n          GET ${url:base}/cookie/check\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--jobs", "1"])
        .assert()
        .code(0);
    let events = std::fs::read_to_string(latest_run_dir(cwd.path()).join("events.jsonl")).unwrap();
    insta::with_settings!({filters => vec![
        (r#""run_id":"[0-9a-f-]+""#, r#""run_id":"[run-id]""#),
        (r#""duration_ms":\d+"#, r#""duration_ms":0"#),
        // Injected run-relative timing (ADR-0015) is non-deterministic; the
        // worker index is deterministic under `--jobs 1` (a single worker → 0).
        (r#""timestamp_ms":\d+"#, r#""timestamp_ms":0"#),
    ]}, {
        insta::assert_snapshot!("reference_event_stream", events);
    });
}

/// US-10: values from the encrypted store drive a run (no env override), and
/// never appear in artifacts or records.
#[test]
fn encrypted_secret_store_drives_a_run() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let config_dir = cwd.path().join("config");
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n  Scenario: store secret authenticates\n    When the secured search runs\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  secured:\n    match: the secured search runs\n    steps:\n      - hurl: |\n          GET ${url:base}/api/v1/admin/search/records\n          Authorization: Bearer ${secret:apiToken}\n          HTTP 200\n",
    )
    .unwrap();
    std::fs::write(cwd.path().join("proef.toml"), BASE_URL_CONFIG).unwrap();

    // `secret set` + `list` (US-10), key auto-created under PROEF_CONFIG_DIR.
    let mut set = Command::cargo_bin("proef").unwrap();
    set.current_dir(cwd.path())
        .env("PROEF_CONFIG_DIR", &config_dir)
        .args(["secret", "set", "apiToken", "--stdin"])
        .write_stdin(API_TOKEN)
        .assert()
        .code(0);
    let mut list = Command::cargo_bin("proef").unwrap();
    let list_out = list
        .current_dir(cwd.path())
        .env("PROEF_CONFIG_DIR", &config_dir)
        .args(["secret", "list"])
        .assert()
        .code(0);
    let names = String::from_utf8_lossy(&list_out.get_output().stdout).into_owned();
    assert!(names.contains("apiToken"), "{names}");
    assert!(
        !names.contains(API_TOKEN),
        "list never prints values: {names}"
    );
    let store_text = std::fs::read_to_string(cwd.path().join(".proef-secrets.json")).unwrap();
    assert!(!store_text.contains(API_TOKEN), "store is ciphertext only");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let key = config_dir.join("proef").join("keys").join("default.key");
        let mode = std::fs::metadata(&key).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "key file is private from creation");
    }

    // Execute WITHOUT the env override: the store must supply the value.
    let mut run = Command::cargo_bin("proef").unwrap();
    run.current_dir(cwd.path())
        .env("NO_COLOR", "1")
        .env("PROEF_CONFIG_DIR", &config_dir)
        .env("PROEF_BASE_URL", &fixture.base_url)
        .env_remove("PROEF_SECRET_APITOKEN")
        .args(["test", "suite"])
        .assert()
        .code(0);

    // The secret value appears in no artifact and no record (ADR-0005).
    let run_dir = latest_run_dir(cwd.path());
    for entry in std::fs::read_dir(run_dir.join("artifacts"))
        .unwrap()
        .flatten()
    {
        let content = std::fs::read_to_string(entry.path()).unwrap_or_default();
        assert!(
            !content.contains(API_TOKEN),
            "secret leaked into {:?}",
            entry.path()
        );
    }
    let log = std::fs::read_to_string(run_dir.join("run.log")).unwrap_or_default();
    assert!(!log.contains(API_TOKEN), "secret leaked into run.log");
    // The JSONL record goes through the same sink-boundary redaction: scan it
    // too — the invariant covers *every* sink, not just the console.
    let events = std::fs::read_to_string(run_dir.join("events.jsonl")).unwrap();
    assert!(!events.is_empty(), "events.jsonl must not be empty");
    assert!(
        !events.contains(API_TOKEN),
        "secret leaked into events.jsonl"
    );
}

/// `--max-fail N` stops the run after N suite failures, honestly (R3-1).
///
/// The semantics every runner in the field shares — Playwright
/// `--max-failures`, pytest `--maxfail`, nextest `--max-fail`: stop at the
/// threshold, and the tests that never ran report as *not run*, never as
/// passed. proef rides its graceful-cancel path, so the record is a complete
/// **cancelled** run — which `diff --fail-on-regression` already refuses to
/// certify, exactly right for a deliberately-partial run.
#[test]
fn max_fail_stops_after_the_threshold_and_records_the_rest_as_skipped() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    // Three scenarios that all fail (the fixture answers 200; the pack asserts
    // 500). With --jobs 1 they run in file order, so --max-fail 1 must stop
    // after the first: `ScenarioFinished` is emitted from the dispatcher
    // thread, so the cancel lands before the next scenario is scheduled.
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: first\n    When health is mischecked\n  \
         Scenario: second\n    When health is mischecked\n  \
         Scenario: third\n    When health is mischecked\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  bad:\n    match: health is mischecked\n    steps:\n      - hurl: |\n          \
         GET ${url:base}/health\n          HTTP 500\n",
    )
    .unwrap();

    let assert = proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--jobs", "1", "--max-fail", "1"])
        .assert()
        .code(1); // a test failure is still a test failure
    let rendered = String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
        + &String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        rendered.contains("--max-fail"),
        "the stop must say why: {rendered}"
    );

    let events = std::fs::read_to_string(latest_run_dir(cwd.path()).join("events.jsonl")).unwrap();
    // One failure, two never-run scenarios recorded as skipped — not absent,
    // and never passed. The record is a complete cancelled run.
    assert!(
        events.contains(r#""failed":1"#) && events.contains(r#""skipped":2"#),
        "totals must show 1 failed / 2 skipped: {events}"
    );
    assert!(
        events.contains(r#""cancelled":true"#),
        "a max-fail stop is a cancelled run — diff must refuse to certify it: {events}"
    );

    // Without the flag, all three run and fail: the wrapper is pass-through
    // when unset, so this pins that the flag *caused* the early stop above.
    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--jobs", "1"])
        .assert()
        .code(1);
    let events = std::fs::read_to_string(latest_run_dir(cwd.path()).join("events.jsonl")).unwrap();
    assert!(
        events.contains(r#""failed":3"#),
        "without --max-fail all three must run: {events}"
    );
}

/// `--rerun` after a cancelled run continues it — failures *and* the tail the
/// run never reached (round-15 P2).
///
/// Without the union this was a silent false green: stop at the threshold,
/// fix, rerun → only the old failures ran, `exit 0`, most of the suite never
/// executed in either run. Stop → fix → continue is the workflow fail-fast
/// exists for, so "continue" must mean the unfinished work too.
#[test]
fn rerun_after_a_max_fail_stop_continues_the_never_ran_tail() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: first\n    When health is mischecked\n  \
         Scenario: second\n    When health is mischecked\n  \
         Scenario: third\n    When health is mischecked\n",
    )
    .unwrap();
    let pack = |code: u16| {
        format!(
            "macros:\n  bad:\n    match: health is mischecked\n    steps:\n      - hurl: |\n          \
             GET ${{url:base}}/health\n          HTTP {code}\n"
        )
    };
    let pack_path = cwd.path().join("suite/packs/p.yaml");

    // Stop at the first failure: 1 failed, 2 never ran.
    std::fs::write(&pack_path, pack(500)).unwrap();
    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--jobs", "1", "--max-fail", "1"])
        .assert()
        .code(1);

    // Fix everything, then continue.
    std::fs::write(&pack_path, pack(200)).unwrap();
    let assert = proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--rerun"])
        .assert()
        .code(0);
    let rendered = String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
        + &String::from_utf8_lossy(&assert.get_output().stderr);
    // The green is earned: all three scenarios ran — the failure retried AND
    // the two the stopped run never reached — and the continuation said so.
    assert!(
        rendered.contains("3 passed"),
        "the rerun must continue the never-ran tail, not just retry the failure: {rendered}"
    );
    assert!(
        rendered.contains("cancelled before 2 scenario(s) ran"),
        "continuing a partial run must be announced: {rendered}"
    );

    // A rerun from a *completed* base is unchanged: it retries failures only.
    std::fs::write(&pack_path, pack(500)).unwrap();
    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--jobs", "1"])
        .assert()
        .code(1); // completes: 3 failed, nothing skipped
    std::fs::write(&pack_path, pack(200)).unwrap();
    let assert = proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--rerun"])
        .assert()
        .code(0);
    let rendered = String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
        + &String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        rendered.contains("3 passed") && !rendered.contains("note: the last run was cancelled"),
        "a completed base keeps the old semantics, without the partial-run note: {rendered}"
    );
}

/// A secret reflected back *encoded* is still redacted (S1).
///
/// The raw-needle half of ADR-0005 was airtight and insufficient, demonstrated
/// live: the fixture's introspection route echoes the bearer base64-encoded (an
/// OAuth-introspection shape), a failing assert then quotes that string in its
/// detail, and — before `Redactions` derived encoded forms — `dG9r…` reached
/// the console and `events.jsonl`, trivially `base64 -d`-able back to the live
/// credential. The scenario *fails on purpose*: the leak only ever rode on the
/// assert-failure detail, so a green run would test nothing.
#[test]
fn an_encoded_reflection_of_a_secret_never_reaches_the_record() {
    // The base64 of `fixture-token`, pinned by the fixture's own unit test so
    // this literal and the route cannot drift apart silently.
    const ENCODED: &str = "Zml4dHVyZS10b2tlbg==";

    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n  Scenario: the token comes back encoded\n    \
         When the token is introspected\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  introspect:\n    match: the token is introspected\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/api/v1/token/introspect\n          \
         Authorization: Bearer ${secret:apiToken}\n          HTTP 200\n          \
         [Asserts]\n          jsonpath \"$.token_b64\" == \"will-not-match\"\n",
    )
    .unwrap();

    let assert = proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(1); // the assert genuinely fails — that is the leaking path

    let console = String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
        + &String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        !console.contains(ENCODED),
        "encoded secret leaked into the console:\n{console}"
    );
    // Non-vacuity: the assert detail was actually rendered — redacted, not
    // absent. Without this, a reporting change that dropped the detail line
    // entirely would pass the scan above while testing nothing.
    assert!(
        console.contains("***"),
        "expected a redacted assert detail in the console:\n{console}"
    );

    let run_dir = latest_run_dir(cwd.path());
    let events = std::fs::read_to_string(run_dir.join("events.jsonl")).unwrap();
    assert!(
        !events.contains(ENCODED),
        "encoded secret leaked into events.jsonl:\n{events}"
    );
    let log = std::fs::read_to_string(run_dir.join("run.log")).unwrap_or_default();
    assert!(!log.contains(ENCODED), "encoded secret leaked into run.log");
    // And the raw form stays out too — deriving needles must not have
    // regressed the original invariant.
    for text in [&console, &events, &log] {
        assert!(!text.contains(API_TOKEN), "raw secret leaked");
    }
}

/// TESTING-STRATEGY fixture cases: the negative-path endpoints have
/// consumers. A wrong bearer asserts the 401 contract (green — asserting a
/// refusal is a passing test), and a jsonpath over `/malformed`'s broken JSON
/// fails as a *test failure* (exit 1) with its artifact anchor — never a
/// crash or a system fault.
#[test]
fn auth_rejection_and_malformed_bodies_are_exercised() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n  Scenario: a wrong token is refused\n    When an invalid token is rejected\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  denied:\n    match: an invalid token is rejected\n    steps:\n      - name: wrong bearer is refused with the 401 contract\n        hurl: |\n          GET ${url:base}/api/v1/admin/search/records\n          Authorization: Bearer wrong-token\n          HTTP 401\n          [Asserts]\n          jsonpath \"$.error\" == \"unauthorized\"\n",
    )
    .unwrap();
    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(0);

    let broken = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(broken.path().join("suite/packs")).unwrap();
    std::fs::write(
        broken.path().join("suite/case.feature"),
        "# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n  Scenario: broken json fails the assert\n    When the malformed payload is parsed\n",
    )
    .unwrap();
    std::fs::write(
        broken.path().join("suite/packs/p.yaml"),
        "macros:\n  broken:\n    match: the malformed payload is parsed\n    steps:\n      - name: jsonpath over a truncated body\n        hurl: |\n          GET ${url:base}/malformed\n          HTTP 200\n          [Asserts]\n          jsonpath \"$.broken\" == \"x\"\n",
    )
    .unwrap();
    let assert = proef_in(broken.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(1);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains("case.feature:4"),
        "feature anchor: {stderr}"
    );
    assert!(stderr.contains(".hurl:"), "artifact anchor: {stderr}");
}

/// `proef explain` replays the record and names the failures (ADR-0008).
#[test]
fn explain_summarizes_the_latest_run() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n  Scenario: doomed\n    When the health endpoint is checked\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  health:\n    match: the health endpoint is checked\n    steps:\n      - hurl: |\n          GET ${url:base}/health\n          HTTP 500\n",
    )
    .unwrap();
    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(1);

    let assert = proef_in(cwd.path(), &fixture)
        .arg("explain")
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.contains("1 failed"), "{stdout}");
    assert!(stdout.contains("case.feature:4"), "{stdout}");
}

/// The `cargo test` invocation that drives the harness's own test target — one
/// place that knows the crate and target names, so a rename is not something to
/// find twice.
///
/// **The `harness_` test-name prefix is load-bearing**: `.config/nextest.toml`
/// widens the slow-timeout kill ceiling for `test(/^harness_/)` because these
/// tests shell a real cargo build (216s measured on a cold cache). A test that
/// calls this without the prefix — or a rename that drops it — silently loses
/// the override and dies at the 120s global ceiling on cold CI, far from the
/// cause. Keep the prefix, or move the filter with the name.
fn harness_command() -> std::process::Command {
    let mut cmd = std::process::Command::new("cargo");
    cmd.current_dir(workspace_root())
        .args([
            "test",
            "-q",
            "-p",
            "proef-harness",
            "--test",
            "scenarios",
            "--",
        ])
        .env("NO_COLOR", "1");
    cmd
}

/// Three bytes that are not valid UTF-8 in any position — what a variable looks
/// like when the harness can tell it is set but not what it says.
#[cfg(unix)]
const NOT_UTF8: &[u8] = &[0x66, 0xff, 0x6f];

/// List the harness's trials with `extra_env` applied, so a test can poison
/// one variable and see what the harness exposes.
#[cfg(unix)]
fn list_harness_trials(extra_env: &[(&str, &std::ffi::OsStr)]) -> String {
    let mut cmd = harness_command();
    cmd.args(["--list", "--format", "terse"]);
    for (name, value) in extra_env {
        cmd.env(name, value);
    }
    String::from_utf8_lossy(&cmd.output().unwrap().stdout).into_owned()
}

/// A variable the harness cannot read must surface as a loud failing trial,
/// never as an empty trial list — "never run zero tests green" is the
/// harness's own stated invariant, and a test runner that silently tests
/// nothing is the worst way to report success. Unset stays meaningful: it
/// deliberately exposes nothing so a plain `cargo test` stays green.
#[cfg(unix)]
#[test]
fn harness_fails_loudly_on_an_unreadable_suite_variable() {
    use std::os::unix::ffi::OsStrExt as _;
    let bad = std::ffi::OsStr::from_bytes(NOT_UTF8);
    let listing = list_harness_trials(&[("PROEF_HARNESS_SUITE", bad)]);
    assert!(
        listing.contains("proef::config"),
        "an unreadable suite variable must expose a loud config trial, not an \
         empty list; got: {listing}"
    );
}

/// The same rule for the binary path: falling back to `proef` on PATH would
/// silently test a different binary than the caller named.
#[cfg(unix)]
#[test]
fn harness_fails_loudly_on_an_unreadable_binary_variable() {
    use std::os::unix::ffi::OsStrExt as _;
    let bad = std::ffi::OsStr::from_bytes(NOT_UTF8);
    let suite = workspace_root().join("tests/features");
    let listing = list_harness_trials(&[
        ("PROEF_HARNESS_SUITE", suite.as_os_str()),
        ("PROEF_BIN", bad),
    ]);
    assert!(
        listing.contains("proef::config"),
        "an unreadable binary path must expose a loud config trial rather than \
         falling back to PATH; got: {listing}"
    );
}

/// US-12: the libtest-mimic harness lists one Trial per scenario and runs an
/// exact selection — the nextest/IDE contract (`--list`, `--exact`).
#[test]
fn harness_lists_and_runs_scenarios() {
    let fixture = Fixture::start().unwrap();
    let suite = workspace_root().join("tests/features");
    let proef_bin = assert_cmd::cargo::cargo_bin("proef");

    let run_harness = |extra: &[&str]| {
        let mut cmd = harness_command();
        cmd.args(extra)
            .env("PROEF_HARNESS_SUITE", suite.display().to_string())
            .env("PROEF_BIN", &proef_bin)
            .env("PROEF_BASE_URL", &fixture.base_url)
            .env("PROEF_SECRET_APITOKEN", API_TOKEN);
        cmd.output().unwrap()
    };

    let list = run_harness(&["--list", "--format", "terse"]);
    let listing = String::from_utf8_lossy(&list.stdout).into_owned();
    assert!(list.status.success(), "{listing}");
    assert_eq!(listing.matches(": test").count(), 12, "{listing}");
    assert!(
        listing.contains("500-api-note::A note posted via the API appears on the board")
            || listing.contains("500_api_note::A note posted via the API appears on the board"),
        "{listing}"
    );

    let exact = run_harness(&[
        "--exact",
        "520_api_breadth::A profile form is submitted",
        "--nocapture",
    ]);
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&exact.stdout),
        String::from_utf8_lossy(&exact.stderr)
    );
    assert!(exact.status.success(), "{out}");
    assert!(
        out.contains("1 passed") || out.contains("test result: ok"),
        "{out}"
    );
}

/// Parallel `--jobs` determinism: two runs produce the same normalized event
/// view (TESTING-STRATEGY flake rule — never raw interleaving).
#[test]
fn parallel_runs_are_deterministic_under_normalization() {
    let fixture = Fixture::start().unwrap();
    let corpus = workspace_root().join("tests/features");
    let mut normalized = Vec::new();
    for _ in 0..2 {
        let cwd = corpus_cwd();
        proef_in(cwd.path(), &fixture)
            .args(["test", &corpus.display().to_string(), "--jobs", "4"])
            .assert()
            .code(0);
        let run_dir = latest_run_dir(cwd.path());
        normalized.push(normalize_events(&run_dir.join("events.jsonl")));
    }
    assert_eq!(normalized[0], normalized[1]);
}

/// ADR-0005 extended to the state sink: a `saveAs: global` capture whose
/// value equals a known secret is refused (the step warns) — secret-derived
/// material never reaches `.proef-state.json` in plaintext.
#[test]
fn secret_valued_captures_never_promote_to_global_state() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n  Scenario: leak guard\n    When the record is fetched\n",
    )
    .unwrap();
    // `recordRef`'s secret value is `r-1` — exactly what the endpoint echoes
    // back as `$.id`, so the capture is secret-valued by construction.
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  fetch:\n    match: the record is fetched\n    steps:\n      - saveAs: { leaked: global }\n        hurl: |\n          GET ${url:base}/api/v1/records/${secret:recordRef}\n          Authorization: Bearer ${secret:apiToken}\n          HTTP 200\n          [Captures]\n          leaked: jsonpath \"$.id\"\n",
    )
    .unwrap();
    std::fs::write(cwd.path().join("proef.toml"), BASE_URL_CONFIG).unwrap();

    let mut run = Command::cargo_bin("proef").unwrap();
    let assert = run
        .current_dir(cwd.path())
        .env("PROEF_BASE_URL", &fixture.base_url)
        .env("PROEF_SECRET_APITOKEN", API_TOKEN)
        .env("PROEF_SECRET_RECORDREF", "r-1")
        .args(["test", "suite", "--jobs", "1"])
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        stdout.contains("saveAs: global refused for `leaked`"),
        "the refusal must surface on the owning step: {stdout}"
    );

    let state = cwd.path().join(".proef-state.json");
    if state.exists() {
        let text = std::fs::read_to_string(&state).unwrap();
        assert!(
            !text.contains("leaked") && !text.contains("r-1"),
            "secret-valued capture persisted: {text}"
        );
    }
}

// `proef diff --fail-on-regression` must not pass on a truncated or
// cancelled new run. The synthetic run dirs below skip real execution and
// write `events.jsonl` directly by serializing `Event`s (the JSONL stream IS
// the record, ADR-0008) — mirroring record.rs's own test helpers.

/// A valid uuid-v7-shaped dir name (`fsutil::is_run_id` parses via
/// `uuid::Uuid::try_parse`) so `all_runs` picks these up under the default
/// `diff` resolution (no base/new given → previous vs latest).
const DIFF_BASE_RUN_ID: &str = "00000000-0000-0000-0000-000000000001";
const DIFF_NEW_RUN_ID: &str = "00000000-0000-0000-0000-000000000002";

/// Write `events` as one JSON object per line into `<runs_root>/<id>/events.jsonl`.
fn write_run(runs_root: &Path, id: &str, events: &[proef_core::event::Event]) {
    let dir = runs_root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let body: String = events
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("events.jsonl"), body).unwrap();
}

fn diff_run_started(run_id: &str) -> proef_core::event::Event {
    proef_core::event::Event::RunStarted {
        schema: proef_core::event::EVENT_SCHEMA_VERSION,
        run_id: std::sync::Arc::from(run_id),
    }
}

fn diff_step_finished() -> proef_core::event::Event {
    use proef_core::step::{Status, StepRef};
    proef_core::event::Event::StepFinished {
        scenario: std::sync::Arc::from("health"),
        engine: std::sync::Arc::from("hurl"),
        step: StepRef {
            file: std::sync::Arc::from("case.feature"),
            line: 1,
            text: std::sync::Arc::from("health is checked"),
        },
        status: Status::Passed,
        attempts: 1,
        duration_ms: 10,
        captures: Vec::new(),
        fragment: None,
        detail: None,
        attempt_details: Vec::new(),
    }
}

fn diff_scenario_finished() -> proef_core::event::Event {
    proef_core::event::Event::ScenarioFinished {
        scenario: std::sync::Arc::from("health"),
        file: std::sync::Arc::from("case.feature"),
        status: proef_core::step::Status::Passed,
        timestamp_ms: None,
        worker: None,
        phase: None,
    }
}

fn diff_run_finished(cancelled: bool) -> proef_core::event::Event {
    proef_core::event::Event::RunFinished {
        passed: 1,
        failed: 0,
        skipped: 0,
        cancelled,
    }
}

/// A complete run: one passing scenario, tail `RunFinished { cancelled: false }`.
fn complete_pass_events(run_id: &str) -> Vec<proef_core::event::Event> {
    vec![
        diff_run_started(run_id),
        diff_step_finished(),
        diff_scenario_finished(),
        diff_run_finished(false),
    ]
}

/// A truncated/died run: same scenario, but no tail `RunFinished` at all.
fn incomplete_pass_events(run_id: &str) -> Vec<proef_core::event::Event> {
    vec![
        diff_run_started(run_id),
        diff_step_finished(),
        diff_scenario_finished(),
    ]
}

/// A cancelled run: tail `RunFinished { cancelled: true }`.
fn cancelled_pass_events(run_id: &str) -> Vec<proef_core::event::Event> {
    vec![
        diff_run_started(run_id),
        diff_step_finished(),
        diff_scenario_finished(),
        diff_run_finished(true),
    ]
}

/// ADR-0008: a new run with no tail `RunFinished` (truncated/died) cannot
/// certify "no regressions" — `--fail-on-regression` must fail it even
/// though the one scenario present didn't itself regress.
#[test]
fn fail_on_regression_fails_when_new_run_is_incomplete() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join(".proef-runs");
    write_run(
        &runs,
        DIFF_BASE_RUN_ID,
        &complete_pass_events(DIFF_BASE_RUN_ID),
    );
    write_run(
        &runs,
        DIFF_NEW_RUN_ID,
        &incomplete_pass_events(DIFF_NEW_RUN_ID),
    );

    Command::cargo_bin("proef")
        .unwrap()
        .current_dir(tmp.path())
        .args(["diff", "--fail-on-regression"])
        .assert()
        .code(1)
        .stderr(
            predicates::str::contains("INCOMPLETE").or(predicates::str::contains("cannot certify")),
        );
}

/// A cancelled new run is likewise not gate-clean, with wording that
/// distinguishes it from a plain incomplete/truncated run.
#[test]
fn fail_on_regression_fails_when_new_run_was_cancelled() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join(".proef-runs");
    write_run(
        &runs,
        DIFF_BASE_RUN_ID,
        &complete_pass_events(DIFF_BASE_RUN_ID),
    );
    write_run(
        &runs,
        DIFF_NEW_RUN_ID,
        &cancelled_pass_events(DIFF_NEW_RUN_ID),
    );

    Command::cargo_bin("proef")
        .unwrap()
        .current_dir(tmp.path())
        .args(["diff", "--fail-on-regression"])
        .assert()
        .code(1)
        .stderr(predicates::str::contains("cancelled"));
}

/// Without `--fail-on-regression`, diff stays informational (exit 0) but still
/// banners an incomplete record so a human is never misled by a partial run.
#[test]
fn plain_diff_reports_incomplete_but_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join(".proef-runs");
    write_run(
        &runs,
        DIFF_BASE_RUN_ID,
        &complete_pass_events(DIFF_BASE_RUN_ID),
    );
    write_run(
        &runs,
        DIFF_NEW_RUN_ID,
        &incomplete_pass_events(DIFF_NEW_RUN_ID),
    );

    let assert = Command::cargo_bin("proef")
        .unwrap()
        .current_dir(tmp.path())
        .args(["diff"])
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        stdout.to_lowercase().contains("incomplete"),
        "expected an incomplete-run banner: {stdout}"
    );
}

/// A closed stderr pipe must not panic the execution path. `head -c0` reads
/// nothing and exits, closing the read end, so every later stderr write gets
/// EPIPE — and a raw `eprintln!` would abort with 101, outside the typed
/// 0/1/2/3 exit taxonomy (ADR-0009). Unix-only because EPIPE and `head` are
/// POSIX; the guard under test is cross-platform.
#[cfg(unix)]
#[test]
fn failure_summary_does_not_panic_on_a_closed_stderr_pipe() {
    use std::fmt::Write as _;

    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(cwd.path().join("proef.toml"), BASE_URL_CONFIG).unwrap();

    // Several failing scenarios: the summary writes a fault line, a failed-step
    // line, a curl hint and a reproduce line per failure, so the write lands
    // well after the reader closes instead of racing one short line.
    let mut feature = String::from("# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n");
    for n in 1..=6 {
        let _ = write!(
            feature,
            "  Scenario: health case {n}\n    When the health endpoint is checked\n"
        );
    }
    std::fs::write(cwd.path().join("suite/case.feature"), feature).unwrap();
    // The fixture answers /health with 200; asserting 500 fails every scenario.
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  health:\n    match: the health endpoint is checked\n    steps:\n      - hurl: |\n          GET ${url:base}/health\n          HTTP 500\n",
    )
    .unwrap();

    let bin = assert_cmd::cargo::cargo_bin("proef");
    let mut proef = std::process::Command::new(&bin)
        .current_dir(cwd.path())
        .env("NO_COLOR", "1")
        .env("PROEF_BASE_URL", &fixture.base_url)
        .env("PROEF_SECRET_APITOKEN", API_TOKEN)
        .args(["test", "suite"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Consume nothing, then drop the reader to close the pipe early.
    let mut head = std::process::Command::new("head")
        .args(["-c", "0"])
        .stdin(proef.stderr.take().unwrap())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let _ = head.wait();
    let status = proef.wait().unwrap();
    // Pins two things at once: no panic (101), and the run actually reached a
    // normal test-failure outcome — so the test cannot silently decay into
    // asserting nothing if the corpus ever stops failing.
    assert_eq!(
        status.code(),
        Some(1),
        "expected the contracted test-failure exit, not a panic or an early abort"
    );
}

/// A suite with `[run] setup` + `[run] teardown`, a PASSING setup, a FAILING
/// main scenario, and a PASSING teardown — the shape that makes phase-blended
/// records visible: the last `run_finished` (teardown's, all-passing) would
/// otherwise win the headline over the suite's own failure.
fn write_phase_suite(cwd: &Path) {
    std::fs::create_dir_all(cwd.join("suite/packs")).unwrap();
    std::fs::write(
        cwd.join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\nsuite = \"suite\"\n\
         setup = \"suite/setup.feature\"\nteardown = \"suite/teardown.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.join("suite/setup.feature"),
        "Feature: S\n  Scenario: provision\n    When setup probes health\n",
    )
    .unwrap();
    // The fixture answers /health with 200; asserting 500 fails this scenario
    // (same technique as `failure_maps_to_feature_line_and_artifact_span`).
    std::fs::write(
        cwd.join("suite/case.feature"),
        "Feature: F\n  Scenario: health asserts the wrong status\n    \
         When the health endpoint is checked\n",
    )
    .unwrap();
    std::fs::write(
        cwd.join("suite/teardown.feature"),
        "Feature: T\n  Scenario: cleanup\n    When teardown probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.join("suite/packs/p.yaml"),
        "macros:\n  setupProbe:\n    match: setup probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n  \
         mainProbe:\n    match: the health endpoint is checked\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 500\n  \
         teardownProbe:\n    match: teardown probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();
}

/// The newest run record's `events.jsonl`, as written under `.proef-runs/`.
fn latest_events_jsonl(cwd: &Path) -> String {
    std::fs::read_to_string(latest_run_dir(cwd).join("events.jsonl")).unwrap()
}

/// A run with setup and teardown must still produce ONE record: one
/// `run_started` line and one `run_finished` line. Three head/tail pairs make
/// every whole-file consumer read phase-blended results.
#[test]
fn phases_produce_a_single_run_started_and_run_finished() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // Build a suite with a PASSING setup, a FAILING main scenario, and a
    // PASSING teardown. The failing main is what makes the bug visible: the
    // last `run_finished` (teardown's) would otherwise win the headline.
    write_phase_suite(cwd.path());

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(1);

    let record = latest_events_jsonl(cwd.path());
    let started = record
        .lines()
        .filter(|l| l.contains("\"run_started\""))
        .count();
    let finished = record
        .lines()
        .filter(|l| l.contains("\"run_finished\""))
        .count();
    assert_eq!(started, 1, "expected exactly one run_started:\n{record}");
    assert_eq!(finished, 1, "expected exactly one run_finished:\n{record}");
}

/// `explain` must report the suite's own verdict, never a blend with
/// setup/teardown (ADR-0014). Before the run-record merge fix this printed
/// "1 passed · 0 failed" — teardown's own totals, the last phase to close its
/// own `run_finished` — directly above a printed failure. Summing all three
/// phases instead (setup's 1 passed + suite's 1 failed + teardown's 1 passed)
/// would silently count setup/teardown scenarios as if they were part of the
/// suite; the ruling is suite-only totals, so this pins that instead.
#[test]
fn explain_reports_the_failure_not_the_teardown_totals() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    write_phase_suite(cwd.path());

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(1);

    let assert = proef_in(cwd.path(), &fixture)
        .arg("explain")
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        out.contains("0 passed · 1 failed · 0 skipped"),
        "headline must be the suite's own verdict: {out}"
    );
    assert!(
        !out.contains("1 passed · 0 failed"),
        "headline must not be teardown's totals: {out}"
    );
    assert!(
        !out.contains("2 passed"),
        "headline must not sum setup+suite+teardown scenarios: {out}"
    );

    // Pin the aggregation itself, not just the headline text: only the
    // suite's own outcome (1 failed) reaches `run_finished`. Setup's pass and
    // teardown's pass are still visible as their own scenario events in the
    // record (asserted below) — just never folded into these totals.
    let record = latest_events_jsonl(cwd.path());
    assert!(
        record.contains(r#""event":"run_finished","passed":0,"failed":1,"skipped":0"#),
        "run_finished must report the suite's totals only, not summed across phases: {record}"
    );
    assert!(
        record.contains(r#""scenario":"provision""#) && record.contains(r#""scenario":"cleanup""#),
        "setup/teardown scenarios must still appear as events in the record: {record}"
    );
}

/// The console run header keys off `RunStarted`, so suppressing phase head/tail
/// must also collapse the three headers a phased run used to print.
#[test]
fn console_prints_the_run_header_once_per_run() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    write_phase_suite(cwd.path());

    let assert = proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(1);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    // `ConsoleReporter::on_event`'s `Event::RunStarted` arm writes `"proef run
    // {run_id}"` to `self.out` (report.rs:217-218) — the per-run header, once
    // per `RunStarted`. `self.out` is stdout here (no `--output json`/`tap`,
    // so `machine_stdout` is false and `console_out` is `std::io::stdout()`,
    // exec.rs:154-158) — the same stream `out` reads.
    assert_eq!(
        out.matches("proef run ").count(),
        1,
        "run header should appear once, not once per phase: {out}"
    );
}

/// The critical regression this test guards: a failing `[run] setup` used to
/// `return` between the record's `RunStarted` and `RunFinished`, leaving a
/// record with a head, setup's scenario events, and no tail — this is the
/// exact shape `RunRecord`'s `Drop` closes even on that early-return path.
#[test]
fn setup_failure_still_closes_the_record_with_one_pair() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\nsetup = \"suite/setup.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/setup.feature"),
        "Feature: S\n  Scenario: broken setup\n    When setup probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: should not run\n    When the suite probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  setupProbe:\n    match: setup probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 500\n  \
         suiteProbe:\n    match: the suite probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(2); // user error — same path as setup_failure_aborts_the_run_as_a_user_error

    let record = latest_events_jsonl(cwd.path());
    let started = record
        .lines()
        .filter(|l| l.contains("\"run_started\""))
        .count();
    let finished = record
        .lines()
        .filter(|l| l.contains("\"run_finished\""))
        .count();
    assert_eq!(started, 1, "expected exactly one run_started:\n{record}");
    assert_eq!(finished, 1, "expected exactly one run_finished:\n{record}");
}

/// A record with no `run_finished` is a truncated run — OOM-kill, CI timeout,
/// crash. `explain` and `report` are the post-mortem tools, so they are exactly
/// the ones that must say so instead of rendering it as complete.
#[test]
fn explain_and_report_flag_a_truncated_record() {
    let cwd = tempfile::tempdir().unwrap();
    let run = cwd
        .path()
        .join(".proef-runs/0198f3c1-0000-7000-8000-000000000001");
    std::fs::create_dir_all(&run).unwrap();
    // Starts, runs one scenario to completion, then stops: no run_finished.
    std::fs::write(
        run.join("events.jsonl"),
        concat!(
            r#"{"schema":1,"event":"run_started","run_id":"0198f3c1-0000-7000-8000-000000000001","scenarios":2}"#, "\n",
            r#"{"schema":1,"event":"scenario_started","scenario":"first","file":"suite/a.feature"}"#, "\n",
            r#"{"schema":1,"event":"scenario_finished","scenario":"first","file":"suite/a.feature","status":"passed","line":3}"#, "\n",
        ),
    )
    .unwrap();

    let mut explain = assert_cmd::Command::cargo_bin("proef").unwrap();
    let assert = explain
        .current_dir(cwd.path())
        .env("NO_COLOR", "1")
        .arg("explain")
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        out.contains("incomplete"),
        "explain must flag incompleteness: {out}"
    );
    // The record holds one passed scenario; reporting zeros is the bug.
    assert!(
        !out.contains("0 passed · 0 failed · 0 skipped"),
        "totals must come from the scenarios present, not the missing tail: {out}"
    );

    let out_html = cwd.path().join("report.html");
    let mut report = assert_cmd::Command::cargo_bin("proef").unwrap();
    report
        .current_dir(cwd.path())
        .env("NO_COLOR", "1")
        .args(["report", "-o", &out_html.display().to_string()])
        .assert()
        .code(0);
    let html = std::fs::read_to_string(&out_html).unwrap();
    // Not a bare `contains("incomplete")`: the page's stylesheet always
    // carries the `.incomplete-banner` rule, so that substring alone would
    // pass even if the banner paragraph itself were never inserted. The
    // banner's own wording is the real signal.
    assert!(
        html.contains("run incomplete"),
        "report must banner incompleteness"
    );
}

/// A truncated record where one scenario finished and a second is still in
/// flight (`ScenarioStarted` + steps, no matching `ScenarioFinished`) when
/// the stream ends — the shape a crash/OOM-kill/CI-timeout actually leaves
/// behind, and the case the whole truncated-record fix exists for.
fn truncated_with_in_flight_events(run_id: &str) -> Vec<proef_core::event::Event> {
    use proef_core::event::Event;
    use proef_core::step::{Status, StepRef};
    use std::sync::Arc;
    vec![
        diff_run_started(run_id),
        diff_step_finished(),     // "health" scenario: 1 step, 1 attempt.
        diff_scenario_finished(), // "health" finishes.
        Event::ScenarioStarted {
            scenario: Arc::from("second"),
            file: Arc::from("case.feature"),
            timestamp_ms: None,
            worker: None,
            phase: None,
        },
        Event::StepFinished {
            scenario: Arc::from("second"),
            engine: Arc::from("hurl"),
            step: StepRef {
                file: Arc::from("case.feature"),
                line: 5,
                text: Arc::from("in-flight step one"),
            },
            status: Status::Passed,
            attempts: 3,
            duration_ms: 5,
            captures: Vec::new(),
            fragment: None,
            detail: None,
            attempt_details: Vec::new(),
        },
        Event::StepFinished {
            scenario: Arc::from("second"),
            engine: Arc::from("hurl"),
            step: StepRef {
                file: Arc::from("case.feature"),
                line: 6,
                text: Arc::from("in-flight step two"),
            },
            status: Status::Passed,
            attempts: 2,
            duration_ms: 5,
            captures: Vec::new(),
            fragment: None,
            detail: None,
            attempt_details: Vec::new(),
        },
        // No `ScenarioFinished` for "second", and no tail `RunFinished` at all.
    ]
}

/// `explain`'s step/attempt totals must count a still-in-flight scenario's
/// steps, not just the ones attached to a finished `ScenarioFinished` — a
/// step only attaches to `Record::scenarios` once its scenario closes
/// (`record::parse_record`), so counting from `rec.scenarios` alone silently
/// drops the dying scenario's step evidence from a truncated record.
#[test]
fn explain_counts_steps_from_a_still_in_flight_scenario() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join(".proef-runs");
    let run_id = "00000000-0000-0000-0000-000000000003";
    write_run(&runs, run_id, &truncated_with_in_flight_events(run_id));

    let assert = Command::cargo_bin("proef")
        .unwrap()
        .current_dir(tmp.path())
        .env("NO_COLOR", "1")
        .arg("explain")
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    // 1 attempt (finished "health") + 3 + 2 (in-flight "second") = 3 steps, 6 attempts.
    assert!(
        out.contains("3 step(s), 6 attempt(s)"),
        "the in-flight scenario's steps must still count toward the headline: {out}"
    );
}

/// A cancelled run is a *complete* run (`RunFinished { cancelled: true }`),
/// not an incomplete one — `record::RunCompletion` keeps the two apart.
/// `diff` bans everything that `!= Completed` from certifying "no
/// regressions" (`diff.rs`), but `explain`/`report` must not borrow that
/// wider rule for their incompleteness banner: a cancelled run has nothing
/// missing to apologize for.
#[test]
fn cancelled_record_is_not_bannered_as_incomplete() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join(".proef-runs");
    let run_id = "00000000-0000-0000-0000-000000000004";
    write_run(&runs, run_id, &cancelled_pass_events(run_id));

    let assert = Command::cargo_bin("proef")
        .unwrap()
        .current_dir(tmp.path())
        .env("NO_COLOR", "1")
        .arg("explain")
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        !out.contains("incomplete"),
        "a cancelled run must not banner as incomplete: {out}"
    );

    let out_html = tmp.path().join("report.html");
    Command::cargo_bin("proef")
        .unwrap()
        .current_dir(tmp.path())
        .env("NO_COLOR", "1")
        .args(["report", "-o", &out_html.display().to_string()])
        .assert()
        .code(0);
    let html = std::fs::read_to_string(&out_html).unwrap();
    // Not a bare `contains("incomplete")`: the page's stylesheet always
    // carries the `.incomplete-banner` rule (used only when the banner
    // paragraph is actually inserted), so that substring is present on every
    // report regardless of completion. The banner's own wording is the signal.
    assert!(
        !html.contains("run incomplete"),
        "report must not banner a cancelled run as incomplete: {html}"
    );
}

/// With one job, every scenario runs on the one worker slot — so every stamped
/// `worker` must be 0. The pre-existing snapshot test uses a single scenario,
/// where a per-scenario ordinal and a worker slot are numerically identical;
/// this needs two or more to tell the two models apart.
#[test]
fn worker_is_a_slot_index_not_a_scenario_ordinal() {
    use std::fmt::Write as _;

    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(cwd.path().join("proef.toml"), BASE_URL_CONFIG).unwrap();
    let mut feature = String::from("Feature: F\n");
    for n in 1..=3 {
        let _ = writeln!(
            feature,
            "  Scenario: case {n}\n    When the health endpoint is checked"
        );
    }
    std::fs::write(cwd.path().join("suite/case.feature"), feature).unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  health:\n    match: the health endpoint is checked\n    steps:\n      - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--jobs", "1"])
        .assert()
        .code(0);

    let record = latest_events_jsonl(cwd.path());
    let stamped: Vec<&str> = record
        .lines()
        .filter(|l| l.contains("\"worker\""))
        .collect();
    assert!(stamped.len() >= 3, "expected stamped events: {record}");
    for line in &stamped {
        assert!(
            line.contains("\"worker\":0"),
            "every event should stamp slot 0 at --jobs 1: {line}"
        );
    }
}

/// A `[run] teardown` path that does not exist must fail like `[run] setup`
/// does — before the pool, as a user error. It used to run the entire suite
/// first (real requests, a run dir, artifacts) and then report exit 3, so the
/// same typo cost wildly different amounts depending on which key held it.
#[test]
fn a_nonexistent_teardown_path_fails_before_the_suite_runs() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\nteardown = \"suite/absent.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: passes\n    When the suite probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  suiteProbe:\n    match: the suite probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(2) // a bad path is the operator's typo, not a system fault
        .stderr(predicate::str::contains("absent.feature"));

    // The proof it failed *early*: nothing ran, so no run record exists.
    assert!(
        !cwd.path().join(".proef-runs").exists(),
        "a config error must not create a run record"
    );
}

/// `--dry-run` validates the phase features, which ADR-0014 has always claimed
/// ("validated like any other feature but never executed") and nothing did.
#[test]
fn dry_run_validates_the_setup_and_teardown_features() {
    let fixture = Fixture::start().unwrap();

    for key in ["setup", "teardown"] {
        let cwd = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
        std::fs::write(
            cwd.path().join("proef.toml"),
            format!(
                "[url]\nbase = \"${{env:PROEF_BASE_URL}}\"\n[run]\n{key} = \"phase.feature\"\n"
            ),
        )
        .unwrap();
        // The phase feature says something no pack binds.
        std::fs::write(
            cwd.path().join("phase.feature"),
            "Feature: P\n  Scenario: S\n    When nothing binds this sentence\n",
        )
        .unwrap();
        std::fs::write(
            cwd.path().join("suite/case.feature"),
            "Feature: F\n  Scenario: passes\n    When the suite probes health\n",
        )
        .unwrap();
        std::fs::write(
            cwd.path().join("suite/packs/p.yaml"),
            "macros:\n  suiteProbe:\n    match: the suite probes health\n    steps:\n      \
             - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
        )
        .unwrap();

        proef_in(cwd.path(), &fixture)
            .args(["test", "suite", "--dry-run"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains(key));
    }
}

/// Cleanup outlives the interrupt (ADR-0014: cleanup is reliable).
///
/// The pool used to share its token with teardown, so Ctrl-C left every
/// teardown scenario `Skipped` — and because a skipped phase carries no fault,
/// `phase_failed` waved it through in silence. Whatever setup created stayed
/// created, and nothing said so.
///
/// Unix-only because it sends a real SIGINT: the whole point is the operator's
/// interrupt, and simulating it through the internal token seam would not prove
/// the signal path. Compiles and runs on the ubuntu and macos gates.
#[cfg(unix)]
#[test]
fn teardown_runs_after_the_pool_is_interrupted() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\nteardown = \"teardown.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: slow one\n    When the slow thing happens\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("teardown.feature"),
        "Feature: T\n  Scenario: cleanup runs\n    When teardown probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  slow:\n    match: the slow thing happens\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/slow\n          HTTP 200\n  \
         teardownProbe:\n    match: teardown probes health\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("proef"))
        .current_dir(cwd.path())
        .env("NO_COLOR", "1")
        .env("PROEF_BASE_URL", &fixture.base_url)
        .args(["test", "suite"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // Drain stderr on its own thread: the interrupt notice and the cleanup
    // announcement are `errln!`, and a pipe nobody reads can wedge the child.
    let mut stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut stderr, &mut buf).ok();
        buf
    });

    // Interrupt once the run is demonstrably underway — read for the banner
    // rather than sleeping, so the test does not race a slow machine.
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut seen = String::new();
    loop {
        let mut line = String::new();
        assert!(
            reader.read_line(&mut line).unwrap() > 0,
            "process ended before the run started:\n{seen}"
        );
        seen.push_str(&line);
        if line.contains("running 1 scenario(s)") {
            break;
        }
    }
    std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();

    let mut rest = String::new();
    while reader.read_line(&mut rest).unwrap() > 0 {}
    seen.push_str(&rest);
    child.wait().unwrap();
    let errors = errors.join().unwrap();

    assert!(
        seen.contains("cancelled"),
        "the interrupt must have landed, or this proves nothing:\n{seen}"
    );
    assert!(
        seen.contains("cleanup runs"),
        "teardown must run after an interrupt, not vanish:\n{seen}"
    );
    assert!(
        errors.contains("cleaning up"),
        "the operator must be told cleanup is running:\n{errors}"
    );
}

/// A mixed suite+phase failure must label the phase block. The label was
/// derived from the whole report (`failed == 0`), so it appeared only while
/// *every* failure was a phase failure — and vanished the moment a suite
/// failure joined one, leaving `1 failed` above two indistinguishable blocks.
/// The record now carries `phase` per scenario and the label is per block.
#[test]
fn explain_labels_the_phase_block_even_beside_a_suite_failure() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[run]\nteardown = \"teardown.feature\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: S\n  Scenario: suite fails\n    When the suite expects a teapot\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("teardown.feature"),
        "Feature: T\n  Scenario: cleanup fails\n    When teardown expects a teapot\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  suiteTeapot:\n    match: the suite expects a teapot\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 418\n  \
         teardownTeapot:\n    match: teardown expects a teapot\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 418\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(3); // teardown's cleanup fault outranks the suite's test failure
    let assert = proef_in(cwd.path(), &fixture)
        .arg("explain")
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();

    assert!(stdout.contains("failed: suite fails"), "{stdout}");
    assert!(
        stdout.contains("failed (teardown — excluded from the totals above): cleanup fails"),
        "the phase block must stay labelled beside a suite failure:\n{stdout}"
    );
}

/// `--rerun` after a phase-only failure has nothing to rerun. Phases are
/// invisible to `--rerun` (ADR-0014) and are not in the pool `build_specs`
/// filters, so returning one produced a run that matched nothing and blamed
/// `--tags`/`--scenario` — flags the operator never passed.
#[test]
fn rerun_after_a_phase_only_failure_says_there_is_nothing_to_rerun() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    write_teardown_only_suite(cwd.path());

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(3); // a cleanup fault, not a test failure

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--rerun"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("nothing to rerun"))
        .stderr(predicate::str::contains("no scenarios matched the filters").not());
}

/// `--output json`'s `exit_code` must be the code the process actually exits
/// with. A failed JUnit write escalates to 3, and that escalation used to be
/// applied by a `return` *after* the body had already been printed — so a
/// machine consumer read a verdict the program then exited past, with no way
/// to notice the disagreement.
#[test]
fn the_json_body_reports_the_exit_code_the_process_uses() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(cwd.path().join("proef.toml"), BASE_URL_CONFIG).unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: fails\n    When the suite expects a teapot\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  teapot:\n    match: the suite expects a teapot\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 418\n",
    )
    .unwrap();

    // A JUnit path proef cannot create: a *file* stands where the parent
    // directory would go, so `create_dir_all` fails and the write is a system
    // fault (3), outranking the suite's own test failure (1).
    //
    // Previously this used a merely-absent parent, which stopped being a fault
    // when output paths began creating the directories they name — every
    // comparable runner does, and requiring `mkdir -p` in CI was the papercut.
    // The property under test is unchanged and still worth pinning: the JSON
    // body must not report a verdict the process then exits past.
    std::fs::write(cwd.path().join("blocker"), "not a directory").unwrap();
    let unwritable = cwd.path().join("blocker").join("report.junit.xml");
    let assert = proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--output", "json", "--junit"])
        .arg(&unwritable)
        .assert()
        .code(3);

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let body: serde_json::Value = serde_json::from_str(stdout.trim()).expect("a json body");
    assert_eq!(
        body["exit_code"], 3,
        "the body must not report a verdict the process exits past:\n{stdout}"
    );
}

/// A truncated record has no `run_finished`, so `explain` recounts the
/// scenarios present. That fallback counted `Passed`/`Failed`/`Skipped` and
/// not `Warned` — so a scenario whose `optional:` step warned vanished from
/// every column, and the reconstructed total silently disagreed with the run
/// it was reconstructing. The live path counts `Passed | Warned` together
/// (`RunSummary::passed` is "passed, warnings allowed"), and `optional:` exists
/// precisely so a scenario can warn and still pass.
#[test]
fn a_truncated_record_counts_a_warned_scenario_as_passed() {
    use proef_core::event::Event;
    use proef_core::step::Status;

    let cwd = tempfile::tempdir().unwrap();
    let runs = cwd.path().join(".proef-runs");
    // No `RunFinished`: the run died mid-write, which is what makes `explain`
    // fall back to counting.
    write_run(
        &runs,
        "00000000-0000-0000-0000-0000000000aa",
        &[
            diff_run_started("00000000-0000-0000-0000-0000000000aa"),
            Event::ScenarioFinished {
                scenario: std::sync::Arc::from("warns but passes"),
                file: std::sync::Arc::from("case.feature"),
                status: Status::Warned,
                timestamp_ms: None,
                worker: None,
                phase: None,
            },
        ],
    );

    let assert = Command::cargo_bin("proef")
        .unwrap()
        .current_dir(cwd.path())
        .env("NO_COLOR", "1")
        .arg("explain")
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        stdout.contains("1 passed"),
        "a warned scenario must be counted, not dropped:\n{stdout}"
    );
    assert!(stdout.contains("run incomplete"), "{stdout}");
}
