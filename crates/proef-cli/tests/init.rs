//! `proef init` scaffolds a working suite: the files it writes must dry-run
//! green, it must never overwrite authored work, and running it twice must be
//! a no-op.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;
use proef_fixture::Fixture;

fn proef(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("proef").unwrap();
    cmd.current_dir(dir).env("NO_COLOR", "1");
    cmd
}

/// The load-bearing test: whatever `init` writes must actually validate. This
/// is what stops the scaffold and the tutorial from silently diverging.
#[test]
fn scaffold_dry_runs_green() {
    let tmp = tempfile::tempdir().unwrap();
    proef(tmp.path()).arg("init").assert().code(0);
    proef(tmp.path())
        .args(["test", "--dry-run"])
        .assert()
        .code(0)
        .stdout(contains("dry-run OK"));
}

/// Every file `init` reports in its trailing count is also named on its own
/// line. The schema JSON was written silently, so the first output a new user
/// reads listed four files and then claimed five.
#[test]
fn init_announces_every_file_it_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let assert = proef(tmp.path()).arg("init").assert().code(0);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    // Match the indented per-file marker, never a path fragment: Windows
    // renders these as `.\proef.toml`, so a separator-bearing needle counts
    // zero there and the assertion passes vacuously. The two-space indent is
    // what distinguishes a per-file line from the flush-left summary.
    let announced = stdout
        .lines()
        .filter(|l| l.starts_with("  created "))
        .count();
    assert!(
        stdout.contains(&format!("created {announced} file(s)")),
        "announced {announced} file(s) but the count line disagrees:\n{stdout}"
    );
    assert!(stdout.contains("proef-pack.schema.json"), "{stdout}");
}

/// The note must not appear on a suite that reached a real verdict.
///
/// **What this pins, precisely:** the `PROEF_BASE_URL`-override conjunct, end
/// to end through the binary — an operator who named a target is never told
/// their target is a placeholder.
///
/// **What it deliberately does not pin:** the reachability conjunct. Faithfully
/// reproducing that regression needs the starter port *closed* while the config
/// still holds the starter literal, and no test can guarantee a port is closed.
/// `exec::tests::the_unconfigured_target_note_needs_an_unreachable_run` covers
/// it on the predicate directly — verified RED. Said plainly here because this
/// test passes with the fix reverted, and a reader could otherwise mistake it
/// for cover it does not give.
#[test]
fn the_unconfigured_target_note_stays_quiet_when_the_target_answered() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    // Verbatim the line GETTING-STARTED teaches — no `init` involved.
    std::fs::write(
        cwd.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL:-http://127.0.0.1:8787}\"\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/case.feature"),
        "Feature: Mine\n  Scenario: teapot\n    When the service answers 418\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  teapot:\n    match: the service answers 418\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 418\n",
    )
    .unwrap();

    // PROEF_BASE_URL points at the live fixture, so the run reaches a real
    // assertion and fails on the status — a genuine verdict, not a placeholder.
    Command::cargo_bin("proef")
        .unwrap()
        .current_dir(cwd.path())
        .env("NO_COLOR", "1")
        .env("PROEF_BASE_URL", &fixture.base_url)
        .args(["test", "suite"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("scaffold").not())
        .stderr(predicate::str::contains("starter value").not());
}

/// Running init twice creates nothing the second time.
#[test]
fn init_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    proef(tmp.path()).arg("init").assert().code(0);
    proef(tmp.path())
        .arg("init")
        .assert()
        .code(0)
        .stdout(contains("nothing to create"));
}

/// An authored file is never overwritten.
#[test]
fn init_never_overwrites_an_existing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("proef.toml");
    std::fs::write(&config, "# authored by hand\n").unwrap();
    proef(tmp.path()).arg("init").assert().code(0);
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        "# authored by hand\n",
        "init overwrote an existing file"
    );
}

/// A hand-authored pack is never rewritten, even by the schema-install step:
/// a skipped pack must come out byte-identical, not just get a pass on the
/// create/overwrite loop and then get modelined anyway.
#[test]
fn init_never_rewrites_an_existing_pack_via_schema_install() {
    let tmp = tempfile::tempdir().unwrap();
    let packs_dir = tmp.path().join("suite/packs");
    std::fs::create_dir_all(&packs_dir).unwrap();
    let pack = packs_dir.join("api.yaml");
    let original = "macros:\n  mine:\n    match: my own step\n    steps: []\n";
    std::fs::write(&pack, original).unwrap();
    proef(tmp.path()).arg("init").assert().code(0);
    assert_eq!(
        std::fs::read_to_string(&pack).unwrap(),
        original,
        "init modified a pre-existing pack file"
    );
}

/// The schema install runs as part of init, so editor completion works on the
/// first run without discovering a flag.
#[test]
fn scaffold_carries_the_pack_schema_and_modeline() {
    let tmp = tempfile::tempdir().unwrap();
    proef(tmp.path()).arg("init").assert().code(0);
    assert!(
        tmp.path()
            .join("suite/packs/proef-pack.schema.json")
            .exists(),
        "schema file missing from the scaffold"
    );
    let pack = std::fs::read_to_string(tmp.path().join("suite/packs/api.yaml")).unwrap();
    assert!(
        pack.contains("yaml-language-server: $schema=./proef-pack.schema.json"),
        "pack modeline missing: {pack}"
    );
}

/// A passing dry-run names the next command. Every failure path already names
/// a remedy; the success path is where a new user decides whether to continue.
/// `init` scaffolds into "." here, so `[run] suite` picks the suite up with no
/// path argument — the printed `next: proef test` must actually run clean,
/// not merely be printed (that gap is why the bug shipped: see the two tests
/// below for the cases where a bare `next: proef test[--dry-run]` does NOT
/// work from the caller's cwd).
#[test]
fn dry_run_success_names_the_next_command() {
    let tmp = tempfile::tempdir().unwrap();
    proef(tmp.path()).arg("init").assert().code(0);
    proef(tmp.path())
        .args(["test", "--dry-run"])
        .assert()
        .code(0)
        .stdout(contains("dry-run OK"))
        .stdout(contains("next: proef test"));

    // The nudge is a bare `proef test` (no `--dry-run`): with the scaffold's
    // `[run] suite = "suite"`, it needs no path argument — running it must not
    // fail at argument/config resolution (a live target for the real network
    // call is a separate concern from what this nudge is pinning).
    proef(tmp.path())
        .args(["test", "--dry-run"])
        .assert()
        .code(0);
}

/// `proef init <dir>` scaffolds into a subdirectory, but `proef.toml`
/// discovery only walks *up* from the working directory, never down into a
/// path argument (`crate::config::find_config`) — so a bare
/// `proef test --dry-run`, run from the same cwd `init` ran in, cannot find
/// the scaffolded config. Before the fix this was exactly the printed nudge,
/// and running it failed with "no path given and no default suite found".
/// Also pins that the nudge warns about the scaffold's placeholder routes —
/// nothing serves `/search`, so a chained `init` → dry-run → real `test`
/// otherwise walks a newcomer into a failing run with no signpost.
#[test]
fn init_into_a_subdirectory_prints_a_working_next_command() {
    let tmp = tempfile::tempdir().unwrap();
    let assert = proef(tmp.path())
        .args(["init", "api-tests"])
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        out.contains(
            "next: cd api-tests && proef test --dry-run  (then point ${url:base} at your API — the scaffold's routes are placeholders)"
        ),
        "nudge must `cd` into the scaffolded directory and warn about placeholder routes: {out}"
    );

    // Run it for real — `current_dir` is `cd api-tests &&`'s equivalent for a
    // directly-spawned (non-shell) child process.
    proef(&tmp.path().join("api-tests"))
        .args(["test", "--dry-run"])
        .assert()
        .code(0);
}

/// `--dry-run`'s "next" nudge must echo an explicit suite path — a bare
/// `proef test` only rediscovers a *defaulted* path (`[run] suite`/`tests/`)
/// on its own. Before the fix, `proef test mysuite --dry-run` in a project
/// with no `[run] suite` printed a bare `next: proef test`, which then failed
/// with "no path given and no default suite found".
#[test]
fn dry_run_with_an_explicit_path_echoes_it_in_the_next_command() {
    let fixture = proef_fixture::Fixture::start().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("mysuite/packs")).unwrap();
    // Deliberately no `[run] suite` — the suite lives at a name `test` (no
    // path) can never default to.
    std::fs::write(
        tmp.path().join("proef.toml"),
        "[url]\nbase = \"${env:PROEF_BASE_URL}\"\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("mysuite/case.feature"),
        "Feature: F\n  Scenario: S\n    When the service is healthy\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("mysuite/packs/p.yaml"),
        "macros:\n  health:\n    match: the service is healthy\n    steps:\n      \
         - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    let assert = proef(tmp.path())
        .env("PROEF_BASE_URL", &fixture.base_url)
        .args(["test", "mysuite", "--dry-run"])
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        out.contains("next: proef test mysuite"),
        "nudge must echo the explicit path: {out}"
    );

    // Run the exact printed command for real.
    proef(tmp.path())
        .env("PROEF_BASE_URL", &fixture.base_url)
        .args(["test", "mysuite"])
        .assert()
        .code(0);
}

/// `init` never overwrites an authored `proef-pack.schema.json`.
///
/// The never-overwrite loop walks a fixed four-entry array; the schema is not
/// in it, and is written afterwards by the shared `schema` installer. So the
/// one unguarded path was pack-absent + schema-present: `init` scaffolds the
/// pack, then the installer replaces a file the user wrote — reported as
/// `created 5 file(s), skipped 0` while the README promises the opposite.
///
/// Named for the **schema**, deliberately. The neighbouring
/// `init_never_rewrites_an_existing_pack_via_schema_install` guards the *pack*
/// on the same code path, and its passing is what made this look covered.
#[test]
fn init_never_overwrites_an_authored_pack_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let schema = tmp.path().join("suite/packs/proef-pack.schema.json");
    std::fs::create_dir_all(schema.parent().unwrap()).unwrap();
    let authored = r#"{"mine":"do not destroy"}"#;
    std::fs::write(&schema, authored).unwrap();

    proef(tmp.path())
        .arg("init")
        .assert()
        .code(0)
        .stdout(contains("skipped"))
        .stdout(contains("proef-pack.schema.json"));

    assert_eq!(
        std::fs::read_to_string(&schema).unwrap(),
        authored,
        "init must not replace a schema the user wrote"
    );

    // `schema --add-to` is the explicit install and still refreshes — that is
    // how the schema is updated after a proef upgrade.
    proef(tmp.path())
        .args(["schema", "--add-to", "suite/packs/api.yaml"])
        .assert()
        .code(0);
    assert_ne!(
        std::fs::read_to_string(&schema).unwrap(),
        authored,
        "an explicit `schema --add-to` must still install the current schema"
    );
}
