//! Documented commands and flags must exist.
//!
//! The counterpart to `xtask docs-check`, which validates links and example
//! syntax by reading files. This half needs a **built binary** to be exact — it
//! asks clap itself rather than parsing help text into a model that could drift —
//! so it lives where `assert_cmd` guarantees one.
//!
//! Motivated by a shipped defect: `IMPROVEMENT-PLAN.md` documented
//! `proef report --html <run-id>` and marked the row shipped. There is no
//! `--html`; `report` *is* the HTML report command. The surrounding prose was
//! accurate, which is exactly what review does not catch.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use assert_cmd::Command;

/// Commands the docs discuss as **proposals or rejected designs**, which
/// therefore must not exist. Listed by name so adding one is a decision someone
/// makes on purpose rather than a check quietly going soft.
///
/// `generate` (ADR-0016's `OpenAPI` suite generator) · `merge` (named in
/// OPEN-FINDINGS E2 as the symptom-treating fix that was *not* taken) ·
/// `stub`, `tap` (IMPROVEMENT-PLAN rows, both superseded).
///
/// `fragments` was here while R9-1 was unbuilt; it is a real command now, so it
/// left — which is the check working: a proposal that ships must stop being
/// exempt, or the list quietly becomes a place where commands go unverified.
const PROPOSED_COMMANDS: &[&str] = &["generate", "merge", "stub", "tap"];

/// Docs whose job includes naming flags that no longer exist. A changelog that
/// could not say "`--value` was removed in favour of `--stdin`" would be failing
/// at the one thing it is for.
const HISTORY_DOCS: &[&str] = &["CHANGELOG.md", "RELEASING.md"];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// The living corpus: what `docs/README.md` indexes, plus the root entry points.
/// `docs/superpowers/` is an archive of dated plans and is not linted.
fn living_docs(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.join("README.md"), root.join("CLAUDE.md")];
    for dir in ["docs", "docs/adr"] {
        if let Ok(entries) = std::fs::read_dir(root.join(dir)) {
            let mut found: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|x| x.to_str())
                        .is_some_and(|x| x.eq_ignore_ascii_case("md"))
                })
                .collect();
            found.sort();
            out.extend(found);
        }
    }
    out
}

/// Every inline `` `code span` `` and fenced-block body in `text`.
///
/// Restricting to code context is what makes this checkable at all: prose says
/// "proef discovers packs" and "proef never writes a fragment", and treating
/// those as invocations produced sixty false positives against four real ones.
/// A backtick is the author saying *this is a command*.
fn code_regions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fenced: Option<String> = None;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            match fenced.take() {
                Some(body) => out.push(body),
                None => fenced = Some(String::new()),
            }
            continue;
        }
        if let Some(body) = fenced.as_mut() {
            body.push_str(line);
            body.push('\n');
            continue;
        }
        // Odd-indexed segments of a backtick split are the spans.
        for (index, segment) in line.split('`').enumerate() {
            if index % 2 == 1 {
                out.push(segment.to_owned());
            }
        }
    }
    out
}

/// The argument list of every `proef …` invocation in `region`.
///
/// A `proef` token only starts a command at a shell boundary. In
/// `cargo install proef --locked` it is the *crate being installed*, so
/// `--locked` is cargo's flag, not ours — the one false positive this rule
/// exists to drop. `cargo run -p proef -- test …` is recognised via its `--`.
fn invocations(region: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for raw in region.lines() {
        // One command per segment; a pipeline's later stages are other programs.
        for segment in raw.split(['|', ';', '#']) {
            for part in segment.split("&&") {
                let mut tokens = part.split_whitespace().peekable();
                let mut args: Vec<String> = Vec::new();
                let mut started = false;
                while let Some(token) = tokens.next() {
                    if !started {
                        // `$ proef …`, `proef …`, or `cargo run -p proef -- …`.
                        let boundary = args.is_empty();
                        if token == "proef" && boundary {
                            started = true;
                        } else if token == "proef" && tokens.peek() == Some(&"--") {
                            tokens.next();
                            // Everything before the `--` belongs to cargo.
                            args.clear();
                            started = true;
                        } else if token != "$" {
                            // Any other leading token means this is not our command.
                            args.push(token.to_owned());
                        }
                        continue;
                    }
                    args.push(token.to_owned());
                }
                if started {
                    out.push(args);
                }
            }
        }
    }
    out
}

fn help_for(cache: &mut BTreeMap<String, Option<String>>, sub: &str) -> Option<String> {
    cache
        .entry(sub.to_owned())
        .or_insert_with(|| {
            let mut cmd = Command::cargo_bin("proef").unwrap();
            if !sub.is_empty() {
                cmd.arg(sub);
            }
            let out = cmd.arg("--help").output().unwrap();
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
        })
        .clone()
}

#[test]
fn every_documented_command_and_flag_exists() {
    let root = workspace_root();
    let mut cache: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();

    for path in living_docs(&root) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let is_history = HISTORY_DOCS.contains(&name.as_str());

        for region in code_regions(&text) {
            for args in invocations(&region) {
                let sub = args
                    .first()
                    .filter(|a| {
                        !a.starts_with('-')
                            && a.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                            && !a.is_empty()
                    })
                    .cloned()
                    .unwrap_or_default();
                if PROPOSED_COMMANDS.contains(&sub.as_str()) {
                    continue;
                }
                let Some(help) = help_for(&mut cache, &sub) else {
                    failures.push(format!("{name}: `proef {sub}` is not a command"));
                    continue;
                };
                if is_history {
                    continue;
                }
                for token in &args {
                    let flag = token
                        .split('=')
                        .next()
                        .unwrap_or_default()
                        .trim_end_matches([',', '.', ')']);
                    if flag.starts_with("--") && flag.len() > 2 && !help.contains(flag) {
                        let shown = if sub.is_empty() { "proef" } else { &sub };
                        failures.push(format!("{name}: `{shown}` has no flag `{flag}`"));
                    }
                }
            }
        }
    }

    failures.sort();
    failures.dedup();
    assert!(
        failures.is_empty(),
        "documentation names commands or flags the binary does not have:\n  {}",
        failures.join("\n  ")
    );
}

/// Every diagnostic code the workspace emits, found by scanning source for the
/// `"proef::<area>::<name>"` literals `Diag::error`/`warning` are built from.
///
/// A scan, not a registry: the codes are string literals at their emission
/// sites, which is what makes them greppable in the first place — the property
/// `DIAGNOSTICS.md` exists to serve.
fn emitted_codes(root: &Path) -> std::collections::BTreeSet<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files);
    files.sort();

    let mut codes = std::collections::BTreeSet::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let mut rest = text.as_str();
        while let Some(start) = rest.find("\"proef::") {
            rest = &rest[start + 1..];
            let Some(end) = rest.find('"') else { break };
            let code = &rest[..end];
            // `proef::<area>::<name>`, nothing else — the corpus driver builds a
            // code from a directory name, and that expression is not one.
            let parts: Vec<&str> = code.split("::").collect();
            if parts.len() == 3
                && parts[1..]
                    .iter()
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
            {
                codes.insert(parts[1..].join("::"));
            }
        }
    }
    // A fixture code from the diagnostics unit tests, not a real diagnostic.
    codes.remove("test::x");
    codes
}

/// The codes `DIAGNOSTICS.md` lists, one per table row.
fn documented_codes(root: &Path) -> std::collections::BTreeSet<String> {
    std::fs::read_to_string(root.join("docs/DIAGNOSTICS.md"))
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("| `")?.split_once('`').map(|(c, _)| c))
        .filter(|code| code.split("::").count() == 2)
        .map(str::to_owned)
        .collect()
}

/// **`DIAGNOSTICS.md` and the emitted codes must agree in both directions.**
///
/// The docs-vs-binary guard above only catches documentation naming things that
/// do not exist. This is the other direction, and it has drifted twice: the file
/// carried a `pack::load` row nothing emitted, so a reader who hit a real
/// failure and grepped the index found a plausible-looking entry that could
/// never be the cause — and the self-reported total counted it. An undocumented
/// code is the same failure from the other side: the index promises to be the
/// fastest route from a code to its cause, and a code missing from it is a
/// promise broken silently.
#[test]
fn every_diagnostic_code_is_documented_and_every_documented_code_exists() {
    let root = workspace_root();
    let emitted = emitted_codes(&root);
    let documented = documented_codes(&root);

    assert!(
        emitted.len() > 50,
        "the scan found only {} codes — it stopped matching emission sites, so this \
         test is passing vacuously",
        emitted.len()
    );

    let undocumented: Vec<&String> = emitted.difference(&documented).collect();
    assert!(
        undocumented.is_empty(),
        "codes the workspace emits with no row in docs/DIAGNOSTICS.md: {undocumented:?}"
    );
    let phantom: Vec<&String> = documented.difference(&emitted).collect();
    assert!(
        phantom.is_empty(),
        "rows in docs/DIAGNOSTICS.md for codes nothing emits: {phantom:?}"
    );

    // The file states its own total; a row added without touching it leaves a
    // count that reads as authoritative and is not.
    let doc = std::fs::read_to_string(root.join("docs/DIAGNOSTICS.md")).unwrap();
    let stated = format!("of the {} codes", documented.len());
    assert!(
        doc.contains(&stated),
        "docs/DIAGNOSTICS.md should say \"{stated}\" — it has {} rows",
        documented.len()
    );
}
