//! Exit codes are a contract (ADR-0009): `0` ok · `1` test failure · `2` user
//! error · `3` system error. This suite pins every code reachable at M0; the
//! taxonomy mapping for codes 1 and 3 is unit-pinned in `proef-core::error`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
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
fn unknown_flag_is_a_user_error() {
    proef().args(["doctor", "--bogus"]).assert().code(2);
}

#[test]
fn unknown_output_format_is_a_user_error() {
    // `--output` is a typed enum: a typo must exit 2, never silently degrade
    // to the human report.
    proef()
        .args(["test", "tests/features", "--output", "jsonl"])
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
    run(&["set", "apiToken", "--value", "hunter2"])
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
        .args(["secret", "set", "tok", "--value", "v"])
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
        .args(["secret", "set", "tok2", "--value", "v"])
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
    run(&["secret", "set", "tok", "--value", "v"])
        .assert()
        .code(0)
        .stderr(contains("moved to .proef-secrets.json.corrupt"));
    assert!(tmp.path().join(".proef-secrets.json.corrupt").exists());
    run(&["secret", "list"])
        .assert()
        .code(0)
        .stdout(contains("tok"));
}
