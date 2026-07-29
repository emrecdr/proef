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
