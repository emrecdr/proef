//! Workspace automation as Rust (TECH-SPEC §15) — no shell scripts for logic.
//! `just` provides thin aliases: `just canary` → `cargo run -p xtask -- canary`.

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

/// Mechanical doc↔code alignment (the drift class fixed by hand a dozen times
/// before this existed): every workspace crate appears in TECH-SPEC §2 and
/// CLAUDE.md; every ADR file appears in the docs index.
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
            if name.starts_with("ADR-") && !index.contains(&name) {
                failures.push(format!(
                    "`{name}` missing from the docs/README.md decision log"
                ));
            }
        }
    }
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
