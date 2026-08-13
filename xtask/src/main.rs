//! Workspace automation as Rust (TECH-SPEC §15) — no shell scripts for logic.
//! `just` provides thin aliases: `just canary` → `cargo run -p xtask -- canary`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("canary") => canary(&args.collect::<Vec<_>>()),
        Some("fixture") => fixture(&args.collect::<Vec<_>>()),
        Some("docs-check") => docs_check(),
        Some("public-api") => public_api(),
        Some(other) => usage(&format!("unknown task `{other}`")),
        None => usage("missing task"),
    }
}

/// The upgrade canary (ADR-0003, M4): build + test the whole workspace against
/// the *next* hurl release in an isolated copy — pins never move automatically;
/// a green canary is the precondition for the runbook (IMPLEMENTATION-PLAN §7).
///
/// `--version X.Y.Z` overrides discovery (also the rehearsal lever: point it at
/// an older release to prove the canary catches API drift).
fn canary(args: &[String]) -> ExitCode {
    let pinned = match pinned_hurl_version() {
        Ok(version) => version,
        Err(message) => {
            eprintln!("canary: {message}");
            return ExitCode::FAILURE;
        }
    };
    let target = match args {
        [flag, version] if flag == "--version" => version.clone(),
        [] => match latest_hurl_version() {
            Ok(version) => version,
            Err(message) => {
                eprintln!("canary: {message} (pass --version X.Y.Z to override)");
                return ExitCode::FAILURE;
            }
        },
        _ => {
            eprintln!("usage: cargo run -p xtask -- canary [--version X.Y.Z]");
            return ExitCode::from(2);
        }
    };

    if target == pinned {
        eprintln!("canary: pinned hurl {pinned} is the latest release — nothing newer to test");
        return ExitCode::SUCCESS;
    }
    eprintln!("canary: testing hurl {target} (pinned: {pinned}) in an isolated workspace copy");

    let scratch = std::env::temp_dir().join(format!("proef-canary-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    if let Err(err) = std::fs::create_dir_all(&scratch) {
        eprintln!("canary: cannot create {}: {err}", scratch.display());
        return ExitCode::FAILURE;
    }

    // Committed state only: the canary answers "would a pin bump break us?".
    let tar = scratch.join("src.tar");
    let ok =
        run_ok(Command::new("git").args(["archive", "HEAD", "-o", &tar.display().to_string()]))
            && run_ok(Command::new("tar").args([
                "-xf",
                &tar.display().to_string(),
                "-C",
                &scratch.display().to_string(),
            ]));
    if !ok {
        eprintln!("canary: cannot materialize the workspace copy");
        return ExitCode::FAILURE;
    }

    // Rewrite the exact pins; drop the lock (it must regenerate for the bump).
    let manifest = scratch.join("Cargo.toml");
    match std::fs::read_to_string(&manifest) {
        Ok(text) => {
            let text = text.replace(&format!("\"={pinned}\""), &format!("\"={target}\""));
            if std::fs::write(&manifest, text).is_err() {
                eprintln!("canary: cannot rewrite pins");
                return ExitCode::FAILURE;
            }
        }
        Err(err) => {
            eprintln!("canary: cannot read workspace manifest: {err}");
            return ExitCode::FAILURE;
        }
    }
    let _ = std::fs::remove_file(scratch.join("Cargo.lock"));

    let target_dir = scratch.join("target");
    let mut build = Command::new("cargo");
    build
        .args(["build", "--workspace", "--all-targets"])
        .current_dir(&scratch)
        .env("CARGO_TARGET_DIR", &target_dir);
    if !run_ok(&mut build) {
        eprintln!("canary RED: hurl {target} breaks the build — see output above.");
        eprintln!(
            "Runbook: docs/IMPLEMENTATION-PLAN.md §7 (fix adapter, or patch via docs/runbooks/thin-fork.md)"
        );
        eprintln!(
            "workspace copy left for inspection at {} (delete when done)",
            scratch.display()
        );
        return ExitCode::FAILURE;
    }
    let mut tests = Command::new("cargo");
    tests
        .args(["nextest", "run"])
        .current_dir(&scratch)
        .env("CARGO_TARGET_DIR", &target_dir);
    if !run_ok(&mut tests) {
        eprintln!("canary RED: hurl {target} builds but the suite fails — behavior drift.");
        eprintln!("Runbook: docs/IMPLEMENTATION-PLAN.md §7");
        eprintln!(
            "workspace copy left for inspection at {} (delete when done)",
            scratch.display()
        );
        return ExitCode::FAILURE;
    }
    let _ = std::fs::remove_dir_all(&scratch);
    eprintln!("canary GREEN: hurl {target} builds and the suite passes.");
    eprintln!("Pins stay at {pinned}; absorb deliberately via IMPLEMENTATION-PLAN §7.");
    ExitCode::SUCCESS
}

/// The `=X.Y.Z` hurl pin from the workspace manifest.
fn pinned_hurl_version() -> Result<String, String> {
    let manifest = std::fs::read_to_string("Cargo.toml")
        .map_err(|err| format!("cannot read Cargo.toml: {err}"))?;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if (trimmed.starts_with("hurl =") || trimmed.starts_with("hurl="))
            && let Some(version) = trimmed.split('=').nth(2)
        {
            return Ok(version.trim().trim_matches('"').to_owned());
        }
    }
    Err("no exact hurl pin found in Cargo.toml".to_owned())
}

/// Latest non-yanked hurl version from the sparse index (via curl).
fn latest_hurl_version() -> Result<String, String> {
    let output = Command::new("curl")
        .args(["-sf", "https://index.crates.io/hu/rl/hurl"])
        .output()
        .map_err(|err| format!("cannot invoke curl: {err}"))?;
    if !output.status.success() {
        return Err("index query failed".to_owned());
    }
    let body = String::from_utf8_lossy(&output.stdout);
    let mut latest = None;
    for line in body.lines() {
        if line.contains("\"yanked\":false")
            && let Some(rest) = line.split("\"vers\":\"").nth(1)
            && let Some(version) = rest.split('"').next()
        {
            latest = Some(version.to_owned());
        }
    }
    latest.ok_or_else(|| "no versions parsed from the index".to_owned())
}

fn run_ok(cmd: &mut Command) -> bool {
    cmd.status().is_ok_and(|status| status.success())
}

/// Run the fixture API server for the dev loop until interrupted. Binds the port
/// the shipped `proef.toml` advertises (8787) so `proef test` reaches it with no
/// `PROEF_BASE_URL`; takes an explicit port as `... -- fixture <port>`. If the
/// port is busy it falls back to an ephemeral one and prints the URL to export —
/// the dev loop should always come up (the original ephemeral-port rationale).
fn fixture(args: &[String]) -> ExitCode {
    const DEFAULT_PORT: u16 = 8787;
    let requested = match args.first() {
        None => DEFAULT_PORT,
        Some(arg) => match arg.parse::<u16>() {
            Ok(port) => port,
            Err(_) => return usage(&format!("fixture: invalid port `{arg}`")),
        },
    };

    // Only binding the default port lets the shipped `proef.toml` default `base`
    // reach the fixture without a PROEF_BASE_URL override (the fallback never is).
    let (server, on_default) = match proef_fixture::Fixture::start_on(requested) {
        Ok(server) => (server, requested == DEFAULT_PORT),
        Err(_) => match proef_fixture::Fixture::start() {
            Ok(server) => (server, false),
            Err(err) => {
                eprintln!("xtask: {err}");
                return ExitCode::FAILURE;
            }
        },
    };

    eprintln!("fixture API listening on {}", server.base_url);
    if !on_default {
        eprintln!("  export PROEF_BASE_URL={}", server.base_url);
    }
    eprintln!(
        "  export PROEF_SECRET_APITOKEN={}",
        proef_fixture::API_TOKEN
    );
    if on_default {
        eprintln!("  (matches proef.toml default — PROEF_BASE_URL not needed)");
    }
    eprintln!("Ctrl-C to stop");
    loop {
        std::thread::sleep(std::time::Duration::from_hours(1));
    }
}

fn usage(problem: &str) -> ExitCode {
    eprintln!("xtask: {problem}");
    eprintln!("usage: cargo run -p xtask -- <canary|fixture|docs-check|public-api>");
    ExitCode::from(2)
}

/// The living documentation corpus — what `docs/README.md` indexes, plus the two
/// root-level entry points.
///
/// `docs/superpowers/` is deliberately excluded. Those are dated plans and specs:
/// an archive of what was proposed at a moment, not documentation anyone
/// maintains. Linting an archive would mean editing history to satisfy a
/// checker — one plan links a review file it proposed and never committed, and
/// that dangling link is the honest record.
fn living_docs() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from("README.md"), PathBuf::from("CLAUDE.md")];
    for dir in ["docs", "docs/adr"] {
        if let Ok(entries) = std::fs::read_dir(dir) {
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

/// Every fenced block in `text` as `(info string, body, 1-based opening line)`.
fn fenced_blocks(text: &str) -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    let mut open: Option<(String, String, usize)> = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(info) = trimmed.strip_prefix("```") {
            match open.take() {
                // A fence closes the open block; its own info string is ignored.
                Some(block) => out.push(block),
                None => open = Some((info.trim().to_owned(), String::new(), index + 1)),
            }
            continue;
        }
        if let Some((_, body, _)) = open.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out
}

/// Fenced `toml` and `yaml` examples must parse — **with the parsers the product
/// uses**, so the check means "proef would accept this", not "some parser would".
///
/// Two shipped defects motivated this: an ADR demonstrated `bind:` with an
/// unquoted `${…}` inside a YAML *flow* mapping, where `{` opens a nested
/// mapping, so the first example a reader of that ADR copied could not load. The
/// prose around it was correct, which is exactly what review does not catch.
fn check_examples(docs: &[PathBuf], failures: &mut Vec<String>) {
    for path in docs {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for (info, body, line) in fenced_blocks(&text) {
            // The info string may carry more than a language (` ```yaml title=… `).
            let lang = info.split_whitespace().next().unwrap_or_default();
            let outcome = match lang {
                "toml" => toml::from_str::<toml::Value>(&body)
                    .err()
                    .map(|e| e.to_string()),
                "yaml" => serde_norway::from_str::<serde_norway::Value>(&body)
                    .err()
                    .map(|e| e.to_string()),
                _ => continue,
            };
            if let Some(error) = outcome {
                let first = error.lines().next().unwrap_or(&error).to_owned();
                failures.push(format!(
                    "{}:{line} — {lang} example does not parse: {first}",
                    path.display()
                ));
            }
        }
    }
}

/// Every relative markdown link resolves.
///
/// Internal links only, and deliberately so: external URLs need the network, and
/// a gate that fails on someone else's outage teaches contributors to ignore it.
/// If external checking is ever wanted, `lychee` is the tool and a scheduled job
/// is the place — not a pull-request gate.
fn check_links(docs: &[PathBuf], failures: &mut Vec<String>) {
    for path in docs {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        for (line_no, line) in text.lines().enumerate() {
            let mut rest = line;
            while let Some(at) = rest.find("](") {
                rest = &rest[at + 2..];
                let Some(end) = rest.find(')') else { break };
                let target = &rest[..end];
                rest = &rest[end..];
                // Anchors, external schemes, and templated paths are not ours.
                let file = target.split('#').next().unwrap_or_default().trim();
                if file.is_empty()
                    || file.starts_with("http")
                    || file.starts_with("mailto:")
                    || file.contains('<')
                {
                    continue;
                }
                if !base.join(file).exists() {
                    failures.push(format!(
                        "{}:{} — link target does not exist: {file}",
                        path.display(),
                        line_no + 1
                    ));
                }
            }
        }
    }
}

/// Every diagnostic code the workspace emits, found by scanning source for the
/// `"proef::<area>::<name>"` literals `Diag::error`/`warning` are built from.
///
/// A scan, not a registry: the codes are string literals at their emission
/// sites, which is what makes them greppable in the first place — the property
/// `DIAGNOSTICS.md` exists to serve.
fn emitted_codes() -> BTreeSet<String> {
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
    walk(Path::new("crates"), &mut files);
    files.sort();

    let mut codes = BTreeSet::new();
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

/// **`DIAGNOSTICS.md` and the emitted codes must agree in both directions.**
///
/// The same shape as the crate and ADR checks above — something that exists in
/// the tree must appear in the doc that indexes it — which is why it lives here
/// rather than beside the binary-dependent half: it reads files and nothing else.
///
/// It has drifted twice. The file carried a `pack::load` row nothing emitted, so
/// a reader who hit a real failure and grepped the index found a plausible entry
/// that could never be the cause — and the self-reported total counted it. An
/// undocumented code is the same failure from the other side: the index promises
/// to be the fastest route from a code to its cause, and a code missing from it
/// is a promise broken silently.
fn check_diagnostics_index(failures: &mut Vec<String>) {
    let index = std::fs::read_to_string("docs/DIAGNOSTICS.md").unwrap_or_default();
    let documented: BTreeSet<String> = index
        .lines()
        .filter_map(|line| line.strip_prefix("| `")?.split_once('`').map(|(c, _)| c))
        .filter(|code| code.split("::").count() == 2)
        .map(str::to_owned)
        .collect();
    let emitted = emitted_codes();

    // Guard against the scan silently ceasing to match emission sites, which
    // would make every assertion below pass by finding nothing.
    if emitted.len() <= 50 {
        failures.push(format!(
            "the diagnostic-code scan found only {} codes — it stopped matching \
             emission sites, so this check is passing vacuously",
            emitted.len()
        ));
        return;
    }
    for code in emitted.difference(&documented) {
        failures.push(format!(
            "`proef::{code}` is emitted but has no row in docs/DIAGNOSTICS.md"
        ));
    }
    for code in documented.difference(&emitted) {
        failures.push(format!(
            "docs/DIAGNOSTICS.md has a row for `proef::{code}`, which nothing emits"
        ));
    }
    // The file states its own total; a row added without touching it leaves a
    // count that reads as authoritative and is not.
    let stated = format!("of the {} codes", documented.len());
    if !index.contains(&stated) {
        failures.push(format!(
            "docs/DIAGNOSTICS.md should say \"{stated}\" — it has {} rows",
            documented.len()
        ));
    }
}

/// Mechanical doc↔code alignment (the drift class fixed by hand a dozen times
/// before this existed): every workspace crate appears in TECH-SPEC §2 and
/// CLAUDE.md; every ADR file appears in the docs index; every diagnostic code
/// the workspace emits has a row in DIAGNOSTICS.md and vice versa; every fenced
/// `toml`/`yaml` example parses; every relative link resolves.
///
/// The command-and-flag half of this lives in `crates/proef-cli/tests/docs.rs`
/// instead, where `assert_cmd` guarantees a built binary — this task reads files
/// and must stay runnable without one.
fn docs_check() -> ExitCode {
    let mut failures: Vec<String> = Vec::new();
    let tech_spec = std::fs::read_to_string("docs/TECH-SPEC.md").unwrap_or_default();
    let claude = std::fs::read_to_string("CLAUDE.md").unwrap_or_default();
    let index = std::fs::read_to_string("docs/README.md").unwrap_or_default();

    if let Ok(entries) = std::fs::read_dir("crates") {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !tech_spec.contains(&name) {
                failures.push(format!("crate `{name}` missing from docs/TECH-SPEC.md §2"));
            }
            if !claude.contains(&name) {
                failures.push(format!("crate `{name}` missing from CLAUDE.md"));
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir("docs/adr") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // `.md` only: an editor backup or scratch file beside an ADR is not a
            // decision, and reporting it as a missing index entry sends the
            // reader looking for a document that does not exist.
            let is_markdown = entry
                .path()
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("md"));
            if name.starts_with("ADR-") && is_markdown && !index.contains(&name) {
                failures.push(format!(
                    "`{name}` missing from the docs/README.md decision log"
                ));
            }
        }
    }
    check_diagnostics_index(&mut failures);
    let docs = living_docs();
    check_examples(&docs, &mut failures);
    check_links(&docs, &mut failures);

    if failures.is_empty() {
        println!("docs-check: aligned");
        ExitCode::SUCCESS
    } else {
        for failure in &failures {
            eprintln!("docs-check: {failure}");
        }
        ExitCode::FAILURE
    }
}

/// The public API of proef-core, snapshotted: the mechanical form of "adding
/// an engine leaves core diff-empty". Requires `cargo-public-api` + nightly
/// (CI runs it in the job that already has both). `PROEF_PUBLIC_API_UPDATE=1`
/// rewrites the snapshot deliberately.
fn public_api() -> ExitCode {
    let snapshot_path = "crates/proef-core/public-api.txt";
    let output = std::process::Command::new("cargo")
        .args(["public-api", "-p", "proef-core", "--simplified"])
        .output();
    let output = match output {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            eprintln!(
                "public-api: cargo public-api failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            return ExitCode::FAILURE;
        }
        Err(err) => {
            eprintln!("public-api: cannot run cargo public-api (install it): {err}");
            return ExitCode::FAILURE;
        }
    };
    let current = String::from_utf8_lossy(&output.stdout).into_owned();
    if std::env::var_os("PROEF_PUBLIC_API_UPDATE").is_some() {
        if let Err(err) = std::fs::write(snapshot_path, &current) {
            eprintln!("public-api: cannot write {snapshot_path}: {err}");
            return ExitCode::FAILURE;
        }
        println!("public-api: snapshot updated ({snapshot_path})");
        return ExitCode::SUCCESS;
    }
    let committed = std::fs::read_to_string(snapshot_path).unwrap_or_default();
    if committed == current {
        println!("public-api: surface unchanged");
        ExitCode::SUCCESS
    } else {
        eprintln!("public-api: proef-core's public API changed — review the diff:");
        for line in diff_lines(&committed, &current) {
            eprintln!("  {line}");
        }
        eprintln!("public-api: if intended, rerun with PROEF_PUBLIC_API_UPDATE=1");
        ExitCode::FAILURE
    }
}

/// Minimal set-difference rendering (order-stable enough for review).
fn diff_lines(before: &str, after: &str) -> Vec<String> {
    let old: std::collections::BTreeSet<&str> = before.lines().collect();
    let new: std::collections::BTreeSet<&str> = after.lines().collect();
    let mut out: Vec<String> = Vec::new();
    out.extend(old.difference(&new).map(|l| format!("- {l}")));
    out.extend(new.difference(&old).map(|l| format!("+ {l}")));
    out
}
