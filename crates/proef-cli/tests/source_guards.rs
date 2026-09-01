//! Drift guards enforced by scanning the workspace's own sources.
//!
//! Four rules, one walker. Each exists because the property it pins is
//! invisible to every other gate: `fmt`, `clippy` and the test suite are all
//! indifferent to the contents of a string literal, and none of them can see
//! that a rule which holds at one call site was never applied at the next.
//!
//! 1. **No raw print macros** in `proef-cli` or `proef-lsp`. Each of `print!`,
//!    `println!`, `eprint!` and `eprintln!` panics when its write fails, and a
//!    closed pipe surfaces as EPIPE (Rust ignores SIGPIPE), so a raw call
//!    aborts with exit 101 — outside the typed 0/1/2/3 taxonomy (ADR-0009).
//!    The scan covers `proef-lsp` too: it runs on `Connection::stdio()`
//!    (`crates/proef-lsp/src/server.rs`), so stdout **is** the JSON-RPC
//!    channel there and a stray print corrupts protocol framing — a worse
//!    failure than the exit-code risk this guard was first written for. One
//!    implementation scans both crates, rather than a second copy under
//!    `proef-lsp`.
//! 2. **No malformed parenthetical plurals** in user-facing text.
//! 3. **No unbounded run-record read** — `record::read_events` is the only
//!    door, and it is where the size ceiling lives.
//! 4. **No unsanctioned hurl grammar in `proef-core`** — the engine syntax the
//!    core may write or recognise is the closed set ADR-0002's amendment
//!    names, and this pins it.
//!
//! These live in `tests/` rather than a `#[cfg(test)] mod` inside `src/` for a
//! correctness reason, not a stylistic one: a source-scanning assertion placed
//! inside its own scan target would match its own needle.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
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

/// A run record is opened through `record::read_events` and nowhere else.
///
/// That function carries the 256 MiB ceiling, and its own comment explains
/// why: the read, the line split and the parsed `Vec<Event>` are resident at
/// once, so a corrupt or hostile file is an OOM rather than an error. Records
/// travel — `diff` reads a downloaded baseline, `flaky` reads every retained
/// run — so the input is not one proef necessarily wrote.
///
/// The ceiling reached two of its four readers. `explain` and `report` each
/// opened `events.jsonl` with a bare `read_to_string`, so neither had it —
/// `report` even used the guarded reader for the *base* record two dozen lines
/// below the raw read of the primary one. Nothing was wrong with either patch;
/// the guard was simply added in one place and left for the next call site to
/// rediscover. This scan is what makes that impossible: a fifth reader has to
/// go through the same door.
#[test]
fn a_run_record_is_only_ever_opened_by_the_reader_that_bounds_it() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_sources(&manifest.join("src"), &mut files);
    assert!(!files.is_empty(), "no Rust sources found");

    let mut offenders = Vec::new();
    for file in &files {
        // `record.rs` *is* the bounded reader.
        if file.file_name().is_some_and(|name| name == "record.rs") {
            continue;
        }
        let text = std::fs::read_to_string(file).expect("readable source file");
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            // The record's own file name next to a raw read is the shape:
            // either on one line, or a `read_to_string` of a path built from
            // it. Both spellings appeared in the two sites this caught.
            if line.contains("events.jsonl")
                && (text.contains("read_to_string(&events_path)")
                    || line.contains("read_to_string"))
            {
                offenders.push(format!("{}:{}", file.display(), index + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "a run record must be read through `record::read_events`, which is \
         where the size ceiling lives \u{2014} these open it directly:\n  {}",
        offenders.join("\n  ")
    );
}

/// Every engine-syntax literal `proef-core` is permitted to carry, and the
/// files permitted to carry it. Sorted; the test compares whole sets.
///
/// Three groups, and the split is the point of ADR-0002's amendment:
///
/// - **written** — the core generates this hurl text (`[Options]`, `[Asserts]`,
///   `HTTP *`, and the four option keys). This is the half the seam has *not*
///   absorbed: [`proef_core::engine::OptionRecogniser`] exists precisely so an
///   engine's option *spellings* stay out of the core, and it covers reading
///   them only.
/// - **recognised** — the core reads this to find an entry boundary before
///   performing text surgery (the fence, and the `HTTP` response line).
/// - **not hurl at all** — `secret:` and `use:` are proef's own pack syntax.
///   Listed because the scan cannot tell them apart by shape, and an
///   unexplained exclusion is how a real entry gets waved through later.
const CORE_ENGINE_GRAMMAR: &[(&str, &str, &[&str])] = &[
    ("fence", "```", &["emit.rs", "lower.rs", "pack/validate.rs"]),
    ("http", "HTTP", &["lower.rs"]),
    ("http", "HTTP ", &["lower.rs"]),
    ("http", "HTTP *", &["lower.rs"]),
    ("http", "HTTP/", &["lower.rs"]),
    ("option", "delay: {delay_ms}ms", &["lower.rs"]),
    ("option", "retry-interval: {}ms", &["lower.rs"]),
    ("option", "retry: {}", &["lower.rs"]),
    ("option", "secret:", &["lower.rs"]),
    ("option", "use:", &["pack/validate.rs"]),
    ("option", r#"variable: {name}=\"{}\""#, &["lower.rs"]),
    ("section", "[Asserts]", &["lower.rs"]),
    ("section", "[Options]", &["lower.rs"]),
];

/// The double-quoted literals on one line of Rust source, as written.
///
/// Escapes are returned unprocessed (`\"` stays two characters), so the pinned
/// inventory above reads the same as the source it pins. Char literals are
/// invisible to this — `'"'` never opens a run, because the scan only toggles
/// on an unescaped `"`.
fn string_literals(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut open: Option<usize> = None;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        match open {
            // Slicing, not copying: a literal's raw text is contiguous in the
            // line precisely because escapes stay unprocessed, so the borrow
            // and the "as written" requirement are the same decision.
            Some(start) => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    out.push(&line[start..index]);
                    open = None;
                }
            }
            // `"` is one byte, so the content starts at the next index.
            None if ch == '"' => open = Some(index + 1),
            None => {}
        }
    }
    out
}

/// Which engine-grammar shape this literal is, if any.
///
/// Structural rather than a needle list, deliberately: a needle list only
/// catches the tokens whoever wrote it thought of, and the point of the guard
/// is to notice grammar nobody predicted. The shapes are hurl's own — a
/// bracketed CamelCase section header, a body fence, a response line, and an
/// `[Options]` key line.
fn engine_grammar_kind(literal: &str) -> Option<&'static str> {
    if literal.starts_with("```") {
        return Some("fence");
    }
    if literal == "HTTP" || literal.starts_with("HTTP ") || literal.starts_with("HTTP/") {
        return Some("http");
    }
    if let Some(name) = literal
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        // Exactly one closing bracket, which is what a hurl section header has.
        if name.starts_with(|c: char| c.is_ascii_uppercase())
            && name.chars().all(|c| c.is_ascii_alphabetic())
        {
            return Some("section");
        }
    }
    if let Some((key, rest)) = literal.split_once(':') {
        let keyed = key.starts_with(|c: char| c.is_ascii_lowercase())
            && key.chars().all(|c| c.is_ascii_lowercase() || c == '-');
        let valued = rest.is_empty()
            || rest
                .strip_prefix(' ')
                .is_some_and(|value| !value.starts_with(' ') && !value.is_empty());
        if keyed && valued {
            return Some("option");
        }
    }
    None
}

/// `proef-core`'s share of hurl's grammar is the closed set ADR-0002 names.
///
/// ADR-0002's acceptance test is that a second engine lands with **zero**
/// `proef-core` diff, and the core performs text surgery on hurl entries —
/// splicing `[Options]` in, merging an `expect:` block's asserts into the
/// previous entry — so it necessarily knows *some* engine syntax. The
/// amendment's answer is not "none" but "this set, and no more". A claim like
/// that decays the moment it is only written down: the worklist carried this
/// item for two rounds as "~290 lines of hurl grammar in core", a figure that
/// counted `#[cfg(test)]` fixtures and was wrong by an order of magnitude in
/// the direction that made the problem look worse than it is.
///
/// So the set is pinned rather than described. Adding a token, or spreading an
/// existing one to another core module, fails here and sends the author to the
/// ADR — where the decision is either to widen the sanctioned set on the
/// record, or to put the new syntax behind the seam where the *reading* half
/// already lives.
#[test]
fn hurl_grammar_in_core_is_the_closed_set_the_adr_names() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../proef-core/src");
    let mut files = Vec::new();
    rust_sources(&root, &mut files);
    assert!(!files.is_empty(), "no Rust sources found under {root:?}");

    let mut found: BTreeMap<(&str, String), BTreeSet<String>> = BTreeMap::new();
    for file in &files {
        // Slash-joined components, never `Path::display` — that renders `\` on
        // Windows, and the inventory below is one spelling for every host.
        let relative = file
            .strip_prefix(&root)
            .expect("scanned file lives under the scan root")
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let text = std::fs::read_to_string(file).expect("readable source file");
        for line in production_lines(&text) {
            // Comments only. An earlier draft also skipped lines opening with
            // `*`, for block-comment continuations — of which `proef-core` has
            // none, while it does have seven `*deref = …` statements the guard
            // was silently dropping from the scan.
            if line.trim_start().starts_with("//") {
                continue;
            }
            for literal in string_literals(line) {
                if let Some(kind) = engine_grammar_kind(literal) {
                    found
                        .entry((kind, literal.to_owned()))
                        .or_default()
                        .insert(relative.clone());
                }
            }
        }
    }

    let expected: BTreeMap<(&str, String), BTreeSet<String>> = CORE_ENGINE_GRAMMAR
        .iter()
        .map(|(kind, literal, files)| {
            (
                (*kind, (*literal).to_owned()),
                files.iter().map(|f| (*f).to_owned()).collect(),
            )
        })
        .collect();

    let render = |label: &str, rows: Vec<String>| {
        if rows.is_empty() {
            String::new()
        } else {
            format!("\n{label}\n  {}", rows.join("\n  "))
        }
    };
    let added: Vec<String> = found
        .iter()
        .filter(|(key, files)| expected.get(*key).is_none_or(|known| known != *files))
        .map(|((kind, literal), files)| {
            format!(
                "{kind} {literal:?} in {}",
                files.iter().cloned().collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    let gone: Vec<String> = expected
        .keys()
        .filter(|key| !found.contains_key(*key))
        .map(|(kind, literal)| format!("{kind} {literal:?}"))
        .collect();

    assert!(
        added.is_empty() && gone.is_empty(),
        "`proef-core`'s engine grammar no longer matches the set ADR-0002's \
         amendment sanctions. Widen it there on the record, or put the syntax \
         behind the seam \u{2014} then update CORE_ENGINE_GRAMMAR.{}{}",
        render("new or relocated:", added),
        render(
            "pinned but gone (the set shrank \u{2014} tighten the ADR):",
            gone
        ),
    );
}

/// The lines of `text` outside its trailing `#[cfg(test)]` module.
///
/// A test fixture writing hurl is not the core knowing hurl — a core test that
/// exercises the pipeline has to supply *some* engine's payload. Conflating
/// the two is what produced the worklist's order-of-magnitude overcount.
fn production_lines(text: &str) -> impl Iterator<Item = &str> {
    let lines: Vec<&str> = text.lines().collect();
    // Column 0 and `mod` on the next line: the crate's test modules are all
    // written that way, and requiring both keeps a nested `#[cfg(test)] fn`
    // *inside* a test module (`tags.rs` has one) from truncating early.
    let end = lines
        .windows(2)
        .position(|pair| pair[0] == "#[cfg(test)]" && pair[1].starts_with("mod "))
        .unwrap_or(lines.len());
    lines.into_iter().take(end)
}
