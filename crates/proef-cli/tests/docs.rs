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

/// The living corpus: what `docs/README.md` indexes, plus the root entry points
/// and `docs/runbooks/` — on the website since #76, so its links and examples
/// are checked like every other page (R17: it was rendered but never linted).
/// `docs/superpowers/` is an archive of dated plans and is not linted.
fn living_docs(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.join("README.md"), root.join("CLAUDE.md")];
    for dir in ["docs", "docs/adr", "docs/runbooks"] {
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

// The other half of this file's job — `DIAGNOSTICS.md` and the emitted codes
// agreeing in both directions — is `xtask docs-check`'s `check_diagnostics_index`.
// It reads files and needs no built binary, which is the line this file draws.

/// The reverse direction: every subcommand the binary exposes appears in
/// README's command table and in TECH-SPEC §10's synopsis.
///
/// [`every_documented_command_and_flag_exists`] checks docs→binary; nothing
/// checked binary→docs, so a new command could ship fully implemented and
/// invisible — and did, four times (A4, TECH-SPEC §10 twice, then `flaky`,
/// each caught by an external review rather than a gate). The DIAGNOSTICS
/// index already models the bidirectional shape ("emitted but no row" and
/// "row but nothing emits"); commands now get the same treatment.
#[test]
fn every_subcommand_is_documented() {
    let root = workspace_root();
    let help = {
        let out = Command::cargo_bin("proef")
            .unwrap()
            .arg("--help")
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    // The `Commands:` block of clap's help: one indented `name  summary` line
    // per subcommand, ended by the `Options:` block.
    let mut subcommands: Vec<String> = Vec::new();
    let mut in_commands = false;
    for line in help.lines() {
        if line.trim_end() == "Commands:" {
            in_commands = true;
            continue;
        }
        if in_commands {
            if line.trim_start().starts_with("Options:") || line.trim().is_empty() {
                break;
            }
            // A subcommand row is indented exactly two spaces; a wrapped
            // summary continuation is indented to the summary column. Only
            // the rows carry names.
            if let Some(rest) = line.strip_prefix("  ")
                && !rest.starts_with(' ')
                && let Some(name) = rest.split_whitespace().next()
                && name != "help"
            {
                subcommands.push(name.to_owned());
            }
        }
    }
    // Vacuity guard: a parse that finds nothing must fail loudly, not pass an
    // empty loop — the exact failure mode this gate exists to prevent.
    assert!(
        subcommands.len() >= 10,
        "expected the full subcommand list from --help, parsed only {subcommands:?}"
    );

    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    let tech_spec = std::fs::read_to_string(root.join("docs/TECH-SPEC.md")).unwrap();
    let mut failures: Vec<String> = Vec::new();
    for sub in &subcommands {
        if !readme.contains(&format!("`proef {sub}")) {
            failures.push(format!("README.md has no row for `proef {sub}`"));
        }
        if !tech_spec.contains(&format!("proef {sub}")) {
            failures.push(format!(
                "docs/TECH-SPEC.md §10 does not mention `proef {sub}`"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "the binary exposes commands the documentation does not carry:\n  {}",
        failures.join("\n  ")
    );
}

/// The flags half of the same direction (R17-2.6): v0.14.0 shipped `--shard`
/// and `--max-fail` and the README said nothing — the command gate above is
/// blind to flags because it parses only the `Commands:` block. Every long
/// flag of every subcommand must appear backticked in the README; TECH-SPEC
/// already carries full synopses (the forward gate checks those exist), so
/// the README — the surface a new user actually reads — is the one that
/// drifts. Measured burden before adding this: the README was already
/// complete except the three flags that motivated the finding.
#[test]
fn every_flag_is_documented_in_the_readme() {
    let root = workspace_root();
    let help = {
        let out = Command::cargo_bin("proef")
            .unwrap()
            .arg("--help")
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let mut subcommands: Vec<String> = Vec::new();
    let mut in_commands = false;
    for line in help.lines() {
        if line.trim_end() == "Commands:" {
            in_commands = true;
            continue;
        }
        if in_commands {
            if line.trim_start().starts_with("Options:") || line.trim().is_empty() {
                break;
            }
            if let Some(rest) = line.strip_prefix("  ")
                && !rest.starts_with(' ')
                && let Some(name) = rest.split_whitespace().next()
                && name != "help"
            {
                subcommands.push(name.to_owned());
            }
        }
    }
    assert!(subcommands.len() >= 10, "parsed only {subcommands:?}");

    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    let mut flags: Vec<(String, String)> = Vec::new();
    for sub in &subcommands {
        let out = Command::cargo_bin("proef")
            .unwrap()
            .args([sub.as_str(), "--help"])
            .output()
            .unwrap();
        let help = String::from_utf8_lossy(&out.stdout).into_owned();
        for line in help.lines() {
            // An option row: `  -j, --jobs <N>  …` or `      --shard <I/N>  …`.
            let rest = line.trim_start();
            let Some(flag) = rest
                .strip_prefix("--")
                .or_else(|| rest.split(", ").nth(1).and_then(|r| r.strip_prefix("--")))
            else {
                continue;
            };
            let name: String = flag
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if !name.is_empty() && !matches!(name.as_str(), "help" | "version") {
                flags.push((sub.clone(), format!("--{name}")));
            }
        }
    }
    flags.sort_unstable();
    flags.dedup();
    // Vacuity guard, same rationale as the command gate above.
    assert!(
        flags.len() >= 20,
        "parsed only {} flags: {flags:?}",
        flags.len()
    );
    let missing: Vec<String> = flags
        .iter()
        .filter(|(_, flag)| !readme.contains(flag.as_str()))
        .map(|(sub, flag)| format!("`proef {sub}` exposes `{flag}`"))
        .collect();
    assert!(
        missing.is_empty(),
        "the README does not mention:\n  {}",
        missing.join("\n  ")
    );
}
