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
        std::fs::write(&config, "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n").unwrap();
    }
    let mut cmd = Command::cargo_bin("proef").unwrap();
    cmd.current_dir(dir)
        .env("NO_COLOR", "1")
        .env("PROEF_BASE_URL", &fixture.base_url)
        .env("PROEF_SECRET_APITOKEN", API_TOKEN);
    cmd
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
    let cwd = tempfile::tempdir().unwrap();
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
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n[http]\ntimeout-ms = 500\n",
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
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n",
    )
    .unwrap();

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
        let cwd = tempfile::tempdir().unwrap();
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
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n",
    )
    .unwrap();

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
