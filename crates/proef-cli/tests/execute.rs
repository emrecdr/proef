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

/// ADR-0014: a teardown failure is a distinct non-zero signal (exit 3, a
/// cleanup fault) — the suite's own green verdict still stands.
#[test]
fn teardown_failure_is_a_distinct_cleanup_fault() {
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
        "Feature: T\n  Scenario: broken cleanup\n    When teardown probes health\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: passes\n    When the suite probes health\n",
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

    let assert = proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(3); // cleanup fault, distinct from a test failure (1) or green (0)
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("teardown failed"), "{stderr}");
    let events = std::fs::read_to_string(latest_run_dir(cwd.path()).join("events.jsonl")).unwrap();
    assert!(
        events.contains(r#""scenario":"passes""#),
        "the suite itself ran: {events}"
    );
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
        .args(["secret", "set", "apiToken", "--value", API_TOKEN])
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

/// US-12: the libtest-mimic harness lists one Trial per scenario and runs an
/// exact selection — the nextest/IDE contract (`--list`, `--exact`).
#[test]
fn harness_lists_and_runs_scenarios() {
    let fixture = Fixture::start().unwrap();
    let suite = workspace_root().join("tests/features");
    let proef_bin = assert_cmd::cargo::cargo_bin("proef");

    let run_harness = |extra: &[&str]| {
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
            .args(extra)
            .env("PROEF_HARNESS_SUITE", suite.display().to_string())
            .env("PROEF_BIN", &proef_bin)
            .env("PROEF_BASE_URL", &fixture.base_url)
            .env("PROEF_SECRET_APITOKEN", API_TOKEN)
            .env("NO_COLOR", "1");
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
