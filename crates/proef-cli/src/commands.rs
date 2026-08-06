//! CLI subcommand implementations.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use proef_core::engine::{DoctorStatus, EngineFactory};
use proef_core::error::ExitCode;

use crate::config::ProjectConfig;
use crate::front;
use crate::render;

/// Build the injected `${url:…}` / `${vars:…}` scope for the active environment
/// (base deep-merged with `[env.<name>]`) from an already-loaded config. An absent
/// file yields an empty scope; an unknown `--env` is a user error.
fn config_vars_for(
    active_env: Option<&str>,
    config: &ProjectConfig,
) -> Result<Arc<BTreeMap<String, String>>, ExitCode> {
    config
        .config_vars(active_env)
        .map(Arc::new)
        .map_err(|message| {
            crate::render::errln!("error: {message}");
            ExitCode::UserError
        })
}

/// Load and validate a suite through the front-end (dry-run mode), mapping a
/// front-end error to its exit code. The shared load path for `flows`,
/// `artifacts`, and `macros`; `dry_run` keeps its own so it can serialize the
/// raw `FrontError` to SARIF.
fn load_front(
    path: &Path,
    active_env: Option<&str>,
    run_id: Option<String>,
    config: &ProjectConfig,
) -> Result<front::FrontEnd, ExitCode> {
    let config_vars = config_vars_for(active_env, config)?;
    front::run(
        path,
        proef_core::resolve::ResolveMode::DryRun,
        run_id,
        config_vars,
    )
    .map_err(|err| report_front_error(&err))
}

/// `proef doctor` — run every engine-contributed environment check and report.
///
/// Exit code: `0` when nothing failed (warnings allowed), `3` when any check
/// failed — a broken environment is a system fault (ADR-0009).
pub fn doctor(engines: &[Box<dyn EngineFactory>]) -> ExitCode {
    fn row(worst: &mut DoctorStatus, name: &str, status: DoctorStatus, detail: &str) {
        let glyph = match status {
            DoctorStatus::Pass => "ok  ",
            DoctorStatus::Warn => "warn",
            DoctorStatus::Fail => "FAIL",
        };
        crate::render::outln!("  [{glyph}] {name:<24} {detail}");
        *worst = (*worst).max(status);
    }

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
            row(&mut worst, check.name, result.status, &result.detail);
        }
    }

    // CLI-owned checks: the secret machinery is not engine-contributed but
    // its health gates runs just the same (corrupt store, unreadable key).
    crate::render::outln!("\nsecrets:");
    for (status, name, detail) in crate::secretstore::doctor_checks() {
        row(&mut worst, name, status, &detail);
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
#[allow(clippy::too_many_arguments)]
pub fn dry_run(
    path: &Path,
    tags: Option<&proef_core::tags::TagExpr>,
    scenario: Option<&str>,
    scenario_file: Option<&str>,
    active_env: Option<&str>,
    run_id: Option<String>,
    sarif: Option<&Path>,
    config: &ProjectConfig,
) -> ExitCode {
    let config_vars = match config_vars_for(active_env, config) {
        Ok(vars) => vars,
        Err(code) => return code,
    };
    let result = front::run(
        path,
        proef_core::resolve::ResolveMode::DryRun,
        run_id,
        config_vars,
    );

    // SARIF export (shift-left gate): serialize the validation findings —
    // warnings on success, the diagnostic list on failure — before rendering.
    if let Some(sarif_path) = sarif {
        let diags: Vec<&proef_core::diag::Diag> = match &result {
            Ok(front) => front.warnings.iter().collect(),
            Err(proef_core::diag::FrontError::Diagnostics(list)) => list.iter().collect(),
            Err(proef_core::diag::FrontError::Core(_)) => Vec::new(),
        };
        match crate::sarif::write(&diags, sarif_path) {
            Ok(()) => crate::render::errln!("sarif report: {}", sarif_path.display()),
            Err(message) => crate::render::errln!("error: {message}"),
        }
    }

    let front = match result {
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
            .filter(|_| scenario_file.is_none_or(|file| feature.file.path == file))
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
    if (tags.is_some() || scenario.is_some() || scenario_file.is_some()) && totals.1 == 0 {
        return front::no_scenarios_matched();
    }
    let selected_note = if tags.is_none() && scenario.is_none() && scenario_file.is_none() {
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
    crate::render::outln!("next: proef test");
    ExitCode::Success
}

/// `proef flows` — list every scenario with its anchor and tags.
pub fn flows(
    path: &Path,
    output_json: bool,
    active_env: Option<&str>,
    config: &ProjectConfig,
) -> ExitCode {
    let front = match load_front(path, active_env, None, config) {
        Ok(front) => front,
        Err(code) => return code,
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

/// A macro is "dead" only if it's a user-pack pattern macro nothing bound:
/// `use:`-only helpers compose at lower time, and an unused builtin is shared
/// library surface, not the author's dead code.
fn is_dead_macro(pack: &str, calls: usize, has_pattern: bool) -> bool {
    has_pattern && calls == 0 && !pack.starts_with("builtin:")
}

/// `proef macros` — every loaded macro with its call count across the corpus,
/// flagging pattern macros that no scenario binds (dead prose bindings).
/// `use:`-only helpers compose during lowering (never a bound step), so they are
/// listed but never flagged unused. Counts the whole corpus, ignoring `--tags`.
pub fn macros(
    path: &Path,
    output_json: bool,
    active_env: Option<&str>,
    config: &ProjectConfig,
) -> ExitCode {
    let front = match load_front(path, active_env, None, config) {
        Ok(front) => front,
        Err(code) => return code,
    };

    // Which pattern macro each bound scenario step invoked.
    let mut calls: BTreeMap<&str, usize> = BTreeMap::new();
    for feature in &front.features {
        for scenario in &feature.scenarios {
            for step in &scenario.bound.steps {
                *calls.entry(step.macro_name.as_str()).or_default() += 1;
            }
        }
    }

    // Grouped by pack then name (the map is keyed by name, so the sort is what
    // groups by pack); `n` is a macro's step-bind count.
    let mut rows: Vec<_> = front.packs.macros.values().collect();
    rows.sort_unstable_by(|a, b| {
        (a.pack.as_str(), a.name.as_str()).cmp(&(b.pack.as_str(), b.name.as_str()))
    });

    // Advisory authoring-hygiene lint: pattern macros differing only in their
    // captures (same literal skeleton) are confusable. Reported, never gated.
    let near_dups = proef_core::matcher::near_duplicate_macros(rows.iter().filter_map(|m| {
        m.pattern
            .as_deref()
            .map(|pattern| (m.name.as_str(), pattern))
    }));

    if output_json {
        for m in &rows {
            let n = calls.get(m.name.as_str()).copied().unwrap_or(0);
            let json = serde_json::json!({
                "name": m.name,
                "pack": m.pack,
                "pattern": m.pattern.is_some(),
                "calls": n,
                "unused": is_dead_macro(m.pack.as_str(), n, m.pattern.is_some()),
                "nearDuplicateOf": near_dups.get(m.name.as_str()).cloned().unwrap_or_default(),
            });
            crate::render::outln!("{json}");
        }
        return ExitCode::Success;
    }

    let mut unused = 0usize;
    let mut near_dup_count = 0usize;
    let mut current_pack = "";
    for m in &rows {
        if m.pack.as_str() != current_pack {
            crate::render::outln!("{}", m.pack);
            current_pack = m.pack.as_str();
        }
        let n = calls.get(m.name.as_str()).copied().unwrap_or(0);
        let marker = if m.pattern.is_none() {
            "  (use:-only helper)"
        } else if is_dead_macro(m.pack.as_str(), n, m.pattern.is_some()) {
            unused += 1;
            "  UNUSED — no scenario binds it"
        } else if n == 0 {
            "  (builtin, unused here)"
        } else {
            ""
        };
        let near = match near_dups.get(m.name.as_str()) {
            Some(siblings) => {
                near_dup_count += 1;
                format!("  ~ near-duplicate of {}", siblings.join(", "))
            }
            None => String::new(),
        };
        crate::render::outln!("  {:<28} {n}×{marker}{near}", m.name);
    }
    let near_note = if near_dup_count > 0 {
        format!(" · {near_dup_count} near-duplicate")
    } else {
        String::new()
    };
    crate::render::outln!("\n{} macro(s) · {unused} unused{near_note}", rows.len());
    ExitCode::Success
}

/// `proef artifacts <path> -o DIR` — emit every scenario's canonical `.hurl`
/// plus sidecars (`.map.json`, `.vars`) for a stable CI hand-off (ADR-0010).
/// The written bytes are exactly the parse-validated emission.
pub fn artifacts(
    path: &Path,
    out_dir: &Path,
    run_id: Option<String>,
    active_env: Option<&str>,
    config: &ProjectConfig,
) -> ExitCode {
    let front = match load_front(path, active_env, run_id, config) {
        Ok(front) => front,
        Err(code) => return code,
    };

    if let Err(err) = std::fs::create_dir_all(out_dir) {
        crate::render::errln!("error: cannot create {}: {err}", out_dir.display());
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
                    crate::render::errln!("error: cannot serialize sidecar map: {err}");
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
                    crate::render::errln!(
                        "error: cannot write {}: {err}",
                        out_dir.join(&name).display()
                    );
                    return ExitCode::SystemError;
                }
            }
            // Copy referenced `file,…;` assets next to the artifact so stock
            // `hurl --test <file>` replays without proef's context root.
            let root = crate::fsutil::parent_dir(Path::new(feature.file.path.as_str()));
            if let Err(err) = crate::assets::copy_assets(&artifact.hurl_text, &root, out_dir) {
                crate::render::errln!("error: {}.hurl: {err}", artifact.slug);
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
    crate::render::outln!(
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
    let rendered = match serde_json::to_string_pretty(&schema) {
        Ok(rendered) => rendered,
        Err(err) => {
            // The old `"true"` fallback was the accept-everything schema —
            // `--add-to` would have installed it and silently disabled
            // editor validation. Fail like every other serialization error.
            crate::render::errln!("error: cannot serialize the pack schema: {err}");
            return ExitCode::SystemError;
        }
    };

    if add_to.is_empty() {
        crate::render::outln!("{rendered}");
        return ExitCode::Success;
    }

    let mut schema_dirs: Vec<PathBuf> = Vec::new();
    for pack_path in add_to {
        let dir = crate::fsutil::parent_dir(pack_path);
        if !schema_dirs.contains(&dir) {
            if let Err(err) = crate::fsutil::write_atomic(&dir.join(SCHEMA_FILE), &rendered) {
                crate::render::errln!(
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
                crate::render::errln!("error: cannot read {}: {err}", pack_path.display());
                return ExitCode::UserError;
            }
        };
        if text.starts_with("# yaml-language-server:") {
            crate::render::outln!("  ok {} (modeline already present)", pack_path.display());
            continue;
        }
        if let Err(err) = crate::fsutil::write_atomic(pack_path, &format!("{MODELINE}\n{text}")) {
            crate::render::errln!("error: cannot write {}: {err}", pack_path.display());
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
            // Same stream as print_all's diagnostics above — a closed reader
            // must not turn this trailing summary line into a 101 panic.
            crate::render::errln!("{errors} error(s)");
        }
        proef_core::diag::FrontError::Core(core) => {
            crate::render::errln!("error: {core}");
            let mut source = std::error::Error::source(core);
            while let Some(err) = source {
                crate::render::errln!("  caused by: {err}");
                source = err.source();
            }
        }
    }
    err.exit_code()
}
