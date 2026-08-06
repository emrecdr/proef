//! `proef init` scaffolds a working suite: the files it writes must dry-run
//! green, it must never overwrite authored work, and running it twice must be
//! a no-op.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::str::contains;

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
}
