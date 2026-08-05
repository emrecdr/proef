//! Drift guard: `proef-cli` writes to stderr only through the EPIPE-safe
//! `render::errln!` macro, never a raw `eprintln!`.
//!
//! `eprintln!` panics when its write fails, and a closed stderr pipe surfaces
//! as EPIPE (Rust ignores SIGPIPE), so a raw call aborts with exit 101 —
//! outside the typed 0/1/2/3 taxonomy (ADR-0009).
//!
//! This lives in `tests/` rather than a `#[cfg(test)] mod` inside `src/` for a
//! correctness reason, not a stylistic one: a source-scanning assertion placed
//! inside its own scan target would match its own needle.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir`, recursively.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("readable source dir {}: {err}", dir.display()))
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn cli_sources_never_use_a_raw_eprintln() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&src, &mut files);
    // A silently empty scan would make this test vacuous.
    assert!(
        !files.is_empty(),
        "no Rust sources found under {}",
        src.display()
    );

    let mut offenders = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("readable source file");
        for (index, line) in text.lines().enumerate() {
            if line.contains("eprintln!") {
                offenders.push(format!("{}:{}", file.display(), index + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "raw eprintln! panics on a closed stderr pipe — use crate::render::errln! instead:\n  {}",
        offenders.join("\n  ")
    );
}
