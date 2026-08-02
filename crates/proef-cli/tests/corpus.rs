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
fn tags_filter_selects_by_boolean_expression() {
    // The `--tags` argument is a boolean expression, not a CSV list. A bare
    // atom is still a valid expression, and `@` is optional.
    let selected = |expr: &str| -> String {
        let assert = proef()
            .args(["test", "tests/features", "--dry-run", "--tags", expr])
            .assert()
            .code(0);
        String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
    };
    assert!(
        selected("sync-note").contains("(1 selected by the filters)"),
        "bare atom"
    );
    assert!(
        selected("@breadth").contains("(4 selected by the filters)"),
        "leading @ is optional"
    );
    let and_not = selected("api and not breadth");
    assert!(and_not.contains("(8 selected by the filters)"), "{and_not}");
    let parens = selected("(api or search) and not breadth");
    assert!(parens.contains("(8 selected by the filters)"), "{parens}");

    // A malformed expression is a user error (exit 2), before any scenario runs.
    proef()
        .args(["test", "tests/features", "--dry-run", "--tags", "api and"])
        .assert()
        .code(2);
}

#[test]
fn flows_lists_the_corpus() {
    let assert = proef().args(["flows", "tests/features"]).assert().code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        stdout.contains("A note posted via the API appears on the board"),
        "{stdout}"
    );
    assert!(stdout.contains("@sync-note"), "{stdout}");
    assert!(
        stdout.contains("Search over records returns 200"),
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

/// `proef macros`: a pattern macro nothing binds is UNUSED; a `use:`-only macro
/// is a labelled helper, never counted unused (it composes at lower time).
#[test]
fn macros_flags_unused_pattern_macros_and_labels_helpers() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"http://x\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: F\n  Scenario: S\n    When the thing is done\n",
    )
    .unwrap();
    // usedMacro is bound and composes `helper` (use:-only); unusedMacro has a
    // `match:` no scenario says.
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  usedMacro:\n    match: the thing is done\n    steps:\n      - use: helper\n  \
         unusedMacro:\n    match: nobody says this\n    steps:\n      - hurl: |\n          \
         GET ${url:base}/x\n          HTTP 200\n  helper:\n    steps:\n      - hurl: |\n          \
         GET ${url:base}/y\n          HTTP 200\n",
    )
    .unwrap();

    let assert = Command::cargo_bin("proef")
        .unwrap()
        .current_dir(cwd.path())
        .env("NO_COLOR", "1")
        .args(["macros", "suite"])
        .assert()
        .code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        stdout.contains("unusedMacro") && stdout.contains("UNUSED"),
        "{stdout}"
    );
    assert!(
        stdout.contains("helper") && stdout.contains("use:-only helper"),
        "{stdout}"
    );
    assert!(stdout.contains("1 unused"), "{stdout}");
}

/// The built-in `expect*` shape macros bind generic response-shape prose and
/// lower to the matching hurl type predicates, merged into the preceding
/// request. Emitting artifacts exercises the whole chain — bind → lower →
/// resolve `${path}` → merge asserts → emit → parse.
#[test]
fn builtin_shape_macros_lower_to_hurl_type_predicates() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"http://x\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  fetchRecord:\n    match: the record is fetched\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/record\n          HTTP 200\n",
    )
    .unwrap();
    // A request step, then three built-in shape assertions that merge into it.
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: Shape\n  Scenario: Response shape holds\n    \
         When the record is fetched\n    \
         Then the value at \"$.id\" is a uuid\n    \
         And the value at \"$.name\" is a string\n    \
         And the value at \"$.tags\" is a non-empty list\n",
    )
    .unwrap();

    // Dry-run validates the whole pipeline without touching the network.
    Command::cargo_bin("proef")
        .unwrap()
        .current_dir(cwd.path())
        .env("NO_COLOR", "1")
        .args(["test", "suite", "--dry-run"])
        .assert()
        .code(0);

    // Artifacts prove `${path}` resolved and the asserts merged into the entry.
    let out = cwd.path().join("out");
    Command::cargo_bin("proef")
        .unwrap()
        .current_dir(cwd.path())
        .env("NO_COLOR", "1")
        .args([
            "artifacts",
            "suite",
            "-o",
            &out.display().to_string(),
            "--run-id",
            "shape",
        ])
        .assert()
        .code(0);

    let hurl = std::fs::read_dir(&out)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "hurl"))
        .expect("an emitted .hurl artifact");
    let text = std::fs::read_to_string(hurl).unwrap();
    assert!(text.contains("jsonpath \"$.id\" isUuid"), "{text}");
    assert!(text.contains("jsonpath \"$.name\" isString"), "{text}");
    assert!(text.contains("jsonpath \"$.tags\" isList"), "{text}");
    assert!(text.contains("jsonpath \"$.tags\" count > 0"), "{text}");
}

/// `test --dry-run --sarif`: validation diagnostics serialize to a SARIF 2.1.0
/// log (code → ruleId, severity → level, byte span → region). The export is
/// additive — the dry-run still exits 2 on a broken case.
#[test]
fn dry_run_sarif_export_serializes_diagnostics() {
    let cwd = tempfile::tempdir().unwrap();
    let sarif = cwd.path().join("out.sarif");
    proef()
        .args([
            "test",
            "tests/errors/bind__unbound_step",
            "--dry-run",
            "--sarif",
            &sarif.display().to_string(),
        ])
        .assert()
        .code(2);
    let text = std::fs::read_to_string(&sarif).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["version"], "2.1.0", "{text}");
    let result = &json["runs"][0]["results"][0];
    assert_eq!(result["ruleId"], "proef::bind::unbound_step", "{text}");
    assert_eq!(result["level"], "error", "{text}");
    assert!(
        result["locations"][0]["physicalLocation"]["region"]["byteOffset"].is_number(),
        "{text}"
    );
}

/// Real execution against the corpus with no secret available is a user fault
/// (exit 2), caught before any request. `${url:base}` resolves to the
/// `proef.toml` default, so the unbound `${secret:apiToken}` is the binding gap.
/// Runs from a temp CWD with the target passed absolutely: the secret store is
/// `.proef-secrets.json` in the *working directory* (CWD-relative, no env
/// override), so a developer's own stored `apiToken` must not be able to mask
/// this — the clean CWD guarantees it.
#[test]
fn execution_without_secret_is_a_user_error() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::copy(
        workspace_root().join("proef.toml"),
        cwd.path().join("proef.toml"),
    )
    .unwrap();
    Command::cargo_bin("proef")
        .unwrap()
        .current_dir(cwd.path())
        .env("NO_COLOR", "1")
        .env_remove("PROEF_BASE_URL")
        .env_remove("PROEF_SECRET_APITOKEN")
        .arg("test")
        .arg(workspace_root().join("tests/features"))
        .assert()
        .code(2);
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
            .env_remove("RUNTIME_ATTACHMENT")
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
