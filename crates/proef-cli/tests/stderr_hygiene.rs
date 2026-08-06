//! Drift guard: `proef-cli` writes to stdout and stderr only through the
//! EPIPE-safe `render::outln!` / `render::errln!` macros, never a raw
//! `print!`, `println!`, `eprint!`, or `eprintln!`.
//!
//! Each of those four std macros panics when its write fails, and a closed
//! pipe on the other end surfaces as EPIPE (Rust ignores SIGPIPE), so a raw
//! call aborts with exit 101 — outside the typed 0/1/2/3 taxonomy (ADR-0009).
//!
//! The scan also covers `proef-lsp`: it runs on `Connection::stdio()`
//! (`crates/proef-lsp/src/server.rs:99`), so stdout **is** the JSON-RPC
//! channel there — a stray print corrupts protocol framing and breaks the
//! editor session, a worse failure than the exit-code risk this guard was
//! first written for. One implementation of the rule scans both crates,
//! rather than a second copy of this test living under `proef-lsp`.
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

/// `println!` does NOT contain `"print!"` as a substring (the `ln` sits
/// between them), so each macro needs its own needle.
const FORBIDDEN_MACROS: [&str; 4] = ["eprintln!", "eprint!", "println!", "print!"];

#[test]
fn cli_sources_never_use_a_raw_print_macro() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let roots = [manifest.join("src"), manifest.join("../proef-lsp/src")];

    let mut offenders = Vec::new();
    for root in &roots {
        let mut files = Vec::new();
        rust_sources(root, &mut files);
        // A silently empty scan would make this test vacuous — per root, so a
        // mistyped path in either scan cannot go unnoticed.
        assert!(
            !files.is_empty(),
            "no Rust sources found under {}",
            root.display()
        );

        for file in &files {
            let text = std::fs::read_to_string(file).expect("readable source file");
            for (index, line) in text.lines().enumerate() {
                if FORBIDDEN_MACROS.iter().any(|needle| line.contains(needle)) {
                    offenders.push(format!("{}:{}", file.display(), index + 1));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "raw print/eprint macro panics on a closed pipe — use crate::render::outln! for \
         stdout or crate::render::errln! for stderr instead:\n  {}",
        offenders.join("\n  ")
    );
}
