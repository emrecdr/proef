//! CLI subcommand implementations.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use proef_core::engine::{DoctorStatus, EngineFactory};
use proef_core::error::ExitCode;

use crate::front;
use crate::render;

/// `proef doctor` — run every engine-contributed environment check and report.
///
/// Exit code: `0` when nothing failed (warnings allowed), `3` when any check
/// failed — a broken environment is a system fault (ADR-0009).
pub fn doctor(engines: &[Box<dyn EngineFactory>]) -> ExitCode {
    let mut worst = DoctorStatus::Pass;

    crate::render::outln!("proef doctor");
    for engine in engines {
        crate::render::outln!("\nengine `{}`:", engine.id());
        let checks = engine.doctor();
        if checks.is_empty() {
            crate::render::outln!("  (no checks contributed)");
        }
        for check in checks {
            let result = (check.run)();
            let glyph = match result.status {
                DoctorStatus::Pass => "ok  ",
                DoctorStatus::Warn => "warn",
                DoctorStatus::Fail => "FAIL",
            };
            crate::render::outln!("  [{glyph}] {:<24} {}", check.name, result.detail);
            worst = worst.max(result.status);
        }
    }

    // CLI-owned checks: the secret machinery is not engine-contributed but
    // its health gates runs just the same (corrupt store, unreadable key).
    crate::render::outln!("\nsecrets:");
    for (status, name, detail) in crate::secretstore::doctor_checks() {
        let glyph = match status {
            DoctorStatus::Pass => "ok  ",
            DoctorStatus::Warn => "warn",
            DoctorStatus::Fail => "FAIL",
        };
        crate::render::outln!("  [{glyph}] {name:<24} {detail}");
        worst = worst.max(status);
    }

    match worst {
        DoctorStatus::Pass => {
            crate::render::outln!("\nall checks passed");
            ExitCode::Success
        }
        DoctorStatus::Warn => {
            crate::render::outln!("\nusable, with warnings");
            ExitCode::Success
        }
        DoctorStatus::Fail => {
            crate::render::outln!("\nenvironment is not ready — see failed checks above");
            ExitCode::SystemError
        }
    }
}

/// `proef test --dry-run` — the validation gate: everything through lowering
/// and emission, every emitted artifact parsed with the engine's real parser
/// (TECH-SPEC §10) — no files written, no execution, no network.
pub fn dry_run(path: &Path, tags: &[String], scenario: Option<&str>) -> ExitCode {
    let front = match front::run(path, proef_core::resolve::ResolveMode::DryRun, None) {
        Ok(front) => front,
        Err(err) => return report_front_error(&err),
    };

    // scenarios, selected, steps, batches, artifacts
    let mut totals = (0usize, 0usize, 0usize, 0usize, 0usize);
    for feature in &front.features {
        let steps: usize = feature
            .scenarios
            .iter()
            .flat_map(|s| s.lowered.batches.iter())
            .map(|b| b.steps.len())
            .sum();
        let batches: usize = feature
            .scenarios
            .iter()
            .map(|s| s.lowered.batches.len())
            .sum();
        let artifacts = feature
            .scenarios
            .iter()
            .filter(|s| s.artifact.is_some())
            .count();
        let selected = feature
            .scenarios
            .iter()
            .filter(|s| front::tag_selected(&s.lowered.tags, tags))
            .filter(|s| scenario.is_none_or(|name| s.lowered.name == name))
            .count();
        crate::render::outln!(
            "  ok {} — {} scenario(s), {} step(s), {} batch(es)",
            feature.file.path,
            feature.scenarios.len(),
            steps,
            batches
        );
        totals.0 += feature.scenarios.len();
        totals.1 += selected;
        totals.2 += steps;
        totals.3 += batches;
        totals.4 += artifacts;
    }

    render::print_all(&front.warnings);
    if (!tags.is_empty() || scenario.is_some()) && totals.1 == 0 {
        eprintln!("error: no scenarios matched the filters (check --tags/--scenario)");
        return ExitCode::UserError;
    }
    let selected_note = if tags.is_empty() && scenario.is_none() {
        String::new()
    } else {
        format!(" ({} selected by the filters)", totals.1)
    };
    crate::render::outln!(
        "\ndry-run OK: {} feature(s), {} scenario(s){selected_note}, {} step(s), {} batch(es), {} artifact(s) parse-validated, {} warning(s)",
        front.features.len(),
        totals.0,
        totals.2,
        totals.3,
        totals.4,
        front.warnings.len()
    );
    ExitCode::Success
}

/// `proef flows` — list every scenario with its anchor and tags.
pub fn flows(path: &Path, output_json: bool) -> ExitCode {
    let front = match front::run(path, proef_core::resolve::ResolveMode::DryRun, None) {
        Ok(front) => front,
        Err(err) => return report_front_error(&err),
    };
    if output_json {
        for feature in &front.features {
            for scenario in &feature.scenarios {
                let scenario = &scenario.lowered;
                let json = serde_json::json!({
                    "file": feature.file.path,
                    "line": scenario.line,
                    "name": scenario.name,
                    "tags": scenario.tags,
                });
                crate::render::outln!("{json}");
            }
        }
        return ExitCode::Success;
    }
    for feature in &front.features {
        crate::render::outln!("{} — {}", feature.file.path, feature.file.name);
        for scenario in &feature.scenarios {
            let scenario = &scenario.lowered;
            let tags = if scenario.tags.is_empty() {
                String::new()
            } else {
                format!("  [@{}]", scenario.tags.join(" @"))
            };
            crate::render::outln!(
                "  {}:{}  {}{tags}",
                feature.file.path,
                scenario.line,
                scenario.name
            );
        }
    }
    crate::render::outln!(
        "\n{} macro(s) from {} pack(s)",
        front.macros_loaded,
        front.packs_loaded
    );
    ExitCode::Success
}

/// `proef artifacts <path> -o DIR` — emit every scenario's canonical `.hurl`
/// plus sidecars (`.map.json`, `.vars`) for a stable CI hand-off (ADR-0010).
/// The written bytes are exactly the parse-validated emission.
pub fn artifacts(path: &Path, out_dir: &Path, run_id: Option<String>) -> ExitCode {
    let front = match front::run(path, proef_core::resolve::ResolveMode::DryRun, run_id) {
        Ok(front) => front,
        Err(err) => return report_front_error(&err),
    };

    if let Err(err) = std::fs::create_dir_all(out_dir) {
        eprintln!("error: cannot create {}: {err}", out_dir.display());
        return ExitCode::SystemError;
    }

    let mut written = 0usize;
    for feature in &front.features {
        for scenario in &feature.scenarios {
            let Some(artifact) = &scenario.artifact else {
                continue;
            };
            let map_json = match serde_json::to_string_pretty(&artifact.map) {
                Ok(json) => format!("{json}\n"),
                Err(err) => {
                    eprintln!("error: cannot serialize sidecar map: {err}");
                    return ExitCode::SystemError;
                }
            };
            let mut files = vec![
                (
                    format!("{}.hurl", artifact.slug),
                    artifact.hurl_text.clone(),
                ),
                (format!("{}.map.json", artifact.slug), map_json),
            ];
            if let Some(vars) = &artifact.vars {
                files.push((format!("{}.vars", artifact.slug), vars.clone()));
            }
            for (name, content) in files {
                if let Err(err) = std::fs::write(out_dir.join(&name), content) {
                    eprintln!(
                        "error: cannot write {}: {err}",
                        out_dir.join(&name).display()
                    );
                    return ExitCode::SystemError;
                }
            }
            // Copy referenced `file,…;` assets next to the artifact so stock
            // `hurl --test <file>` replays without proef's context root.
            if let Some(root) = Path::new(feature.file.path.as_str()).parent()
                && let Err(err) = crate::assets::copy_assets(&artifact.hurl_text, root, out_dir)
            {
                eprintln!("error: {}.hurl: {err}", artifact.slug);
                return match err {
                    crate::assets::AssetCopyError::Unsafe(_) => ExitCode::UserError,
                    crate::assets::AssetCopyError::Io(_) => ExitCode::SystemError,
                };
            }
            crate::render::outln!("  ok {}.hurl", artifact.slug);
            written += 1;
        }
    }

    render::print_all(&front.warnings);
    let _ = writeln!(
        std::io::stdout(),
        "\n{written} artifact(s) written to {} ({} warning(s))",
        out_dir.display(),
        front.warnings.len()
    );
    ExitCode::Success
}

/// `proef schema` — print (or install) the pack JSON Schema, including the
/// step-kind fragments contributed by registered engines.
pub fn schema(add_to: &[PathBuf]) -> ExitCode {
    const SCHEMA_FILE: &str = "proef-pack.schema.json";
    const MODELINE: &str = "# yaml-language-server: $schema=./proef-pack.schema.json";

    let kinds: Vec<proef_core::engine::StepKindSpec> = crate::registry::engines()
        .iter()
        .flat_map(|e| e.step_kinds().iter().copied())
        .collect();
    let schema = proef_core::pack::json_schema(&kinds);
    let rendered = serde_json::to_string_pretty(&schema).unwrap_or_else(|_| "true".to_owned());

    if add_to.is_empty() {
        crate::render::outln!("{rendered}");
        return ExitCode::Success;
    }

    let mut schema_dirs: Vec<PathBuf> = Vec::new();
    for pack_path in add_to {
        let dir = pack_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        if !schema_dirs.contains(&dir) {
            if let Err(err) = crate::fsutil::write_atomic(&dir.join(SCHEMA_FILE), &rendered) {
                eprintln!(
                    "error: cannot write {}: {err}",
                    dir.join(SCHEMA_FILE).display()
                );
                return ExitCode::SystemError;
            }
            schema_dirs.push(dir);
        }
        let text = match std::fs::read_to_string(pack_path) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("error: cannot read {}: {err}", pack_path.display());
                return ExitCode::UserError;
            }
        };
        if text.starts_with("# yaml-language-server:") {
            crate::render::outln!("  ok {} (modeline already present)", pack_path.display());
            continue;
        }
        if let Err(err) = crate::fsutil::write_atomic(pack_path, &format!("{MODELINE}\n{text}")) {
            eprintln!("error: cannot write {}: {err}", pack_path.display());
            return ExitCode::SystemError;
        }
        crate::render::outln!("  ok {} (modeline added)", pack_path.display());
    }
    ExitCode::Success
}

/// Render a front-end failure and map it to the exit-code contract.
pub(crate) fn report_front_error(err: &proef_core::diag::FrontError) -> ExitCode {
    match err {
        proef_core::diag::FrontError::Diagnostics(diags) => {
            render::print_all(diags);
            let errors = diags
                .iter()
                .filter(|d| d.severity == proef_core::diag::Severity::Error)
                .count();
            eprintln!("{errors} error(s)");
        }
        proef_core::diag::FrontError::Core(core) => {
            eprintln!("error: {core}");
            let mut source = std::error::Error::source(core);
            while let Some(err) = source {
                eprintln!("  caused by: {err}");
                source = err.source();
            }
        }
    }
    err.exit_code()
}
