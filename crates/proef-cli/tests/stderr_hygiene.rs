//! Drift guards on what `proef-cli` writes, and how.
//!
//! Two rules, one scan. The first: `proef-cli` writes to stdout and stderr only
//! through the EPIPE-safe `render::outln!` / `render::errln!` macros, never a
//! raw `print!`, `println!`, `eprint!`, or `eprintln!`.
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

/// Parenthetical plurals that cannot be well formed, whatever the stem.
///
/// `(s)` and `(es)` are the house style and read correctly — `3 fragment(s)`,
/// `20 batch(es)`. `(ies)` never can: an English `-ies` plural replaces a
/// trailing `y`, so the singular the reader is offered is the stem with the `y`
/// already removed. `entr(ies)` shipped in two user-facing messages on exactly
/// that reasoning error, and `entr` is not a word. `(y)` is the same mistake
/// spelled from the other end.
const MALFORMED_PLURALS: [&str; 2] = ["(ies)", "(y)"];

/// The rule the `entr(ies)` defect cost two releases to learn, enforced rather
/// than written down.
///
/// It had been invisible to every gate: `fmt`, `clippy` and the test suite are
/// all indifferent to the contents of a string literal, so the only thing
/// standing between the codebase and a repeat was a doc comment on `plural`
/// asking politely. A stem-changing plural must spell both endings — that is
/// what `commands::plural` is for — and this catches the shape that cannot be
/// spelled parenthetically instead of trusting each site to notice.
#[test]
fn user_facing_plurals_are_never_a_malformed_parenthetical() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let roots = [manifest.join("src"), manifest.join("../proef-core/src")];

    let mut offenders = Vec::new();
    for root in &roots {
        let mut files = Vec::new();
        rust_sources(root, &mut files);
        assert!(
            !files.is_empty(),
            "no Rust sources found under {}",
            root.display()
        );

        for file in &files {
            let text = std::fs::read_to_string(file).expect("readable source file");
            for (index, line) in text.lines().enumerate() {
                // Doc comments discuss the malformed spellings by name — that is
                // where the rule is explained, so quoting it is not breaking it.
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if MALFORMED_PLURALS.iter().any(|needle| line.contains(needle)) {
                    offenders.push(format!("{}:{}", file.display(), index + 1));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "a stem-changing plural cannot be written parenthetically — the singular it \
         offers is not a word. Spell both endings with `plural(n, \"y\", \"ies\")`:\n  {}",
        offenders.join("\n  ")
    );
}
