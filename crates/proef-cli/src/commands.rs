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

/// Is the editor-completion schema installed beside every project pack?
///
/// A `Warn`, never a `Fail`: a missing schema costs autocomplete and load-time
/// validation in the editor, it does not stop a run — and `doctor`'s exit is the
/// environment verdict, not an authoring one. Uses the same "is it there"
/// predicate `init` uses to decide between "installed" and "run `schema
/// --add-to`", so the two cannot disagree about what installed means.
fn schema_check(suite: Option<&Path>) -> (DoctorStatus, String) {
    // No project, or no suite in it, is not a finding — `doctor` runs anywhere.
    let Some(suite) = suite else {
        return (DoctorStatus::Pass, "no suite configured".to_owned());
    };
    let packs = crate::front::pack_files(suite).unwrap_or_default();
    if packs.is_empty() {
        return (
            DoctorStatus::Pass,
            format!("no packs under {}", suite.display()),
        );
    }
    let mut missing: Vec<PathBuf> = packs
        .iter()
        .map(|pack| crate::fsutil::parent_dir(pack))
        .filter(|dir| !dir.join(SCHEMA_FILE).exists())
        .collect();
    missing.sort();
    missing.dedup();
    if missing.is_empty() {
        return (
            DoctorStatus::Pass,
            format!("{SCHEMA_FILE} present beside {} pack(s)", packs.len()),
        );
    }
    let dirs: Vec<String> = missing.iter().map(|d| d.display().to_string()).collect();
    (
        DoctorStatus::Warn,
        format!(
            "{SCHEMA_FILE} missing in {} — editor completion is off; run `proef schema --add-to <pack>`",
            dirs.join(", ")
        ),
    )
}

/// `proef doctor` — run every engine-contributed environment check and report.
///
/// Exit code: `0` when nothing failed (warnings allowed), `3` when any check
/// failed — a broken environment is a system fault (ADR-0009).
pub fn doctor(engines: &[Box<dyn EngineFactory>], suite: Option<&Path>) -> ExitCode {
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

    // CLI-owned checks: neither the pack schema nor the secret machinery is
    // engine-contributed, but both gate authoring and runs just the same.
    crate::render::outln!("\nauthoring:");
    let (status, detail) = schema_check(suite);
    row(&mut worst, "pack schema", status, &detail);

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

/// Validate `[run] setup` / `[run] teardown` and return how many scenarios they
/// hold, for `--dry-run`.
///
/// ADR-0014 promises the phase features are "validated like any other feature
/// but never executed" under `--dry-run`. Nothing validated them: `--dry-run`
/// never read the keys at all, so a broken `[run] teardown` surfaced only after
/// a full suite had executed. Counted apart from the suite so the suite's own
/// totals keep meaning what they say.
fn validate_phase_features(
    config: &ProjectConfig,
    config_vars: &Arc<BTreeMap<String, String>>,
) -> Result<usize, ExitCode> {
    let mut scenarios = 0usize;
    for (label, path) in [("setup", config.setup()), ("teardown", config.teardown())] {
        let Some(path) = path else { continue };
        let front = crate::exec::load_phase_feature(label, Path::new(path), None, config_vars)?;
        render::print_all(&front.warnings);
        scenarios += front
            .features
            .iter()
            .map(|feature| feature.scenarios.len())
            .sum::<usize>();
    }
    Ok(scenarios)
}

/// The `proef test` command that reproduces *this* validation run.
///
/// Every selector that decided what was validated is echoed, because the point
/// of the nudge is "now run what you just checked". Printing a bare
/// `proef test` after `--dry-run --env prod --tags smoke` offers a different
/// run — a different `[url] base` and every scenario rather than the tagged
/// subset — and the operator cannot tell: the command works, and simply tests
/// something else.
///
/// **Selectors only.** This is deliberately not a general "reprint the
/// invocation": `--junit`, `--output` and friends are not selectors, and a
/// blanket reprint is how secret-bearing arguments end up on stdout.
///
/// A bare `proef test` rediscovers the suite on its own only when this run
/// resolved a *default* path; an explicit one must be echoed or the printed
/// command exits 2 with "no path given and no default suite found".
fn next_command(
    path: Option<&str>,
    tags: Option<&str>,
    scenario: Option<&str>,
    scenario_file: Option<&str>,
    active_env: Option<&str>,
) -> String {
    let mut out = String::from("proef test");
    if let Some(path) = path {
        out.push(' ');
        out.push_str(&shell_quote(path));
    }
    for (flag, value) in [
        ("--env", active_env),
        ("--tags", tags),
        ("--scenario", scenario),
        ("--scenario-file", scenario_file),
    ] {
        if let Some(value) = value {
            out.push(' ');
            out.push_str(flag);
            out.push(' ');
            out.push_str(&shell_quote(value));
        }
    }
    out
}

/// Single-quote a value the operator is expected to paste back into a shell.
/// A tag expression (`@a and not @b`) and a scenario name both carry spaces,
/// and an unquoted one silently becomes several arguments.
fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/-@:".contains(c))
    {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// `proef test --dry-run` — the validation gate: everything through lowering
/// and emission, every emitted artifact parsed with the engine's real parser
/// (TECH-SPEC §10) — no files written, no execution, no network.
///
/// `path_given` is whether the caller passed an explicit suite path (as
/// opposed to `[run] suite`/`tests/` default resolution) — it decides whether
/// the printed "next command" echoes that path: a bare `proef test` only
/// rediscovers a *defaulted* path on its own.
#[allow(clippy::too_many_arguments)]
pub fn dry_run(
    path: &Path,
    path_given: bool,
    tags: Option<&proef_core::tags::TagExpr>,
    tags_raw: Option<&str>,
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
        Arc::clone(&config_vars),
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

    let phase_note = match validate_phase_features(config, &config_vars) {
        Ok(0) => String::new(),
        Ok(n) => format!(" · {n} setup/teardown scenario(s) validated"),
        Err(code) => return code,
    };
    let selected_note = if tags.is_none() && scenario.is_none() && scenario_file.is_none() {
        String::new()
    } else {
        format!(" ({} selected by the filters)", totals.1)
    };
    crate::render::outln!(
        "\ndry-run OK: {} feature(s), {} scenario(s){selected_note}, {} step(s), {} batch(es), {} artifact(s) parse-validated, {} warning(s){phase_note}",
        front.features.len(),
        totals.0,
        totals.2,
        totals.3,
        totals.4,
        front.warnings.len()
    );
    crate::render::outln!(
        "next: {}",
        next_command(
            path_given.then(|| path.display().to_string()).as_deref(),
            tags_raw,
            scenario,
            scenario_file,
            active_env,
        )
    );
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

/// The pack JSON Schema's filename, written beside every pack `schema --add-to`
/// touches and probed by `init` to tell "not installed" from "already there".
/// One literal — two callers in different modules would otherwise drift.
pub(crate) const SCHEMA_FILE: &str = "proef-pack.schema.json";

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
        // The suite does not bind — which is exactly when an author needs to
        // read the vocabulary. `load_front` has already rendered the
        // diagnostics; list what the packs offer beneath them and keep the
        // failing exit code, so scripts see no change.
        Err(code) => {
            let Ok(packs) = front::load_packs(path) else {
                return code;
            };
            crate::render::errln!(
                "note: listing the vocabulary only — call counts need a suite that binds"
            );
            render_macros(&packs, None, output_json);
            return code;
        }
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
    render_macros(&front.packs, Some(&calls), output_json);
    ExitCode::Success
}

/// Render the macro listing.
///
/// `calls` is `None` when the suite failed to bind. Every count-derived verdict
/// is then withheld rather than guessed: a feature that did not bind
/// contributes no calls, so a macro used only by that feature would otherwise
/// be reported `UNUSED` — a confident wrong answer in precisely the state this
/// path exists to serve.
fn render_macros(
    packs: &proef_core::pack::PackSet,
    calls: Option<&BTreeMap<&str, usize>>,
    output_json: bool,
) {
    // Grouped by pack then name (the map is keyed by name, so the sort is what
    // groups by pack); `n` is a macro's step-bind count.
    let mut rows: Vec<_> = packs.macros.values().collect();
    rows.sort_unstable_by(|a, b| {
        (a.pack.as_str(), a.name.as_str()).cmp(&(b.pack.as_str(), b.name.as_str()))
    });

    // One lookup, both renderers: `None` propagates "not counted" rather than
    // collapsing to a measured zero.
    let call_count = |name: &str| calls.map(|c| c.get(name).copied().unwrap_or(0));

    // Advisory authoring-hygiene lint: pattern macros differing only in their
    // captures (same literal skeleton) are confusable. Reported, never gated.
    let near_dups = proef_core::matcher::near_duplicate_macros(rows.iter().filter_map(|m| {
        m.pattern
            .as_deref()
            .map(|pattern| (m.name.as_str(), pattern))
    }));

    if output_json {
        for m in &rows {
            // `null`, not `0`/`false`: absent knowledge, not a measured zero.
            let n = call_count(m.name.as_str());
            let json = serde_json::json!({
                "name": m.name,
                "pack": m.pack,
                "pattern": m.pattern,
                "calls": n,
                "unused": n.map(|n| is_dead_macro(m.pack.as_str(), n, m.pattern.is_some())),
                "nearDuplicateOf": near_dups.get(m.name.as_str()).cloned().unwrap_or_default(),
            });
            crate::render::outln!("{json}");
        }
        return;
    }

    let mut unused = 0usize;
    let mut near_dup_count = 0usize;
    let mut current_pack = "";
    for m in &rows {
        if m.pack.as_str() != current_pack {
            crate::render::outln!("{}", m.pack);
            current_pack = m.pack.as_str();
        }
        let n = call_count(m.name.as_str());
        let marker = if m.pattern.is_none() {
            "  (use:-only helper)"
        } else if n.is_some_and(|n| is_dead_macro(m.pack.as_str(), n, true)) {
            unused += 1;
            "  UNUSED — no scenario binds it"
        } else if n == Some(0) {
            "  (builtin, unused here)"
        } else {
            ""
        };
        // `12×` when counted, a bare `—` when the suite did not bind.
        let count = match n {
            Some(n) => format!("{n}×"),
            None => "—".to_owned(),
        };
        let near = match near_dups.get(m.name.as_str()) {
            Some(siblings) => {
                near_dup_count += 1;
                format!("  ~ near-duplicate of {}", siblings.join(", "))
            }
            None => String::new(),
        };
        // The `match:` prose, not just the identifier: a test author writes
        // sentences, so the sentence is what this listing exists to show.
        let prose = match &m.pattern {
            Some(pattern) => format!("  {pattern}"),
            None => String::new(),
        };
        crate::render::outln!("  {:<28} {count}{prose}{marker}{near}", m.name);
    }
    let near_note = if near_dup_count > 0 {
        format!(" · {near_dup_count} near-duplicate")
    } else {
        String::new()
    };
    // "0 unused" from an unbound suite would read as "nothing is dead" when
    // the truth is "not counted" — withhold the tally with the verdicts.
    let unused_note = match calls {
        Some(_) => format!(" · {unused} unused"),
        None => String::new(),
    };
    crate::render::outln!("\n{} macro(s){unused_note}{near_note}", rows.len());
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
pub fn schema(add_to: &[PathBuf], overwrite_existing: bool) -> ExitCode {
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
            // `schema --add-to` is an explicit "install/refresh this" and may
            // replace an older copy — that is how you update after upgrading
            // proef. `init` promises the opposite in as many words, so it asks
            // for the preserving mode and an authored file survives.
            if !overwrite_existing && dir.join(SCHEMA_FILE).exists() {
                crate::render::outln!(
                    "  skipped {} (already exists)",
                    dir.join(SCHEMA_FILE).display()
                );
                schema_dirs.push(dir);
                continue;
            }
            if let Err(err) = crate::fsutil::write_atomic(&dir.join(SCHEMA_FILE), &rendered) {
                crate::render::errln!(
                    "error: cannot write {}: {err}",
                    dir.join(SCHEMA_FILE).display()
                );
                return ExitCode::SystemError;
            }
            // Announced, not silent: this file is written by both `schema
            // --add-to` and `init`, and an unannounced write leaves `init`'s
            // trailing count naming more files than it listed.
            crate::render::outln!("  created {}", dir.join(SCHEMA_FILE).display());
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
            // `bind::unbound_step` tells the author to say a sentence the packs
            // already bind, without naming how to find one — deliberately, since
            // that text also renders in an editor's diagnostics pane through the
            // LSP, where the affordance is completion rather than a command.
            // Here we know we are the terminal, so we can say which command:
            // `macros` lists the vocabulary and, since #24, answers even while
            // this very error stands.
            if diags.iter().any(|d| d.code == "proef::bind::unbound_step") {
                crate::render::errln!("note: `proef macros` lists every sentence the packs bind");
            }
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

#[cfg(test)]
mod nudge_tests {
    use super::next_command;

    /// The nudge must offer the run that was just validated. Printing a bare
    /// `proef test` after `--dry-run --env prod --tags smoke` offers a
    /// different one — another `[url] base` and every scenario instead of the
    /// tagged subset — and the operator cannot tell, because the command works
    /// and simply tests something else.
    #[test]
    fn the_nudge_echoes_every_selector_that_chose_what_ran() {
        assert_eq!(next_command(None, None, None, None, None), "proef test");
        assert_eq!(
            next_command(None, Some("smoke"), None, None, Some("prod")),
            "proef test --env prod --tags smoke"
        );
        assert_eq!(
            next_command(Some("suite"), None, None, None, None),
            "proef test suite"
        );
    }

    /// A tag expression and a scenario name both carry spaces; unquoted they
    /// become several arguments and the pasted command means something else.
    #[test]
    fn values_that_would_split_into_arguments_are_quoted() {
        assert_eq!(
            next_command(None, Some("@a and not @b"), None, None, None),
            "proef test --tags '@a and not @b'"
        );
        assert_eq!(
            next_command(None, None, Some("A known record"), None, None),
            "proef test --scenario 'A known record'"
        );
        // An apostrophe in a name must not end the quoting early.
        assert_eq!(
            next_command(None, None, Some("it's fine"), None, None),
            r"proef test --scenario 'it'\''s fine'"
        );
    }
}
