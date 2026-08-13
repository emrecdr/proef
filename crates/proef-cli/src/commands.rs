//! CLI subcommand implementations.

use std::collections::BTreeMap;
use std::fmt::Write as _;
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

/// This invocation's fragment corpus, read from `[run] fragments` (ADR-0018).
///
/// One helper rather than the same three lines at five call sites: the corpus
/// is read once per invocation and shared by every load, and a site that built
/// its own would silently reintroduce the per-load rescan this exists to avoid.
pub(crate) fn corpus(config: &ProjectConfig) -> Result<proef_core::pack::FragmentCorpus, ExitCode> {
    front::fragment_corpus(config.fragments().as_deref()).map_err(|err| report_front_error(&err))
}

/// The parsed `[run] exclusive-tags`, or the user error a malformed one is.
///
/// Shared by `--dry-run` and the real run so they cannot disagree about whether
/// the key is valid — which they did: only the run path parsed it, so the gate
/// CI runs waved through the one error the setting is designed to make
/// impossible.
pub(crate) fn exclusive_tags(
    config: &ProjectConfig,
) -> Result<Option<proef_core::tags::TagExpr>, ExitCode> {
    config.exclusive_tags().map_err(|message| {
        render::errln!("error: {message}");
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
    fragments: &proef_core::pack::FragmentCorpus,
) -> Result<front::FrontEnd, ExitCode> {
    let config_vars = config_vars_for(active_env, config)?;
    // Taken from the caller for the reason `corpus` exists: one read per
    // invocation. Building one here made every caller that *also* held a corpus
    // pay for a second walk, read and hurl-parse of the same files — which
    // `validate_phase_features` had already been fixed not to do.
    front::run(
        path,
        proef_core::resolve::ResolveMode::DryRun,
        run_id,
        config_vars,
        fragments,
        &config.state_file(),
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
pub fn doctor(
    engines: &[Box<dyn EngineFactory>],
    suite: Option<&Path>,
    fragments: Option<&Path>,
    secrets: &Path,
    config_error: Option<&str>,
) -> ExitCode {
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

    // Conditional, like the fragments row below: an absent `proef.toml` is not
    // a finding — `doctor` must run outside a project — but one that is present
    // and unparseable is, and every check under it ran against defaults the
    // project never asked for. A row rather than a bare print so it reaches
    // `worst` and the exit code, which is what a CI script reads.
    if let Some(error) = config_error {
        crate::render::outln!("\nproject:");
        row(&mut worst, "proef.toml", DoctorStatus::Fail, error);
    }

    // CLI-owned checks: neither the pack schema nor the secret machinery is
    // engine-contributed, but both gate authoring and runs just the same.
    crate::render::outln!("\nauthoring:");
    let (status, detail) = schema_check(suite);
    row(&mut worst, "pack schema", status, &detail);
    if let Some((status, detail)) = fragment_check(fragments) {
        row(&mut worst, "fragments", status, &detail);
    }

    crate::render::outln!("\nsecrets:");
    for (status, name, detail) in crate::secretstore::doctor_checks(secrets) {
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
    fragments: &proef_core::pack::FragmentCorpus,
) -> Result<usize, ExitCode> {
    let mut scenarios = 0usize;
    // Taken from the caller, not built here: the corpus depends on neither the
    // phase nor the suite, so the one `dry_run` already read serves both.
    // Building a second would reintroduce the per-load rescan `corpus` exists
    // to avoid — the invariant, not just the cost, is the point.
    for (label, path) in [("setup", config.setup()), ("teardown", config.teardown())] {
        let Some(path) = path else { continue };
        let front =
            crate::exec::load_phase_feature(label, &path, None, config_vars, fragments, config)?;
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

/// SARIF export for a dry run (shift-left gate): serialize the validation
/// findings before rendering them — warnings when the suite validated, the
/// diagnostic list when it did not, because a gate reports what it found and
/// not only what it rejected.
fn write_sarif(result: &Result<front::FrontEnd, proef_core::diag::FrontError>, sarif_path: &Path) {
    let diags: Vec<&proef_core::diag::Diag> = match result {
        Ok(front) => front.warnings.iter().collect(),
        Err(proef_core::diag::FrontError::Diagnostics(list)) => list.iter().collect(),
        Err(proef_core::diag::FrontError::Core(_)) => Vec::new(),
    };
    match crate::sarif::write(&diags, sarif_path) {
        Ok(()) => crate::render::errln!("sarif report: {}", sarif_path.display()),
        Err(message) => crate::render::errln!("error: {message}"),
    }
}

/// `proef test --dry-run` — the validation gate: everything through lowering
/// and emission, every emitted artifact parsed with the engine's real parser
/// (TECH-SPEC §10) — no files written, no execution, no network.
///
/// `path_given` is whether the caller passed an explicit suite path (as
/// opposed to `[run] suite`/`tests/` default resolution) — it decides whether
/// the printed "next command" echoes that path: a bare `proef test` only
/// rediscovers a *defaulted* path on its own.
// One linear validation report: the stages run in a fixed order and the reader
// follows it top to bottom, so splitting hides the order rather than the length.
// The flat parameter list mirrors the CLI flag surface one-to-one.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
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
    let exclusive = match exclusive_tags(config) {
        Ok(expr) => expr,
        Err(code) => return code,
    };
    let config_vars = match config_vars_for(active_env, config) {
        Ok(vars) => vars,
        Err(code) => return code,
    };
    let fragments = match corpus(config) {
        Ok(fragments) => fragments,
        Err(code) => return code,
    };
    let result = front::run(
        path,
        proef_core::resolve::ResolveMode::DryRun,
        run_id,
        Arc::clone(&config_vars),
        &fragments,
        &config.state_file(),
    );

    if let Some(sarif_path) = sarif {
        write_sarif(&result, sarif_path);
    }

    let front = match result {
        Ok(front) => front,
        Err(err) => return report_front_error(&err),
    };
    front::warn_if_exclusive_matches_nothing(
        &front,
        exclusive.as_ref(),
        config.run.exclusive_tags.as_deref(),
    );

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

    let phase_note = match validate_phase_features(config, &config_vars, &fragments) {
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
    let fragments = match corpus(config) {
        Ok(fragments) => fragments,
        Err(code) => return code,
    };
    let front = match load_front(path, active_env, None, config, &fragments) {
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
    let fragments = match corpus(config) {
        Ok(fragments) => fragments,
        Err(code) => return code,
    };
    let front = match load_front(path, active_env, None, config, &fragments) {
        Ok(front) => front,
        // The suite does not bind — which is exactly when an author needs to
        // read the vocabulary. `load_front` has already rendered the
        // diagnostics; list what the packs offer beneath them and keep the
        // failing exit code, so scripts see no change.
        Err(code) => {
            let Ok(packs) = front::load_packs(path, &fragments) else {
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

/// The `[run] fragments` root, as `doctor` sees it. `None` when the key is
/// unset — the feature is off, and a row saying so would be noise for the
/// majority who never use it.
///
/// A misconfigured root otherwise surfaces only much later, as
/// `pack::unknown_ref` on the first `ref:` — an error about a *name* when the
/// cause is a *path*. Same argument that gave the pack schema its row: a suite
/// whose fragments had silently stopped loading had nothing telling it so.
///
/// Goes through the same loader the runner uses, so the two cannot disagree
/// about what the root contains.
fn fragment_check(root: Option<&Path>) -> Option<(DoctorStatus, String)> {
    let root = root?;
    let shown = root.display();
    if !root.is_dir() {
        return Some((
            DoctorStatus::Warn,
            format!("`{shown}` is not a directory — every `ref:` will read as unknown"),
        ));
    }
    let corpus = match front::fragment_corpus(Some(root)) {
        Ok(corpus) => corpus,
        Err(err) => return Some((DoctorStatus::Warn, format!("`{shown}`: {err}"))),
    };
    let named = corpus.fragments().len();
    let bare: usize = corpus.unannotated().values().map(Vec::len).sum();
    let broken = corpus.diagnostics().len();
    let status = if broken > 0 {
        DoctorStatus::Warn
    } else {
        DoctorStatus::Pass
    };
    // Pluralised rather than spelled `entr(ies)`, which is not a word either
    // way round; the listing below already carries the same `entr{}` idiom.
    let mut detail = format!(
        "{named} fragment{} from `{shown}`",
        if named == 1 { "" } else { "s" }
    );
    if bare > 0 {
        let _ = write!(
            detail,
            " · {bare} unannotated entr{}",
            if bare == 1 { "y" } else { "ies" }
        );
    }
    if broken > 0 {
        let _ = write!(
            detail,
            " · {broken} file{} proef could not read",
            if broken == 1 { "" } else { "s" }
        );
    }
    Some((status, detail))
}

/// `proef fragments` — list the corpus, with how many scenarios actually run
/// each entry (ADR-0018).
///
/// The counterpart to [`macros`], and deliberately symmetric with it: fragments
/// are the second input language, and until now nothing in proef's output stated
/// how many there were. Without a denominator neither way a fragment can die is
/// noticeable — an entry no macro references is invisible, and one reached only
/// through a macro no scenario binds looks covered because the *macro* is
/// flagged.
///
/// Reachability is read off the **lowered** scenarios rather than walked
/// statically: `use:` inlines a target's steps, so a fragment can be reached
/// through a chain of macros, and lowering has already resolved every such hop.
/// What the run would execute is therefore the answer, not an approximation of
/// it.
pub fn fragments(
    path: &Path,
    output_json: bool,
    check: bool,
    require_annotated: bool,
    active_env: Option<&str>,
    config: &ProjectConfig,
) -> ExitCode {
    let corpus = match corpus(config) {
        Ok(corpus) => corpus,
        Err(code) => return code,
    };
    // Diagnostics first: an unreadable or unparseable file is why a fragment is
    // missing from the listing, and a listing that silently omitted it would
    // read as "you never wrote it".
    render::print_all(corpus.diagnostics());

    // The suite is loaded for run counts only. When it does not bind there are
    // no counts to have — the same state `macros` handles by listing anyway and
    // withholding every count-derived verdict, rather than reporting a confident
    // `0×` that means "not measured".
    // The failing exit code is kept, as `macros` keeps it: the listing is still
    // worth printing beneath the diagnostics, but a suite that did not load is
    // not a success, and a script reading only the code must not be told it was.
    let (loaded, load_failure) = match load_front(path, active_env, None, config, &corpus) {
        Ok(front) => (Some(front), None),
        Err(code) => (None, Some(code)),
    };
    let runs: Option<BTreeMap<String, usize>> = loaded.as_ref().map(|front| {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        count_fragment_runs(front, &mut counts);
        counts
    });
    // `[run] setup`/`teardown` run their scenarios too, so a fragment only they
    // reach is *used*. Judging reachability over the suite alone reported it
    // `UNREACHABLE — no scenario binds it`, which was false, and failed
    // `--check` — a false CI failure in the workflow `--check` exists for. It
    // also made the verdict depend on where the phase file sat: inside the
    // suite directory it was discovered as an ordinary feature and counted.
    // Which half went missing, kept so the reader is told the truth about it.
    // Collapsing the two said "--check needs a suite that binds" when the suite
    // had bound perfectly well and a `[run] setup` feature was the thing that
    // would not load — sending the reader to inspect the half that was fine.
    let phase_runs = phase_fragment_runs(config, active_env, &corpus);
    let phase_failure = phase_runs.as_ref().err().copied();
    let (runs, unmeasured) = match (runs, phase_runs.ok()) {
        (Some(mut suite), Some(phases)) => {
            for (fragment, n) in phases {
                *suite.entry(fragment).or_default() += n;
            }
            (Some(suite), None)
        }
        // Either half missing leaves the universe incomplete, so every
        // count-derived verdict is withheld rather than guessed.
        (None, _) => (None, Some("the suite did not load")),
        (Some(_), None) => (
            None,
            Some("a configured `[run] setup`/`teardown` feature did not load"),
        ),
    };

    let referenced_by = loaded.as_ref().map(macros_referencing).unwrap_or_default();

    render_fragments(&corpus, runs.as_ref(), &referenced_by, output_json);

    // Say why a count column is dashes, the way `macros` does. Without it the
    // listing looks like a corpus nothing uses rather than a measurement that
    // could not be taken — the same confusion, one column over.
    if let Some(reason) = unmeasured
        && !output_json
    {
        render::errln!(
            "note: listing the corpus only — run counts need a suite that binds ({reason})"
        );
    }

    // Either half failing is a failure, for the reason the suite half already
    // gave: the listing is worth printing beneath the diagnostics, but a script
    // reading only the code must not be told this was a success.
    if let Some(code) = load_failure.or(phase_failure) {
        return code;
    }
    if !check {
        return ExitCode::Success;
    }
    // A `--check` with no corpus to check passes for the same reason an empty
    // one does, and the two are indistinguishable in the output: `0 entries`,
    // exit 0. That makes the CI gate disarm silently the day `[run] fragments`
    // is dropped from the config — the gate keeps reporting success about a
    // corpus it is no longer looking at. Asking for a check of nothing is a
    // question about the configuration, so it is answered as one.
    if config.fragments().is_none() {
        render::errln!(
            "error: --check needs `[run] fragments` set — with no corpus configured there is \
             nothing to check, and a passing gate would say otherwise"
        );
        return ExitCode::UserError;
    }
    // `--check` needs measured counts to fail on. Without them the honest
    // answer is that the gate could not run, not that it passed.
    let Some(runs) = runs.as_ref() else {
        render::errln!(
            "error: --check needs measured run counts and {} — so they were not measured",
            unmeasured.unwrap_or("the universe was incomplete")
        );
        return ExitCode::UserError;
    };
    let never_run: Vec<&str> = corpus
        .fragments()
        .values()
        .filter(|f| !runs.contains_key(&f.qualified()))
        .map(|f| f.name.as_str())
        .collect();
    let unannotated: usize = corpus.unannotated().values().map(Vec::len).sum();
    let mut failed = false;
    if !never_run.is_empty() {
        render::errln!(
            "error: {} fragment{} that no scenario runs: {}",
            never_run.len(),
            if never_run.len() == 1 { "" } else { "s" },
            never_run.join(", ")
        );
        failed = true;
    }
    // Opt-in, because an unannotated entry is inert *by design* (ADR-0018): a
    // corpus proef did not write is expected to be mostly those, and pointing at
    // one costs nothing. During a port the same signal means "not done yet",
    // which is a different claim — so the porting team asks for it explicitly
    // rather than every adopter inheriting it.
    if require_annotated && unannotated > 0 {
        render::errln!(
            "error: {unannotated} entr{} carr{} no `# @proef` annotation",
            if unannotated == 1 { "y" } else { "ies" },
            if unannotated == 1 { "ies" } else { "y" }
        );
        failed = true;
    }
    if failed {
        ExitCode::TestFailure
    } else {
        ExitCode::Success
    }
}

/// Which macros name each fragment, keyed by qualified name.
///
/// For the "reached only through a macro nothing binds" death mode — the one
/// that looks covered, because the *macro* warning already fires and reads as
/// the whole story. Naming the macro is what lets the reader tell the two
/// apart: a fragment nothing references at all, and one referenced by a macro
/// no scenario says.
fn macros_referencing(front: &front::FrontEnd) -> BTreeMap<String, Vec<String>> {
    let mut referenced_by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for macro_ in front.packs.macros.values() {
        let proef_core::pack::MacroBody::Steps(steps) = &macro_.body else {
            continue;
        };
        for step in steps {
            if let proef_core::pack::MacroStepKind::Ref { target } = &step.kind
                && let Some(fragment) = front.packs.find_fragment(target)
            {
                let names = referenced_by.entry(fragment.qualified()).or_default();
                if !names.contains(&macro_.name) {
                    names.push(macro_.name.clone());
                }
            }
        }
    }
    referenced_by
}

/// Add every fragment `front`'s scenarios run to `counts`.
///
/// Distinct scenarios, not steps: `2×` should mean two scenarios exercise this
/// request, not that one scenario called it twice.
fn count_fragment_runs(front: &front::FrontEnd, counts: &mut BTreeMap<String, usize>) {
    for feature in &front.features {
        for scenario in &feature.scenarios {
            let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            for batch in &scenario.lowered.batches {
                for step in &batch.steps {
                    if let Some(fragment) = &step.fragment {
                        seen.insert(fragment.as_str());
                    }
                }
            }
            for fragment in seen {
                *counts.entry(fragment.to_owned()).or_default() += 1;
            }
        }
    }
}

/// Fragment usage from `[run] setup` / `[run] teardown`, or the failing exit
/// code if a configured phase could not be loaded.
///
/// The runner executes these features, so a fragment they reach is used. An
/// error rather than an empty map on failure: a phase that did not load leaves
/// the universe incomplete, and a count drawn from an incomplete universe is
/// the false `UNREACHABLE` this exists to prevent.
///
/// The code is carried rather than flattened to `None`, because the diagnostics
/// have already been printed by the time it is known. Swallowing it printed
/// `error:` lines and then exited 0 — the same "reported success while
/// producing wrong output" shape the suite half is careful about two lines up.
fn phase_fragment_runs(
    config: &ProjectConfig,
    active_env: Option<&str>,
    fragments: &proef_core::pack::FragmentCorpus,
) -> Result<BTreeMap<String, usize>, ExitCode> {
    let mut counts = BTreeMap::new();
    let phases = [("setup", config.setup()), ("teardown", config.teardown())];
    if phases.iter().all(|(_, path)| path.is_none()) {
        return Ok(counts);
    }
    let config_vars = config_vars_for(active_env, config)?;
    for (label, path) in phases {
        let Some(path) = path else { continue };
        let front =
            crate::exec::load_phase_feature(label, &path, None, &config_vars, fragments, config)?;
        count_fragment_runs(&front, &mut counts);
    }
    Ok(counts)
}

/// Render the fragment listing, grouped by file.
fn render_fragments(
    corpus: &proef_core::pack::FragmentCorpus,
    runs: Option<&BTreeMap<String, usize>>,
    referenced_by: &BTreeMap<String, Vec<String>>,
    output_json: bool,
) {
    let fragments = corpus.fragments();
    let unannotated = corpus.unannotated();
    // Every file that contributed anything, annotated or not — a file of purely
    // unannotated entries is precisely what a porting team needs to see.
    let mut files: Vec<&str> = fragments.values().map(|f| f.file.as_str()).collect();
    files.extend(unannotated.keys().map(String::as_str));
    files.sort_unstable();
    files.dedup();

    if output_json {
        for fragment in fragments.values() {
            let qualified = fragment.qualified();
            // `null`, not `0`: absent knowledge rather than a measured zero.
            let count = runs.map(|r| r.get(&qualified).copied().unwrap_or(0));
            let refs = referenced_by.get(&qualified).cloned().unwrap_or_default();
            // `annotated` is the discriminator between this row shape and the
            // unannotated one below, which carries four fields to this one's
            // eleven. Present on **both**, so a consumer branches on a key that
            // is always there rather than probing for the absence of `kind`.
            let json = serde_json::json!({
                "annotated": true,
                "name": fragment.name,
                "file": fragment.file,
                "qualified": qualified,
                "line": fragment.line,
                "kind": fragment.kind,
                "reads": fragment.placeholders,
                "supplies": fragment.supplied_variables,
                "referencedBy": refs,
                "scenarios": count,
                "unused": count.map(|n| n == 0),
            });
            crate::render::outln!("{json}");
        }
        for (file, lines) in unannotated {
            for line in lines {
                let json = serde_json::json!({
                    "name": serde_json::Value::Null,
                    "file": file,
                    "line": line,
                    "annotated": false,
                });
                crate::render::outln!("{json}");
            }
        }
        return;
    }

    let mut never_run = 0usize;
    for file in &files {
        let in_file: Vec<_> = fragments.values().filter(|f| f.file == *file).collect();
        let blank: &[usize] = &[];
        let bare = unannotated.get(*file).map_or(blank, Vec::as_slice);
        crate::render::outln!("{file}{}", entries(in_file.len() + bare.len()));
        for fragment in &in_file {
            let qualified = fragment.qualified();
            let count = runs.map(|r| r.get(&qualified).copied().unwrap_or(0));
            let refs = referenced_by.get(&qualified);
            let marker = match count {
                // Two death modes, named apart. The second is the dangerous one:
                // the macro warning fires, so it reads as already covered.
                Some(0) => {
                    never_run += 1;
                    match refs {
                        None => "  UNREFERENCED — no macro refs it".to_owned(),
                        Some(names) => format!(
                            "  UNREACHABLE — only `{}`, which no scenario binds",
                            names.join("`, `")
                        ),
                    }
                }
                _ => String::new(),
            };
            let shown = match count {
                Some(n) => format!("{n}×"),
                None => "—".to_owned(),
            };
            crate::render::outln!("  {:<28} {shown}{marker}", fragment.name);
        }
        for line in bare {
            crate::render::outln!("  {:<28} (line {line}) UNANNOTATED — not referenceable", "");
        }
    }

    let bare_total: usize = unannotated.values().map(Vec::len).sum();
    let total = fragments.len() + bare_total;
    // Withheld with the verdicts it depends on, exactly as `macros` withholds
    // its unused tally: "0 never run" from an unbound suite would read as
    // "nothing is dead" when the truth is "not counted".
    let dead_note = match runs {
        Some(_) => format!(" · {never_run} never run"),
        None => String::new(),
    };
    crate::render::outln!(
        "\n{total} entr{} · {} annotated · {bare_total} unannotated{dead_note}",
        if total == 1 { "y" } else { "ies" },
        fragments.len()
    );
}

/// `N entries` suffix for a file header.
fn entries(n: usize) -> String {
    format!("        {n} entr{}", if n == 1 { "y" } else { "ies" })
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
    let fragments = match corpus(config) {
        Ok(fragments) => fragments,
        Err(code) => return code,
    };
    let front = match load_front(path, active_env, run_id, config, &fragments) {
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

    let kinds = crate::registry::step_kinds();
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

    // `--add-to` takes *packs*. The modeline it prepends is a yaml-language-server
    // directive, and the schema it drops beside the file describes the pack
    // schema — neither means anything to a `.hurl` corpus, and writing one is a
    // straight violation of ADR-0018's "fragment files are inputs proef never
    // writes". `fmt` learned this predicate in #44; this call site did not, and a
    // guard bolted in front of one writer leaves the next to rediscover it.
    let refused: Vec<&PathBuf> = add_to
        .iter()
        .filter(|p| !crate::fmt::is_pack_file(p))
        .collect();
    if !refused.is_empty() {
        for path in &refused {
            crate::render::errln!(
                "error: {} is not a pack — `schema --add-to` takes `.yaml`/`.yml` pack files",
                path.display()
            );
        }
        crate::render::errln!(
            "help: fragment files are inputs proef never writes (ADR-0018); point `--add-to` at the pack that `ref:`s them"
        );
        return ExitCode::UserError;
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
