//! Workspace automation as Rust (TECH-SPEC §15) — no shell scripts for logic.
//! `just` provides thin aliases: `just canary` → `cargo run -p xtask -- canary`.

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("canary") => canary(&args.collect::<Vec<_>>()),
        Some("fixture") => fixture(),
        Some("dist") => not_yet("dist", "release packaging lands after M4"),
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

/// Run the fixture API server on a fixed local port until interrupted.
fn fixture() -> ExitCode {
    // The library binds an ephemeral port; for the dev loop we document the
    // printed URL rather than forcing 8787 (which may be taken).
    match proef_fixture::Fixture::start() {
        Ok(server) => {
            eprintln!("fixture API listening on {}", server.base_url);
            eprintln!("  export PROEF_BASE_URL={}", server.base_url);
            eprintln!(
                "  export PROEF_SECRET_APITOKEN={}",
                proef_fixture::API_TOKEN
            );
            eprintln!("Ctrl-C to stop");
            loop {
                std::thread::sleep(std::time::Duration::from_hours(1));
            }
        }
        Err(err) => {
            eprintln!("xtask: {err}");
            ExitCode::FAILURE
        }
    }
}

fn not_yet(task: &str, plan: &str) -> ExitCode {
    eprintln!("xtask: `{task}` is not implemented yet — {plan}");
    ExitCode::from(2)
}

fn usage(problem: &str) -> ExitCode {
    eprintln!("xtask: {problem}");
    eprintln!("usage: cargo run -p xtask -- <canary|fixture|dist>");
    ExitCode::from(2)
}
