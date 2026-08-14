//! An artifact does not name the machine that produced it (ADR-0010).
//!
//! ADR-0010 makes the emitted `.hurl` the executed input and a contract: the
//! same inputs must give the same bytes. An absolute path in the provenance
//! header breaks that silently — two checkouts of one suite stop comparing
//! equal, a laptop artifact stops matching CI's, and every `step_finished`
//! event carries `/home/runner/...` into a record that `explain`, `diff`,
//! `JUnit` and the HTML report all read.
//!
//! This is the assertion the class had been missing, and the reason it was
//! missing is worth stating: every other test runs the suite one way, from one
//! directory, so nothing compared two spellings of one file. The regression it
//! pins shipped in 0.12.0 and reached an adopting suite before any gate saw it.
//!
//! Deliberately self-contained rather than part of the reference corpus:
//! `tests/features` is run with no `proef.toml` in scope by design, and the
//! defect only appears when a path is *derived from* one (`[run] suite`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use assert_cmd::Command;

const FEATURE: &str =
    "Feature: health\n\n  Scenario: the service answers\n    Given the service is up\n";

const PACK: &str = "macros:\n  up:\n    match: the service is up\n    steps:\n      - name: health\n        hurl: |\n          GET ${url:base}/health\n          HTTP 200\n";

/// A project whose suite is named by `[run] suite` — the path-less form, where
/// the path reaches the front end already resolved against the config.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("suite/packs")).unwrap();
    std::fs::write(
        root.join("proef.toml"),
        "[run]\nsuite = \"suite\"\n\n[url]\nbase = \"http://127.0.0.1:8787\"\n",
    )
    .unwrap();
    std::fs::write(root.join("suite/health.feature"), FEATURE).unwrap();
    std::fs::write(root.join("suite/packs/core.yaml"), PACK).unwrap();
    dir
}

fn proef(cwd: &Path) -> Command {
    let mut cmd = Command::cargo_bin("proef").unwrap();
    cmd.current_dir(cwd).env("NO_COLOR", "1");
    cmd
}

/// Every emitted file, as `(name, bytes)`, sorted.
fn emitted(dir: &Path) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read_to_string(e.path()).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

/// The four ways to point proef at one suite must all produce **one** artifact,
/// byte for byte, and none of them may name the directory the project happens
/// to sit in.
///
/// Asserting equality across the four is stronger than asserting each is
/// relative, and it is the property adopters actually depend on: whether the
/// path was typed, typed absolutely, derived from `[run] suite`, or reached
/// from a subdirectory changes how proef *finds* the suite and must not change
/// what it *records*.
#[test]
fn one_suite_named_four_ways_emits_one_artifact() {
    let dir = project();
    let root = dir.path();
    std::fs::create_dir_all(root.join("sub")).unwrap();

    let run = |cwd: &Path, args: &[&str]| {
        proef(cwd).args(args).assert().code(0);
    };

    run(root, &["artifacts", "-o", "out-derived", "--run-id", "ci"]);
    run(
        root,
        &["artifacts", "suite", "-o", "out-typed", "--run-id", "ci"],
    );
    let absolute = root.join("suite");
    run(
        root,
        &[
            "artifacts",
            absolute.to_str().unwrap(),
            "-o",
            "out-absolute",
            "--run-id",
            "ci",
        ],
    );
    // From a subdirectory the config is found by walking *up*, so the derived
    // suite path is absolute here whatever the shell's cwd is.
    run(
        &root.join("sub"),
        &["artifacts", "-o", "../out-subdir", "--run-id", "ci"],
    );

    let derived = emitted(&root.join("out-derived"));
    assert!(
        !derived.is_empty(),
        "no artifacts emitted — the assertions below would be vacuous"
    );
    for other in ["out-typed", "out-absolute", "out-subdir"] {
        assert_eq!(
            derived,
            emitted(&root.join(other)),
            "`{other}` recorded a different artifact than the derived-path run"
        );
    }

    // Non-vacuity: the header must actually carry the suite-relative spelling,
    // not merely agree across runs (four identical *absolute* paths would also
    // satisfy the equality above).
    let (_, hurl) = derived
        .iter()
        .find(|(name, _)| {
            Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("hurl"))
        })
        .expect("an emitted .hurl");
    assert!(
        hurl.contains("# source: suite/health.feature:3"),
        "provenance header is not project-relative:\n{hurl}"
    );
}

/// Nothing proef writes may contain the absolute path of the project — not the
/// artifact, not the sidecar, not the run record, not a diagnostic.
///
/// The temp directory *is* a machine-specific path, so scanning for it is
/// exactly the property, and it is what would have caught the 0.12.0
/// regression: the emitters had never been asked whether the path they printed
/// was the one the caller typed or one `proef.toml` resolved for them.
#[test]
fn nothing_written_or_printed_names_the_directory_the_project_sits_in() {
    let dir = project();
    let root = dir.path();
    // The scan compares against the resolved spelling: `current_dir()` returns
    // an already-resolved path, so that is the form a leak would take on a
    // platform where the temp root is a symlink (macOS `/var` → `/private/var`).
    let resolved = root.canonicalize().unwrap();
    let needle = resolved.to_string_lossy().into_owned();

    proef(root)
        .args(["artifacts", "-o", "out", "--run-id", "ci"])
        .assert()
        .code(0);

    let files = emitted(&root.join("out"));
    assert_eq!(
        files.len(),
        2,
        "expected a .hurl and its .map.json: {files:?}"
    );
    for (name, text) in &files {
        assert!(
            !text.contains(&needle),
            "{name} names the machine that produced it:\n{text}"
        );
    }

    // A pack diagnostic names its source file too, and packs are discovered
    // from the same derived suite path — so the rendering is on the same hook.
    std::fs::write(
        root.join("suite/packs/core.yaml"),
        format!("{PACK}  orphan:\n    match: nothing binds this\n    steps:\n      - ref: no-such-fragment\n"),
    )
    .unwrap();
    let broken = proef(root).args(["test", "--dry-run"]).assert().code(2);
    let rendered = String::from_utf8_lossy(&broken.get_output().stdout).into_owned()
        + &String::from_utf8_lossy(&broken.get_output().stderr);
    assert!(
        rendered.contains("unknown_ref"),
        "expected the seeded diagnostic, got:\n{rendered}"
    );
    assert!(
        !rendered.contains(&needle),
        "a diagnostic names the machine that produced it:\n{rendered}"
    );
}
