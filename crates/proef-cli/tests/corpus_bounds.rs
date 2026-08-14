//! The fragment corpus is bounded (R9-3).
//!
//! `[run] fragments` names a directory proef did not write and does not
//! control — CONFIG.md sells "pointing at a corpus you did not write costs
//! nothing" — so its size is not the suite author's to get right. Pointed one
//! directory too high it read whatever was underneath: a 279 MB file cost
//! 601 MB of resident memory on `proef flows`, a command that never looks at a
//! fragment, because the text is read whole and then copied into an `Arc<str>`.
//!
//! The size is taken from the directory entry, so an oversized file is never
//! allocated. That is what makes this cheap to test: the fixture below is a
//! *sparse* file — 9 MiB by `metadata`, a few KiB on disk — and it exercises
//! exactly the path a real 9 MiB file would.
//!
//! The accumulation half of the bound is unit-tested at `front::admit` rather
//! than here; reaching it end-to-end would mean writing 64 MiB per run.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use assert_cmd::Command;

const FEATURE: &str = "Feature: F\n\n  Scenario: S\n    Given the counter is hit\n";

/// Refs a fragment, so the corpus is actually consulted and its diagnostics
/// surface. The scan is lazy — a pack that refs nothing never reports on the
/// corpus at all, which is the promise the other test here pins.
const PACK_REFS: &str =
    "macros:\n  hit:\n    match: the counter is hit\n    steps:\n      - ref: counter.hit\n";

/// Refs nothing: same corpus, no reason to look at it.
const PACK_INLINE: &str = "macros:\n  hit:\n    match: the counter is hit\n    steps:\n      - name: n\n        hurl: |\n          GET http://example.invalid/x\n          HTTP 200\n";

const SMALL: &str = "# @proef counter.hit\nGET http://example.invalid/x\nHTTP 200\n";

/// A project with a two-file corpus: one ordinary fragment file, and one that
/// reports 9 MiB to `metadata` while occupying almost nothing.
fn project(pack: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("features/packs")).unwrap();
    std::fs::create_dir_all(root.join("hurl")).unwrap();
    std::fs::write(
        root.join("proef.toml"),
        "[run]\nsuite = \"features\"\nfragments = \"hurl\"\n",
    )
    .unwrap();
    std::fs::write(root.join("features/f.feature"), FEATURE).unwrap();
    std::fs::write(root.join("features/packs/api.yaml"), pack).unwrap();
    std::fs::write(root.join("hurl/small.hurl"), SMALL).unwrap();

    // `set_len` on a fresh file gives a sparse extent on every filesystem the
    // gate runs (APFS, ext4, NTFS): the length is real, the blocks are not.
    let huge = std::fs::File::create(root.join("hurl/zz-huge.hurl")).unwrap();
    huge.set_len(9 * 1024 * 1024).unwrap();
    drop(huge);
    dir
}

fn proef(cwd: &Path) -> Command {
    let mut cmd = Command::cargo_bin("proef").unwrap();
    cmd.current_dir(cwd).env("NO_COLOR", "1");
    cmd
}

/// The oversized file is named and left out; every sibling still loads.
///
/// "Skipped, not fatal" is the whole design: a corpus is foreign by design, so
/// one bad file must never sink the ones beside it — the rule per-file read
/// resilience already established, applied to a file that is too big rather
/// than one that will not decode.
#[test]
fn an_oversized_corpus_file_is_skipped_and_its_siblings_still_load() {
    let dir = project(PACK_REFS);
    let out = proef(dir.path()).args(["fragments"]).assert();
    let rendered = String::from_utf8_lossy(&out.get_output().stdout).into_owned()
        + &String::from_utf8_lossy(&out.get_output().stderr);

    assert!(
        rendered.contains("proef::pack::oversized_fragment_file"),
        "the oversized file must report itself:\n{rendered}"
    );
    assert!(
        rendered.contains("zz-huge.hurl"),
        "the diagnostic must name the file:\n{rendered}"
    );
    // The sibling loaded: its fragment is listed, so the skip was a skip and
    // not a corpus-wide bail-out.
    assert!(
        rendered.contains("counter.hit"),
        "the rest of the corpus must still load:\n{rendered}"
    );
}

/// A corpus nothing `ref:`s still costs nothing — the CONFIG.md promise the
/// bound must not have quietly broken.
///
/// The bound runs at *read* time and the scan is lazy, so it would have been
/// easy to make an unreferenced corpus start reporting on itself. It does not:
/// no diagnostic, exit 0.
#[test]
fn a_corpus_nothing_refs_stays_silent_even_when_a_file_is_oversized() {
    let dir = project(PACK_INLINE);
    let out = proef(dir.path()).args(["flows"]).assert().code(0);
    let rendered = String::from_utf8_lossy(&out.get_output().stdout).into_owned()
        + &String::from_utf8_lossy(&out.get_output().stderr);

    assert!(
        !rendered.contains("oversized_fragment_file"),
        "an unreferenced corpus must not report on itself:\n{rendered}"
    );
    assert!(
        rendered.contains("features/f.feature"),
        "the suite must still list:\n{rendered}"
    );
}
