//! The corpus suites (TESTING-STRATEGY):
//!
//! - `tests/features/` — the real suite doubles as the `--dry-run` regression
//!   corpus (the four 500-series features + the proef-native matrix).
//! - `tests/errors/` — seeded broken inputs, one directory per diagnostic
//!   code (`pack__adjacent_captures` ⇒ `proef::pack::adjacent_captures`);
//!   every case must exit 2 with its code in the rendered output, and the
//!   rendering is snapshot-locked (insta).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn proef() -> Command {
    let mut cmd = Command::cargo_bin("proef").unwrap();
    cmd.current_dir(workspace_root()).env("NO_COLOR", "1");
    cmd
}

#[test]
fn green_corpus_dry_runs_clean() {
    let assert = proef()
        .args(["test", "tests/features", "--dry-run"])
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.contains("dry-run OK"), "{stdout}");
    assert!(
        stdout.contains("0 warning(s)"),
        "corpus must be warning-free: {stdout}"
    );
    // All six corpus features validate.
    assert_eq!(stdout.matches("  ok ").count(), 6, "{stdout}");
}

#[test]
fn tags_filter_selects_scenarios() {
    let assert = proef()
        .args([
            "test",
            "tests/features",
            "--dry-run",
            "--tags",
            "sync-message",
        ])
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(stdout.contains("(1 selected by the filters)"), "{stdout}");
}

#[test]
fn flows_lists_the_corpus() {
    let assert = proef().args(["flows", "tests/features"]).assert().code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        stdout.contains("A message sent via the API appears in the client feed"),
        "{stdout}"
    );
    assert!(stdout.contains("@sync-message"), "{stdout}");
    assert!(
        stdout.contains("Search over clients returns 200"),
        "{stdout}"
    );
}

#[test]
fn schema_prints_merged_json() {
    let assert = proef().arg("schema").assert().code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let schema: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(
        schema.pointer("/$defs/RawStep/properties/hurl").is_some(),
        "engine fragment merged"
    );
}

#[test]
fn execution_without_dry_run_is_not_available_yet() {
    proef().args(["test", "tests/features"]).assert().code(2);
}

/// The canonical artifact corpus (ADR-0010): emission is byte-deterministic
/// under a fixed run id, the written bytes are exactly the parse-validated
/// emission, and every artifact + sidecar is snapshot-locked — emitter changes
/// require deliberate `cargo insta review`.
#[test]
fn artifact_corpus_is_deterministic_and_snapshot_locked() {
    let tmp = tempfile::tempdir().unwrap();
    let (dir_a, dir_b) = (tmp.path().join("a"), tmp.path().join("b"));

    for dir in [&dir_a, &dir_b] {
        proef()
            // The corpus directives read these env vars; scrub for determinism.
            .env_remove("PROEF_BASE_URL")
            .env_remove("RUNTIME_PHOTO")
            .args([
                "artifacts",
                "tests/features",
                "-o",
                &dir.display().to_string(),
                "--run-id",
                "proef-snapshot",
            ])
            .assert()
            .code(0);
    }

    let mut names: Vec<String> = std::fs::read_dir(&dir_a)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names.len(),
        35,
        "12 scenarios × (.hurl + .map.json) + 10 secret-referencing .vars \
         + 1 copied file asset: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "fixture.jpg"),
        "referenced file bodies ship next to the artifacts: {names:?}"
    );

    for name in &names {
        let content_a = std::fs::read(dir_a.join(name)).unwrap();
        let content_b = std::fs::read(dir_b.join(name)).unwrap();
        assert_eq!(content_a, content_b, "`{name}` must be byte-deterministic");
        if std::path::Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("jpg"))
        {
            continue; // binary asset — determinism-checked above, not snapshotted
        }
        insta::assert_snapshot!(
            format!("artifact__{name}"),
            String::from_utf8(content_a).unwrap()
        );
    }
}

#[test]
fn error_corpus_exits_2_with_stable_codes_and_snapshots() {
    let errors_dir = workspace_root().join("tests/errors");
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&errors_dir)
        .expect("tests/errors exists")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    cases.sort();
    assert!(cases.len() >= 20, "seeded corpus shrank: {}", cases.len());

    for case in cases {
        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let expected_code = format!("proef::{}", name.replace("__", "::"));
        let relative = format!("tests/errors/{name}");

        let assert = proef()
            .args(["test", &relative, "--dry-run"])
            .assert()
            .code(2);
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(
            stderr.contains(&expected_code),
            "case `{name}`: expected `{expected_code}` in:\n{stderr}"
        );
        insta::assert_snapshot!(name, stderr);
    }
}
