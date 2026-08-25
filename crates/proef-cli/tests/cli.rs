//! Exit codes are a contract (ADR-0009): `0` ok · `1` test failure · `2` user
//! error · `3` system error. This suite pins every code reachable at M0; the
//! taxonomy mapping for codes 1 and 3 is unit-pinned in `proef-core::error`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

fn proef() -> Command {
    Command::cargo_bin("proef").unwrap()
}

#[test]
fn version_exits_zero() {
    proef()
        .arg("--version")
        .assert()
        .code(0)
        .stdout(contains("proef"));
}

#[test]
fn unknown_subcommand_is_a_user_error() {
    proef().arg("frobnicate").assert().code(2);
}

#[test]
fn lsp_help_exits_zero() {
    proef()
        .args(["lsp", "--help"])
        .assert()
        .code(0)
        .stdout(contains("language server"));
}

#[test]
fn unknown_flag_is_a_user_error() {
    proef().args(["doctor", "--bogus"]).assert().code(2);
}

#[test]
fn unknown_machine_format_is_a_user_error() {
    // `--format` is a typed enum: a typo must exit 2, never silently degrade
    // to the human report.
    proef()
        .args(["test", "tests/features", "--format", "jsonl"])
        .assert()
        .code(2)
        .stderr(contains("json"));
    // The listing commands speak `json` alone, and their enum says so —
    // `tap` in their help while the runtime rejected it was the old shared
    // enum lying about a quarter of the surface.
    proef()
        .args(["flows", "tests/features", "--format", "tap"])
        .assert()
        .code(2)
        .stderr(contains("json"));
}

#[test]
fn no_arguments_shows_help_as_a_user_error() {
    proef().assert().code(2).stderr(contains("Usage"));
}

#[test]
fn doctor_reports_engine_contributed_checks() {
    // Acceptance (M0): doctor reports native-library status via the
    // engine-contributed check hook — the first proof the capability hook works.
    proef()
        .arg("doctor")
        .assert()
        .code(0)
        .stdout(contains("engine `hurl`"));
}

#[test]
fn secret_lifecycle_set_list_rm() {
    // Isolated store (cwd) and key (config dir) — never the developer's.
    let tmp = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let mut cmd = proef();
        cmd.current_dir(tmp.path())
            .env("PROEF_CONFIG_DIR", tmp.path().join("cfg"))
            .args(["secret"])
            .args(args);
        cmd
    };
    run(&["set", "apiToken", "--stdin"])
        .write_stdin("hunter2")
        .assert()
        .code(0);
    run(&["list"]).assert().code(0).stdout(contains("apiToken"));
    run(&["rm", "apiToken"]).assert().code(0);
    run(&["list"])
        .assert()
        .code(0)
        .stdout(contains("no secrets stored"));
    // Removing an absent name is a user error, not a silent success.
    run(&["rm", "apiToken"]).assert().code(2);
}

#[test]
fn proef_key_env_override_replaces_the_key_file() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("cfg");
    // 32 zero bytes, base64 — a valid key supplied via env only.
    let key_b64 = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    proef()
        .current_dir(tmp.path())
        .env("PROEF_CONFIG_DIR", &cfg)
        .env("PROEF_KEY", key_b64)
        .args(["secret", "set", "tok", "--stdin"])
        .write_stdin("v")
        .assert()
        .code(0);
    assert!(
        !cfg.join("proef").join("keys").join("default.key").exists(),
        "PROEF_KEY must be used as-is — no key file side effect"
    );
    // A set-but-invalid key is an error, never a silent fallthrough.
    proef()
        .current_dir(tmp.path())
        .env("PROEF_CONFIG_DIR", &cfg)
        .env("PROEF_KEY", "not-base64!!")
        .args(["secret", "set", "tok2", "--stdin"])
        .write_stdin("v")
        .assert()
        .code(2)
        .stderr(contains("PROEF_KEY"));
}

#[test]
fn corrupt_store_is_recovered_by_set_and_flagged_by_doctor() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(".proef-secrets.json"), "{ not json").unwrap();
    let run = |args: &[&str]| {
        let mut cmd = proef();
        cmd.current_dir(tmp.path())
            .env("PROEF_CONFIG_DIR", tmp.path().join("cfg"))
            .args(args);
        cmd
    };
    // doctor names the problem (exit 3: broken environment)…
    run(&["doctor"])
        .assert()
        .code(3)
        .stdout(contains("corrupt"));
    // …and `secret set` moves the wreck aside and proceeds.
    run(&["secret", "set", "tok", "--stdin"])
        .write_stdin("v")
        .assert()
        .code(0)
        .stderr(contains("moved to .proef-secrets.json.corrupt"));
    assert!(tmp.path().join(".proef-secrets.json.corrupt").exists());
    run(&["secret", "list"])
        .assert()
        .code(0)
        .stdout(contains("tok"));
}

// EPIPE is a POSIX signal-pipe behavior and `head` may be absent on Windows;
// the fix under test (render.rs's `errln!`) is cross-platform even though
// this reproduction is unix-only.
#[cfg(unix)]
#[test]
fn diagnostics_do_not_panic_on_a_closed_stderr_pipe() {
    use std::process::{Command, Stdio};
    // `head -c0` reads nothing then exits, closing the read end of the pipe so
    // the next stderr write from proef gets EPIPE. The diagnostic renderer must
    // swallow it (exit with the normal error code), never panic with 101.
    let bin = assert_cmd::cargo::cargo_bin("proef");
    // Point at the whole seeded broken corpus so validation streams many
    // diagnostics to stderr (repo-relative: tests/errors/ fails dry-run by
    // design) — enough bytes that the write reliably lands after the reader
    // closes the pipe, rather than racing a single small diagnostic.
    let repo_root = env!("CARGO_MANIFEST_DIR"); // crates/proef-cli
    let errors_dir = std::path::Path::new(repo_root).join("../../tests/errors");

    let mut proef = Command::new(&bin)
        .args(["test", "--dry-run"])
        .arg(&errors_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Consume nothing then drop the reader to close the pipe early.
    let mut head = Command::new("head")
        .args(["-c", "0"])
        .stdin(proef.stderr.take().unwrap())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _ = head.wait();
    let status = proef.wait().unwrap();
    // Pins two things at once: no panic (101), and the diagnostic path was
    // actually reached — the seeded broken corpus fails `--dry-run`
    // validation by design, which is the contracted user-error exit (2).
    assert_eq!(
        status.code(),
        Some(2),
        "expected the contracted user-error exit, not a panic or an early abort"
    );
}

// The stdout mirror of the stderr test above: `head -c0` reads nothing and
// exits, closing the read end, so every later stdout write gets EPIPE.
// `outln!`'s `BrokenPipe` guard must swallow it — `proef … | head` ends the
// pipeline on purpose — so this must stay the command's ordinary success
// exit, never the system-error exit a genuine stdout write failure now
// produces.
#[cfg(unix)]
#[test]
fn flows_does_not_report_a_system_error_on_a_closed_stdout_pipe() {
    use std::process::{Command, Stdio};
    let bin = assert_cmd::cargo::cargo_bin("proef");
    // Absolute, like the PROEF_ENV test below: nextest's cwd for this binary
    // is the crate manifest dir, which has no `tests/features` of its own.
    let repo_root = env!("CARGO_MANIFEST_DIR"); // crates/proef-cli
    let features_dir = std::path::Path::new(repo_root).join("../../tests/features");

    let mut proef = Command::new(&bin)
        .arg("flows")
        .arg(&features_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Consume nothing then drop the reader to close the pipe early — `flows`
    // over the reference corpus (tests/features) prints one line per
    // scenario plus a summary line, enough bytes that the write reliably
    // lands after the reader closes, rather than racing a single short line.
    let mut head = Command::new("head")
        .args(["-c", "0"])
        .stdin(proef.stdout.take().unwrap())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _ = head.wait();
    let status = proef.wait().unwrap();
    assert_eq!(
        status.code(),
        Some(0),
        "expected the ordinary success exit for a closed stdout pipe, not a system error"
    );
}

// /dev/full accepts opens and fails every write with ENOSPC — a full disk
// without needing one. Linux-only: macOS has no such device, and no
// portable substitute forces a write failure (a read-only or closed stdout
// both exit 0). The mechanism itself is pinned portably in render.rs.
//
// Uses std::process::Command, not assert_cmd::Command: assert_cmd's builder
// has no stdio-redirection setter of its own (its `.stdout()` asserts
// against an already-captured child's output, after the fact) and its
// `.output()` fixes its own pipes, so neither lets a caller hand the child
// a pre-opened handle like `/dev/full`.
#[cfg(target_os = "linux")]
#[test]
fn a_failed_stdout_write_is_a_system_error() {
    use std::process::{Command, Stdio};
    // Absolute, like the PROEF_ENV test above: nextest's cwd for this binary
    // is the crate manifest dir, which has no `tests/features` of its own.
    let repo_root = env!("CARGO_MANIFEST_DIR"); // crates/proef-cli
    let features_dir = std::path::Path::new(repo_root).join("../../tests/features");
    let devfull = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("/dev/full is a standard Linux device");
    let bin = assert_cmd::cargo::cargo_bin("proef");
    let output = Command::new(&bin)
        .arg("flows")
        .arg(&features_dir)
        .stdout(devfull)
        .stderr(Stdio::piped())
        .output()
        .expect("proef must spawn and run to completion");
    // A bare exit-3 check can't tell "the funnel upgraded 0 -> 3" apart from
    // "flows failed on its own for an unrelated reason", so pin the funnel's
    // own message too.
    assert_eq!(
        output.status.code(),
        Some(3),
        "a stdout write failure must be reported as the system-error exit"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot write to stdout"),
        "expected the exit funnel's own diagnostic on stderr, got: {stderr}"
    );
}

// A non-UTF-8 PROEF_ENV must not be silently treated as unset — running
// against the wrong environment is exactly the "reports the wrong cause"
// failure this contract exists to remove.
#[cfg(unix)]
#[test]
fn a_non_utf8_env_var_is_a_user_error() {
    use std::os::unix::ffi::OsStrExt as _;
    // Absolute, like the EPIPE reproduction above: nextest's cwd for this
    // binary is the crate manifest dir, which has no `tests/features` of its
    // own, so a relative path would fail on path resolution instead of the
    // env var — a different exit-2 cause and a vacuous test.
    let repo_root = env!("CARGO_MANIFEST_DIR"); // crates/proef-cli
    let features_dir = std::path::Path::new(repo_root).join("../../tests/features");
    let bad = std::ffi::OsStr::from_bytes(&[0x66, 0xff, 0x6f]);
    proef()
        .arg("flows")
        .arg(&features_dir)
        .env("PROEF_ENV", bad)
        .assert()
        .code(2)
        .stderr(contains("PROEF_ENV"));
}

// `fmt` promises to normalize hurl blocks only, never a file's line
// endings. `normalize_pack` unit tests pin the string it returns, but the
// user-visible contract is what `fmt --check` reports and what lands on
// disk — a regression could keep those unit tests green while still
// corrupting bytes the command actually writes. These two exercise the
// command end to end.
#[test]
fn fmt_check_reports_a_canonical_crlf_pack_as_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let pack_path = tmp.path().join("pack.yaml");
    let canonical_crlf = "macros:\r\n  ping:\r\n    match: I ping\r\n    steps:\r\n      - hurl: |\r\n          GET http://x/a\r\n          HTTP 200\r\n";
    std::fs::write(&pack_path, canonical_crlf).unwrap();

    proef()
        .arg("fmt")
        .arg(&pack_path)
        .arg("--check")
        .assert()
        .code(0)
        .stdout(contains("all pack blocks already canonical"));
    assert_eq!(
        std::fs::read(&pack_path).unwrap(),
        canonical_crlf.as_bytes(),
        "--check must never rewrite the file"
    );
}

#[test]
fn fmt_rewrites_a_dirty_crlf_pack_preserving_crlf_throughout() {
    let tmp = tempfile::tempdir().unwrap();
    let pack_path = tmp.path().join("pack.yaml");
    let dirty_crlf = "# comment stays\r\nmacros:\r\n  m:\r\n    steps:\r\n      - hurl: |\r\n          GET http://x   \r\n\r\n\r\n          HTTP 200\r\n\r\n    match: keep\r\n";
    std::fs::write(&pack_path, dirty_crlf).unwrap();

    // --check reports the dirty file and exits 1, without rewriting it.
    proef()
        .arg("fmt")
        .arg(&pack_path)
        .arg("--check")
        .assert()
        .code(1)
        .stdout(contains("needs formatting"));
    assert_eq!(
        std::fs::read(&pack_path).unwrap(),
        dirty_crlf.as_bytes(),
        "--check must never rewrite the file"
    );

    // Without --check, the file is rewritten in place.
    proef()
        .arg("fmt")
        .arg(&pack_path)
        .assert()
        .code(0)
        .stdout(contains("formatted:"));

    // Byte-level, not `contains("\r\n")`: a half-converted file can still
    // contain CRLF pairs while also containing a bare LF elsewhere.
    let rewritten = std::fs::read(&pack_path).unwrap();
    assert!(
        rewritten.windows(2).any(|w| w == b"\r\n"),
        "expected CRLF pairs to survive the rewrite: {rewritten:?}"
    );
    for (i, &byte) in rewritten.iter().enumerate() {
        if byte == b'\n' {
            assert!(
                i > 0 && rewritten[i - 1] == b'\r',
                "bare LF at byte {i}, not part of a CRLF pair: {rewritten:?}"
            );
        }
    }

    // The rewritten file re-checks clean.
    proef()
        .arg("fmt")
        .arg(&pack_path)
        .arg("--check")
        .assert()
        .code(0)
        .stdout(contains("all pack blocks already canonical"));
}

/// `fmt` took an explicit file on trust, so it rewrote whatever it was pointed
/// at — `proef fmt src/main.rs` stripped trailing whitespace from Rust source
/// and reported success. A formatter that writes without parsing turns a
/// mistyped path into a silent edit.
#[test]
fn fmt_refuses_a_file_that_is_not_a_pack() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("notes.rs");
    let original = "fn main() {   \n    println!(\"hi\");   \n}\n";
    std::fs::write(&source, original).unwrap();

    proef()
        .arg("fmt")
        .arg(&source)
        // No path fragment in the expectation: the message interpolates a path,
        // and its separator differs by platform.
        .assert()
        .code(2)
        .stderr(contains("is not a pack file"));
    assert_eq!(
        std::fs::read(&source).unwrap(),
        original.as_bytes(),
        "a refused file must come back byte-for-byte"
    );
}

/// `fmt`'s promise is hurl blocks; its module doc says the YAML skeleton,
/// comments included, is never touched. It trimmed every line anyway, so a
/// trailing space in a comment failed `--check` on a pack whose blocks were
/// already canonical — a CI red an author cannot explain from the documented
/// scope, which is the same defect the line-ending fix removed.
#[test]
fn fmt_check_ignores_trailing_whitespace_outside_a_hurl_block() {
    let tmp = tempfile::tempdir().unwrap();
    let pack_path = tmp.path().join("pack.yaml");
    let canonical_blocks_dirty_skeleton = "# a comment with a trailing space   \nmacros:\n  ping:\n    match: I ping   \n    steps:\n      - hurl: |\n          GET http://x/a\n          HTTP 200\n";
    std::fs::write(&pack_path, canonical_blocks_dirty_skeleton).unwrap();

    proef()
        .arg("fmt")
        .arg(&pack_path)
        .arg("--check")
        .assert()
        .code(0)
        .stdout(contains("all pack blocks already canonical"));
    assert_eq!(
        std::fs::read(&pack_path).unwrap(),
        canonical_blocks_dirty_skeleton.as_bytes(),
        "the skeleton is not this command's to normalize"
    );
}

/// `doctor` reports a missing pack schema. `init` installs it automatically,
/// but the other half of that finding — noticing when it is absent — never
/// shipped, so a suite whose editor completion was silently off had nothing
/// telling it so. A `Warn`, not a `Fail`: it costs autocomplete, not a run.
#[test]
fn doctor_reports_a_missing_pack_schema_and_notices_when_it_returns() {
    let tmp = tempfile::tempdir().unwrap();
    proef().current_dir(tmp.path()).arg("init").assert().code(0);

    // Freshly scaffolded: `init` installed it.
    proef()
        .current_dir(tmp.path())
        .arg("doctor")
        .assert()
        .code(0)
        .stdout(contains("pack schema"))
        .stdout(contains("proef-pack.schema.json present"));

    std::fs::remove_file(tmp.path().join("suite/packs/proef-pack.schema.json")).unwrap();
    proef()
        .current_dir(tmp.path())
        .arg("doctor")
        .assert()
        .code(0) // a warning never gates the environment verdict
        .stdout(contains("missing"))
        .stdout(contains("proef schema --add-to"));
}

/// `doctor` runs outside a project: no config and no suite is not a finding.
#[test]
fn doctor_outside_a_project_reports_no_suite_rather_than_failing() {
    let tmp = tempfile::tempdir().unwrap();
    proef()
        .current_dir(tmp.path())
        .arg("doctor")
        .assert()
        .code(0)
        .stdout(contains("no suite configured"));
}

/// …but a `proef.toml` that is *present and broken* is its first finding.
///
/// Leniency is about a config being absent. When the discovery arm became a
/// silent `unwrap_or_default`, `doctor` ran every check against a configuration
/// the project never wrote and reported "all checks passed", exit 0 — the one
/// answer a diagnosis tool must not give about a file sitting right there. The
/// error reaches the exit code via a row, because that is what CI reads.
#[test]
fn doctor_fails_on_a_discovered_config_that_does_not_parse() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("proef.toml"), "runs-dir = [[[ &&&\n").unwrap();
    proef()
        .current_dir(tmp.path())
        .arg("doctor")
        .assert()
        .code(3)
        .stdout(contains("[FAIL] proef.toml"))
        .stdout(contains("environment is not ready"));
}

/// A secret must never be passable in argv, where `ps` exposes it. `--value` is
/// gone rather than deprecated, and the flag that replaces it reads stdin.
#[test]
fn secret_set_takes_no_value_on_the_command_line() {
    let tmp = tempfile::tempdir().unwrap();
    proef()
        .current_dir(tmp.path())
        .env("PROEF_CONFIG_DIR", tmp.path().join("cfg"))
        .args(["secret", "set", "tok", "--value", "hunter2"])
        .assert()
        .failure()
        .stderr(contains("--value"));

    // The supported script path, and the stored value must not keep the
    // newline the pipe added.
    proef()
        .current_dir(tmp.path())
        .env("PROEF_CONFIG_DIR", tmp.path().join("cfg"))
        .args(["secret", "set", "tok", "--stdin"])
        .write_stdin("hunter2\n")
        .assert()
        .code(0);
    proef()
        .current_dir(tmp.path())
        .env("PROEF_CONFIG_DIR", tmp.path().join("cfg"))
        .args(["secret", "list"])
        .assert()
        .code(0)
        .stdout(contains("tok"));
}

/// `proef.toml` is found by searching *up*, so a config beside the suite is
/// invisible from the repository root — a layout an adopting team planned and
/// had to abandon. `--config` names the file instead.
///
/// The missing-file half matters as much as the working half: discovery finding
/// nothing legitimately means "no project, use defaults", but a *named* file
/// that is not there is a typo, and answering it with a silently unconfigured
/// run is the failure mode that produces `${url:…}` unset and nothing saying why.
#[test]
fn config_names_a_file_that_discovery_would_never_find() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tests/proef")).unwrap();
    std::fs::create_dir_all(root.join("tests/features/packs")).unwrap();
    // Below the working directory, so the upward search cannot reach it.
    // `suite` is spelled relative to *this file*, like every other written path
    // (`ProjectConfig::resolve`) — which is what lets the same config drive the
    // run from the root and from a subdirectory alike.
    std::fs::write(
        root.join("tests/proef/proef.toml"),
        "[run]\nsuite = \"../features\"\n[url]\nbase = \"http://127.0.0.1:1\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/features/packs/api.yaml"),
        "macros:\n  h:\n    match: the service is up\n    steps:\n      - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/features/a.feature"),
        "Feature: F\n  Scenario: S\n    When the service is up\n",
    )
    .unwrap();

    // Without it: `${url:base}` is unset, because no config was found.
    proef()
        .current_dir(root)
        .args(["test", "tests/features", "--dry-run"])
        .assert()
        .failure();

    proef()
        .current_dir(root)
        .args(["test", "--dry-run", "--config", "tests/proef/proef.toml"])
        .assert()
        .code(0);

    // A named file that is not there is a user error, never a quiet default.
    proef()
        .current_dir(root)
        .args(["test", "--dry-run", "--config", "tests/proef/nope.toml"])
        .assert()
        .code(2)
        .stderr(contains("is not a file"));
}

/// A project is where its `proef.toml` is, not where the shell is.
///
/// Discovery searches *up*, so running from a subdirectory finds the same
/// config — but every path it wrote used to resolve against the working
/// directory, so `[run] suite = "features"` became `sub/features` and reported
/// "neither a feature file nor a directory". `[run] fragments` was the one key
/// already anchored on the config, which meant two keys in one table quietly
/// meant two different roots.
#[test]
fn a_subdirectory_run_resolves_the_same_project_as_the_root_run() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("features/packs")).unwrap();
    std::fs::create_dir_all(root.join("hurl")).unwrap();
    std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
    std::fs::write(
        root.join("proef.toml"),
        "[run]\nsuite = \"features\"\nfragments = \"hurl\"\n\
         [url]\nbase = \"http://127.0.0.1:1\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("hurl/api.hurl"),
        "# @proef health\nGET {{base}}/health\nHTTP 200\n",
    )
    .unwrap();
    std::fs::write(
        root.join("features/packs/api.yaml"),
        "macros:\n  h:\n    match: the service is up\n    steps:\n      - ref: health\n        bind:\n          base: ${url:base}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("features/a.feature"),
        "Feature: F\n  Scenario: S\n    When the service is up\n",
    )
    .unwrap();

    // The reference run, from the directory holding the config.
    proef()
        .current_dir(root)
        .args(["test", "--dry-run"])
        .assert()
        .code(0);

    // …and from two directories down, where the upward search finds the very
    // same file. Both keys have to resolve, not just the fragment root.
    for cwd in ["sub", "sub/deeper"] {
        proef()
            .current_dir(root.join(cwd))
            .args(["test", "--dry-run"])
            .assert()
            .code(0);
    }
}

/// An output path proef was told to write includes the directories it needs.
///
/// `--junit` and `report -o` are used mostly in CI, where the target path
/// routinely does not exist yet — a fresh workspace, `target/reports/`. proef
/// created parents for `artifacts -o` and the run directory but not for these,
/// so there was no rule, and the two paths used most in CI were on the failing
/// side. Every adopter paid the same `mkdir -p`.
///
/// The comparison that settles it: `pytest --junitxml`, `jest-junit`,
/// `cargo-nextest`'s `JUnit` store and the `hurl` proef embeds all create them.
#[test]
fn an_output_path_creates_the_directories_it_needs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tests/features/packs")).unwrap();
    std::fs::write(
        root.join("proef.toml"),
        "[run]\nsuite = \"tests/features\"\n[url]\nbase = \"http://127.0.0.1:1\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/features/packs/api.yaml"),
        "macros:\n  m:\n    match: it runs\n    steps:\n      - hurl: |\n          GET ${url:base}/x\n          HTTP 200\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/features/a.feature"),
        "Feature: F\n  Scenario: S\n    When it runs\n",
    )
    .unwrap();

    // The run itself fails (nothing listens on port 1) — the point is that the
    // report is still written, rather than the missing directory replacing the
    // run's own verdict with a path error.
    proef()
        .current_dir(root)
        .args(["test", "--junit", "reports/nested/junit.xml"])
        .assert()
        .failure();
    assert!(
        root.join("reports/nested/junit.xml").is_file(),
        "--junit must create the directories its path names"
    );

    // A validation failure, so SARIF has something to write.
    std::fs::write(
        root.join("tests/features/bad.feature"),
        "Feature: F\n  Scenario: S\n    When nothing matches this\n",
    )
    .unwrap();
    proef()
        .current_dir(root)
        .args(["test", "--dry-run", "--sarif", "sarif/nested/out.sarif"])
        .assert()
        .code(2);
    assert!(
        root.join("sarif/nested/out.sarif").is_file(),
        "--sarif must create the directories its path names"
    );
}

/// A suite with one exclusive scenario and one ordinary one, plus whatever
/// `[run] exclusive-tags` the caller wants to try against it.
fn exclusive_tags_project(expression: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("features/packs")).unwrap();
    std::fs::write(
        root.join("proef.toml"),
        format!("[run]\nsuite = \"features\"\nexclusive-tags = \"{expression}\"\n"),
    )
    .unwrap();
    std::fs::write(
        root.join("features/packs/p.yaml"),
        "macros:\n  ping:\n    match: I ping\n    steps:\n      - hurl: |\n          GET http://127.0.0.1:1/health\n          HTTP 200\n",
    )
    .unwrap();
    std::fs::write(
        root.join("features/a.feature"),
        "Feature: F\n  @solo\n  Scenario: alone\n    When I ping\n\n  Scenario: shared\n    When I ping\n",
    )
    .unwrap();
    tmp
}

/// `--dry-run` validates the settings a run would use — and `[run]
/// exclusive-tags` was exempt, so a malformed expression exited 2 from
/// `proef test` and passed `dry-run OK … 0 warning(s)` from the gate CI runs.
#[test]
fn dry_run_refuses_a_malformed_exclusive_tags_expression() {
    let tmp = exclusive_tags_project("@solo and (");
    for args in [
        vec!["test", "--dry-run"],
        // The real run already refused it; both paths must agree.
        vec!["test"],
    ] {
        proef()
            .current_dir(tmp.path())
            .args(&args)
            .assert()
            .code(2)
            .stderr(contains(
                "[run] exclusive-tags is not a valid tag expression",
            ));
    }
}

/// A *well-formed* expression matching no scenario is the typo that defeats the
/// key's own design rationale: isolation degrades, every scenario rejoins the
/// shared pool, and the interference reads as flakiness rather than as a
/// misspelled setting.
#[test]
fn an_exclusive_tags_expression_matching_nothing_is_reported() {
    let tmp = exclusive_tags_project("@soloz");
    proef()
        .current_dir(tmp.path())
        .args(["test", "--dry-run"])
        .assert()
        .code(0)
        .stderr(contains("matches no scenario"));
}

/// …and the two cases it must *not* fire on: a spelling that matches, and a
/// `--tags` filter that removed the matches from this particular run. The
/// second is why the verdict is taken over every scenario the suite loaded
/// rather than over the ones selected — otherwise every filtered run would
/// report a broken setting.
#[test]
fn a_matching_exclusive_expression_is_silent_even_when_tags_filter_it_out() {
    let tmp = exclusive_tags_project("@solo");
    for args in [
        vec!["test", "--dry-run"],
        vec!["test", "--dry-run", "--tags", "not @solo"],
    ] {
        proef()
            .current_dir(tmp.path())
            .args(&args)
            .assert()
            .code(0)
            .stderr(contains("matches no scenario").not());
    }
}
