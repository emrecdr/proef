//! `proef test` execution: validate (front end), then run via the core
//! orchestrator with the hurl engine — run records under
//! `.proef-runs/<run-id>/` (events.jsonl **is** the record, ADR-0008),
//! Ctrl-C graceful/hard (ADR-0007), state persisted atomically (ADR-0005).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use proef_core::cancel::CancellationToken;
use proef_core::engine::ArtifactRef;
use proef_core::error::ExitCode;
use proef_core::lower::LowerCtx;
use proef_core::report::{ConsoleReporter, JsonlReporter, Redactions, Reporter};
use proef_core::resolve::ResolveMode;
use proef_core::runner::{self, Prepared, RunConfig, ScenarioSpec};
use proef_core::world::GlobalStore;
use proef_core::{emit, lower};

use crate::config::ProjectConfig;
use crate::front::{self, FrontEnd};
use crate::{registry, render};

/// How many run records to keep (TECH-SPEC §11).
const RUN_RETENTION: usize = 200;

/// Secret resolution order (US-10): `PROEF_SECRET_<NAME>` environment
/// override → the encrypted store (`proef secret set`).
fn collect_secrets(names: &BTreeSet<String>) -> Result<BTreeMap<String, String>, Vec<String>> {
    let mut secrets = BTreeMap::new();
    let mut missing = Vec::new();
    for name in names {
        let env_key = format!("PROEF_SECRET_{}", name.to_uppercase());
        if let Ok(value) = std::env::var(&env_key) {
            secrets.insert(name.clone(), value);
            continue;
        }
        match crate::secretstore::resolve(name) {
            Ok(Some(value)) => {
                secrets.insert(name.clone(), value);
            }
            Ok(None) => missing.push(format!(
                "`{name}` (run `proef secret set {name}`, or set {env_key})"
            )),
            Err(message) => missing.push(format!("`{name}` ({message})")),
        }
    }
    if missing.is_empty() {
        Ok(secrets)
    } else {
        Err(missing)
    }
}

/// Run the suite. Exit codes: 0 ok · 1 test failure · 2 user error · 3 system
/// error (ADR-0009, worst-wins across scenarios).
// One cohesive listing of the run lifecycle; splitting hides the order.
#[allow(clippy::too_many_lines)]
pub fn execute(
    path: &Path,
    tags: &[String],
    jobs: Option<usize>,
    output_json: bool,
    junit: Option<&str>,
    scenario_filter: Option<&str>,
    external_cancel: Option<CancellationToken>,
) -> ExitCode {
    let config = match ProjectConfig::load() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::UserError;
        }
    };

    // Phase 1: full validation pass (fail fast on static errors, discover
    // secrets, produce the run id).
    let front = match front::run(path, ResolveMode::DryRun, None) {
        Ok(front) => front,
        Err(err) => return crate::commands::report_front_error(&err),
    };
    render::print_all(&front.warnings);

    let secret_names: BTreeSet<String> = front
        .features
        .iter()
        .flat_map(|f| f.scenarios.iter())
        .flat_map(|s| s.lowered.secrets.iter().cloned())
        .collect();
    let secrets = match collect_secrets(&secret_names) {
        Ok(secrets) => Arc::new(secrets),
        Err(missing) => {
            eprintln!("error: missing secret value(s): {}", missing.join(", "));
            return ExitCode::UserError;
        }
    };

    // Run directory.
    let runs_root = PathBuf::from(config.runs_dir());
    let run_dir = runs_root.join(front.run_id.as_ref());
    let artifacts_dir = run_dir.join("artifacts");
    if let Err(err) = std::fs::create_dir_all(&artifacts_dir) {
        eprintln!("error: cannot create run dir {}: {err}", run_dir.display());
        return ExitCode::SystemError;
    }
    rotate_runs(&runs_root);

    // Reporters: console (stdout + run.log tee) and the JSONL record.
    let events_file = match std::fs::File::create(run_dir.join("events.jsonl")) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("error: cannot create events.jsonl: {err}");
            return ExitCode::SystemError;
        }
    };
    let log_file = std::fs::File::create(run_dir.join("run.log")).ok();
    let redactions = Redactions::new(secrets.values().cloned());
    // Machine output owns stdout exclusively (`--output json` must be
    // pipeable into jq); the human report moves to stderr in that mode.
    let console_out: Box<dyn Write + Send> = if output_json {
        Box::new(std::io::stderr())
    } else {
        Box::new(std::io::stdout())
    };
    let reporters: Vec<Box<dyn Reporter>> = vec![
        Box::new(ConsoleReporter::new(
            Tee(console_out, log_file),
            redactions.clone(),
        )),
        Box::new(JsonlReporter::new(events_file)),
    ];
    // Redaction is applied once at the sink boundary (before fan-out), so the
    // JSONL record and any future reporter are covered, not just the console.
    let sink = proef_core::report::sink(reporters, redactions.clone());

    // Ctrl-C: first = graceful cancel, second = hard exit (ADR-0007). Under
    // `--watch` the loop owns the handler and hands us its token instead.
    let cancel = external_cancel.unwrap_or_else(|| {
        let cancel = CancellationToken::new();
        let handler_token = cancel.clone();
        let once = AtomicBool::new(false);
        let _ = ctrlc::set_handler(move || {
            if once.swap(true, Ordering::SeqCst) {
                eprintln!("\nsecond interrupt — hard exit");
                std::process::exit(130);
            }
            eprintln!("\ninterrupt — cancelling after current batches (Ctrl-C again to force)");
            handler_token.cancel();
        });
        cancel
    });

    // Shared global store (scenario merge-back through the lock, §12).
    let store = match GlobalStore::load(Path::new(".proef-state.json")) {
        Ok(store) => Arc::new(Mutex::new(store)),
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::SystemError;
        }
    };

    let specs = build_specs(&front, tags, scenario_filter, &artifacts_dir);
    let selected = specs.len();
    if selected == 0 {
        // A typo'd --tags/--scenario passing CI with zero tests run is the
        // silent-green failure mode; make it loud (ADR-0009: user error).
        eprintln!("error: no scenarios matched the filters (check --tags/--scenario)");
        return ExitCode::UserError;
    }
    let status_line = format!(
        "running {selected} scenario(s) with {} job(s) — run {}",
        config.jobs(jobs),
        front.run_id
    );
    if output_json {
        eprintln!("{status_line}");
    } else {
        println!("{status_line}");
    }

    let run_config = RunConfig {
        run_id: Arc::clone(&front.run_id),
        jobs: config.jobs(jobs),
        default_batch_budget: Duration::from_millis(
            config
                .http_defaults()
                .timeout_ms
                .saturating_mul(4)
                .max(60_000),
        ),
        secrets,
        http: config.http_defaults(),
    };
    let engines = Arc::new(registry::engines());
    let summary = runner::run(specs, &engines, &store, &run_config, &sink, &cancel);

    // Persist the World (atomic temp+rename, 0600 — ADR-0005).
    if let Ok(guard) = store.lock()
        && let Err(err) = guard.save(Path::new(".proef-state.json"))
    {
        eprintln!("warning: cannot persist global state: {err}");
    }

    // Failure details (feature line + artifact span already inside details).
    // Redacted like every other sink — engine details are pre-redacted, but
    // fault messages can quote resolved user input.
    for outcome in &summary.outcomes {
        if let Some(fault) = &outcome.fault {
            let (kind, message) = match fault {
                runner::Fault::User(message) => ("user error", message),
                runner::Fault::System(message) => ("system error", message),
            };
            eprintln!(
                "{kind}: {}:{} {} — {}",
                outcome.file,
                outcome.line,
                outcome.name,
                redactions.apply(message)
            );
        }
        for step in &outcome.steps {
            if step.status == proef_core::step::Status::Failed
                && let Some(detail) = &step.detail
            {
                eprintln!(
                    "  ✗ {}:{} — {}",
                    step.step.file,
                    step.step.line,
                    redactions.apply(detail)
                );
            }
        }
        // The artifact is re-executable — hand the exact command over.
        if outcome.status == proef_core::step::Status::Failed {
            let stem = Path::new(outcome.file.as_ref()).file_stem().map_or_else(
                || "feature".to_owned(),
                |s| s.to_string_lossy().into_owned(),
            );
            let slug = format!("{}--{}", emit::slugify(&stem), emit::slugify(&outcome.name));
            let artifact = artifacts_dir.join(format!("{slug}.hurl"));
            if artifact.exists() {
                let vars = artifacts_dir.join(format!("{slug}.vars"));
                let vars_arg = if vars.exists() {
                    format!(" --variables-file {}", vars.display())
                } else {
                    String::new()
                };
                eprintln!("  reproduce: hurl --test {}{vars_arg}", artifact.display());
            }
        }
    }

    let mut junit_failed = false;
    // CI reports (US-8): JUnit XML + GitHub job summary.
    let junit_path = match junit {
        Some("auto") if std::env::var_os("GITHUB_ACTIONS").is_some() => {
            Some(run_dir.join("report.junit.xml"))
        }
        Some("auto") | None => None,
        Some(path) => Some(PathBuf::from(path)),
    };
    if let Some(junit_path) = junit_path {
        match crate::ci_reports::write_junit(&summary, &front.run_id, &junit_path, &redactions) {
            Ok(()) => eprintln!("junit report: {}", junit_path.display()),
            Err(message) => {
                // A CI job gating on this file must not see exit 0.
                eprintln!("error: {message}");
                junit_failed = true;
            }
        }
    }
    crate::ci_reports::write_github_summary(&summary, &front.run_id, &redactions);

    if output_json {
        let json = serde_json::json!({
            "run_id": front.run_id.as_ref(),
            "passed": summary.passed,
            "failed": summary.failed,
            "skipped": summary.skipped,
            "exit_code": summary.exit_code().code(),
            "events": run_dir.join("events.jsonl").display().to_string(),
        });
        println!("{json}");
    }

    if junit_failed {
        return ExitCode::SystemError;
    }
    summary.exit_code()
}

/// Build one `ScenarioSpec` per tag-selected scenario. The prepare closure
/// re-lowers with the **live** World (ADR-0005 lower-time globals), emits the
/// artifact, writes it into the run dir, and hands the same bytes to the
/// engine (ADR-0010).
fn build_specs(
    front: &FrontEnd,
    tags: &[String],
    scenario_filter: Option<&str>,
    artifacts_dir: &Path,
) -> Vec<ScenarioSpec> {
    let mut specs = Vec::new();
    for feature in &front.features {
        let file_arc: Arc<str> = Arc::from(feature.file.path.as_str());
        let feature_arc = Arc::new(feature.file.clone());
        let stem: Arc<str> = Arc::from(
            Path::new(feature.file.path.as_str())
                .file_stem()
                .map_or_else(
                    || "feature".to_owned(),
                    |s| s.to_string_lossy().into_owned(),
                )
                .as_str(),
        );
        for scenario in &feature.scenarios {
            if !front::tag_selected(&scenario.lowered.tags, tags) {
                continue;
            }
            if let Some(filter) = scenario_filter
                && scenario.lowered.name != filter
            {
                continue;
            }
            let bound = scenario.bound.clone();
            let packs = Arc::clone(&front.packs);
            let env = Arc::clone(&front.env);
            let kind_to_engine = Arc::clone(&front.kind_to_engine);
            let run_id = Arc::clone(&front.run_id);
            let feature_file = Arc::clone(&feature_arc);
            let stem = Arc::clone(&stem);
            let artifacts_dir = artifacts_dir.to_path_buf();
            let prepare: runner::PrepareFn = Box::new(move |world| {
                let empty = BTreeMap::new();
                let ctx = LowerCtx {
                    feature: &feature_file,
                    packs: &packs,
                    kind_to_engine: &kind_to_engine,
                    env: &env,
                    run_id: &run_id,
                    world,
                    config: &empty,
                    mode: ResolveMode::Strict,
                };
                let lowered = lower::lower(&bound, &ctx)?;
                let artifact = emit::emit(&lowered, &stem, world).map(|artifact| {
                    // The run dir holds the exact executed bytes.
                    let _ = std::fs::write(
                        artifacts_dir.join(format!("{}.hurl", artifact.slug)),
                        &artifact.hurl_text,
                    );
                    if let Ok(map_json) = serde_json::to_string_pretty(&artifact.map) {
                        let _ = std::fs::write(
                            artifacts_dir.join(format!("{}.map.json", artifact.slug)),
                            format!("{map_json}\n"),
                        );
                    }
                    if let Some(vars) = &artifact.vars {
                        let _ = std::fs::write(
                            artifacts_dir.join(format!("{}.vars", artifact.slug)),
                            vars,
                        );
                    }
                    // Referenced `file,…;` assets ride along so the run-dir
                    // artifact replays under stock hurl (ADR-0010 hand-off).
                    if let Some(root) = Path::new(feature_file.path.as_str()).parent() {
                        for asset in emit::file_references(&artifact.hurl_text) {
                            let source = root.join(&asset);
                            let target = artifacts_dir.join(&asset);
                            if source.is_file() {
                                if let Some(parent) = target.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                let _ = std::fs::copy(&source, &target);
                            }
                        }
                    }
                    ArtifactRef {
                        slug: Arc::from(artifact.slug.as_str()),
                        text: Arc::from(artifact.hurl_text.as_str()),
                        map: Arc::new(artifact.map),
                    }
                });
                Ok(Prepared {
                    batches: lowered.batches,
                    artifact,
                })
            });
            specs.push(ScenarioSpec {
                file: Arc::clone(&file_arc),
                name: Arc::from(scenario.lowered.name.as_str()),
                line: scenario.lowered.line,
                tags: scenario.lowered.tags.clone(),
                file_root: Path::new(feature.file.path.as_str())
                    .parent()
                    .map(Path::to_path_buf),
                prepare,
            });
        }
    }
    specs
}

/// Keep the newest [`RUN_RETENTION`] run dirs (uuid-v7 names sort by time).
fn rotate_runs(runs_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(runs_dir) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    if dirs.len() > RUN_RETENTION {
        let excess = dirs.len() - RUN_RETENTION;
        for dir in dirs.into_iter().take(excess) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Console output tee'd into `run.log` (§11 — the human-readable run record).
struct Tee(Box<dyn Write + Send>, Option<std::fs::File>);

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(file) = &mut self.1 {
            let _ = file.write_all(buf);
        }
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(file) = &mut self.1 {
            let _ = file.flush();
        }
        self.0.flush()
    }
}
