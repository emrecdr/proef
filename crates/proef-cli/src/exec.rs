//! `proef test` execution: validate (front end), then run via the core
//! orchestrator with the hurl engine — run records under
//! `.proef-runs/<run-id>/` (events.jsonl **is** the record, ADR-0008),
//! Ctrl-C graceful/hard (ADR-0007), state persisted atomically (ADR-0005).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use proef_core::cancel::CancellationToken;
use proef_core::engine::{ArtifactRef, EngineFactory, HttpDefaults};
use proef_core::error::ExitCode;
use proef_core::event::{Event, EventSink};
use proef_core::lower::LowerCtx;
use proef_core::report::{ConsoleReporter, JsonlReporter, Redactions, Reporter};
use proef_core::resolve::ResolveMode;
use proef_core::runner::{self, Prepared, RunConfig, ScenarioSpec};
use proef_core::world::GlobalStore;
use proef_core::{emit, lower};

use crate::config::ProjectConfig;
use crate::front::{self, FrontEnd};
use crate::{OutputFormat, registry, render};

/// How many run records to keep (TECH-SPEC §11).
const RUN_RETENTION: usize = 200;

/// A scenario tagged `@quarantine` runs and reports, but its test-failure does
/// not gate the exit code (`RunSummary::exit_code_excluding`).
const QUARANTINE_TAG: &str = "quarantine";

/// Run the suite. Exit codes: 0 ok · 1 test failure · 2 user error · 3 system
/// error (ADR-0009, worst-wins across scenarios).
// One cohesive listing of the run lifecycle; splitting hides the order. The
// flat parameter list mirrors the CLI flag surface one-to-one.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub fn execute(
    path: &Path,
    tags: Option<&proef_core::tags::TagExpr>,
    jobs: Option<usize>,
    output: Option<OutputFormat>,
    junit: Option<&str>,
    scenario_filter: Option<&str>,
    scenario_file_filter: Option<&str>,
    active_env: Option<&str>,
    run_id: Option<String>,
    rerun: bool,
    config: &ProjectConfig,
    external_cancel: Option<CancellationToken>,
) -> ExitCode {
    // Resolve the active environment once. All three calls consult `env_profile`,
    // so any of them surfaces an unknown `--env` (user error); the match below
    // reports the first such error.
    let (config_vars, effective_jobs, http_defaults, sla_thresholds) = match (
        config.config_vars(active_env),
        config.jobs(jobs, active_env),
        config.http_defaults(active_env),
        config.sla_thresholds(active_env),
    ) {
        (Ok(vars), Ok(jobs), Ok(http), Ok(sla)) => (Arc::new(vars), jobs, http, sla),
        (Err(message), ..)
        | (_, Err(message), ..)
        | (_, _, Err(message), _)
        | (_, _, _, Err(message)) => {
            eprintln!("error: {message}");
            return ExitCode::UserError;
        }
    };

    // Phase 1: full validation pass (fail fast on static errors, discover
    // secrets, produce the run id).
    let mut front = match front::run(path, ResolveMode::DryRun, run_id, Arc::clone(&config_vars)) {
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
    let secrets = match crate::secretstore::resolve_all(&secret_names) {
        Ok(secrets) => Arc::new(secrets),
        Err(missing) => {
            eprintln!("error: missing secret value(s): {}", missing.join(", "));
            return ExitCode::UserError;
        }
    };

    // Run directory. Rotation happens before the new dir exists so the
    // in-flight run can never be a rotation candidate.
    let runs_root = PathBuf::from(config.runs_dir());

    // `--rerun`: read the prior run's failures BEFORE this run's dir exists, so
    // `latest_run` sees the previous run, not this one. No prior record is a
    // user error; a clean prior run has nothing to rerun (exit 0).
    let rerun_set = if rerun {
        let Some(dir) = crate::record::resolve_dir(&runs_root, None) else {
            eprintln!("error: --rerun found no prior run record to rerun from");
            return ExitCode::UserError;
        };
        match crate::record::failed_scenarios(&dir) {
            Ok(failed) if failed.is_empty() => {
                crate::render::outln!("no failed scenarios in the last run — nothing to rerun");
                return ExitCode::Success;
            }
            Ok(failed) => Some(failed),
            Err(err) => {
                eprintln!("error: {err}");
                return ExitCode::UserError;
            }
        }
    } else {
        None
    };

    rotate_runs(&runs_root, front.run_id.as_ref());
    let run_dir = runs_root.join(front.run_id.as_ref());
    let artifacts_dir = run_dir.join("artifacts");
    if let Err(err) = std::fs::create_dir_all(&artifacts_dir) {
        eprintln!("error: cannot create run dir {}: {err}", run_dir.display());
        return ExitCode::SystemError;
    }

    // Reporters: console (stdout + run.log tee) and the JSONL record.
    let events_file = match std::fs::File::create(run_dir.join("events.jsonl")) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("error: cannot create events.jsonl: {err}");
            return ExitCode::SystemError;
        }
    };
    let log_file = match std::fs::File::create(run_dir.join("run.log")) {
        Ok(file) => Some(file),
        Err(err) => {
            // Best-effort mirror — the run proceeds, but never silently.
            eprintln!("warning: cannot create run.log (console mirror disabled): {err}");
            None
        }
    };
    let redactions = Redactions::new(secrets.values().cloned());
    // A machine format (`--output json`/`tap`) owns stdout exclusively — json
    // must be pipeable into jq, tap into `prove` — so the human report moves to
    // stderr in that mode.
    let machine_stdout = output.is_some();
    let console_out: Box<dyn Write + Send> = if machine_stdout {
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
    // Stamp scenario events with run-relative timing + worker index at the sink
    // (ADR-0015): the clock and thread-id reads live here, at the CLI edge, so
    // the core stays sans-IO. `stamp` runs on the emitting worker thread.
    let sink = stamp_scenario_timing(proef_core::report::sink(reporters, redactions.clone()));

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

    let engines = Arc::new(registry::engines());

    // Suite-level setup/teardown (ADR-0014): a feature run once before / after
    // the pool at the CLI edge, sharing the store so its `saveAs: global`
    // promotions reach every scenario. The core runner stays tag-agnostic.
    let setup_path = config.setup().map(PathBuf::from);
    let teardown_path = config.teardown().map(PathBuf::from);

    // Setup runs first and merges its globals *before* the pool snapshots the
    // store. A setup failure aborts here, never masked — a broken fixture is a
    // user/system fault, not a test failure that would gate on exit 1.
    if let Some(setup) = &setup_path {
        match run_phase(
            "setup",
            setup,
            &front.run_id,
            &config_vars,
            &http_defaults,
            &store,
            &engines,
            &sink,
            &cancel,
            &artifacts_dir,
        ) {
            Err(code) => return code,
            Ok(summary) => {
                if let Some(code) = phase_failed(&summary, ExitCode::UserError) {
                    eprintln!("error: setup failed — aborting before the suite runs");
                    return code;
                }
            }
        }
    }

    // A setup/teardown feature must not also run as an ordinary suite scenario.
    exclude_phase_features(&mut front, setup_path.as_ref(), teardown_path.as_ref());

    let specs = build_specs(
        &front,
        tags,
        scenario_filter,
        scenario_file_filter,
        rerun_set.as_deref(),
        &artifacts_dir,
    );
    let selected = specs.len();
    if selected == 0 {
        // A typo'd --tags/--scenario passing CI with zero tests run is the
        // silent-green failure mode; make it loud (ADR-0009: user error).
        return front::no_scenarios_matched();
    }
    let status_line = format!(
        "running {selected} scenario(s) with {effective_jobs} job(s) — run {}",
        front.run_id
    );
    if machine_stdout {
        eprintln!("{status_line}");
    } else {
        crate::render::outln!("{status_line}");
    }

    let run_config = RunConfig {
        run_id: Arc::clone(&front.run_id),
        jobs: effective_jobs,
        default_batch_budget: Duration::from_millis(
            http_defaults.timeout_ms.saturating_mul(4).max(60_000),
        ),
        secrets,
        http: http_defaults,
    };
    let summary = runner::run(specs, &engines, &store, &run_config, &sink, &cancel);

    // Suite-level teardown: runs after the pool (setup succeeded, or there was
    // none). Its failure is a distinct non-zero signal (exit 3), never a
    // silently-green suite — the suite's own verdict still stands.
    let mut teardown_exit = ExitCode::Success;
    if let Some(teardown) = &teardown_path {
        match run_phase(
            "teardown",
            teardown,
            &front.run_id,
            &config_vars,
            &http_defaults,
            &store,
            &engines,
            &sink,
            &cancel,
            &artifacts_dir,
        ) {
            Err(_) => teardown_exit = ExitCode::SystemError,
            Ok(summary) => {
                if phase_failed(&summary, ExitCode::SystemError).is_some() {
                    eprintln!(
                        "error: teardown failed — cleanup did not complete (the suite verdict stands)"
                    );
                    teardown_exit = ExitCode::SystemError;
                }
            }
        }
    }

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
                if let Some(hint) = &step.reproduce_hint {
                    eprintln!("    curl: {}", redactions.apply(hint));
                }
            }
        }
        // The artifact is re-executable — hand the exact command over. The
        // slug travels with the outcome; the emitter's naming is never
        // re-derived here (it would silently drift on emitter changes).
        if outcome.status == proef_core::step::Status::Failed
            && let Some(slug) = &outcome.artifact_slug
        {
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
    // GitHub annotations render each failure in the PR diff gutter. They are
    // stdout workflow commands, so emit only under Actions and only when the
    // human report (not `--output json`) owns stdout.
    if !machine_stdout && std::env::var_os("GITHUB_ACTIONS").is_some() {
        let annotations = crate::ci_reports::github_annotations(&summary, &redactions);
        if !annotations.is_empty() {
            crate::render::outln!("{}", annotations.trim_end());
        }
    }

    // `@quarantine` scenarios run and report, but their test-failures do not
    // gate the run (a System/User fault still does — quarantine is for flaky
    // tests, not broken input or infra).
    let non_gating: Vec<(String, String)> = front
        .features
        .iter()
        .flat_map(|feature| {
            feature
                .scenarios
                .iter()
                .filter(|scenario| {
                    scenario
                        .lowered
                        .tags
                        .iter()
                        .any(|tag| tag == QUARANTINE_TAG)
                })
                .map(move |scenario| (feature.file.path.clone(), scenario.lowered.name.clone()))
        })
        .collect();
    let quarantined_failures = summary
        .outcomes
        .iter()
        .filter(|o| o.status == proef_core::step::Status::Failed && o.fault.is_none())
        .filter(|o| {
            non_gating.iter().any(|(file, name)| {
                file.as_str() == o.file.as_ref() && name.as_str() == o.name.as_ref()
            })
        })
        .count();
    if quarantined_failures > 0 {
        eprintln!(
            "note: {quarantined_failures} quarantined scenario(s) failed but did not gate the run"
        );
    }

    // Run-level SLA gate (opt-in via `proef.toml [sla]`). A breach prints on
    // stderr and folds into the exit code as a test failure — but only when the
    // run is otherwise clean, so it never downgrades a `User`/`System` fault.
    // With no `[sla]` table configured this is inert (exit unchanged).
    let base_exit = summary.exit_code_excluding(&non_gating);
    let exit = match crate::sla::check(&summary, sla_thresholds) {
        Some(report) if base_exit == ExitCode::Success => {
            eprintln!("{report}");
            ExitCode::TestFailure
        }
        Some(report) => {
            eprintln!("{report}");
            base_exit
        }
        None => base_exit,
    };
    // A teardown/cleanup fault (exit 3) outranks any test result.
    let exit = if teardown_exit.code() > exit.code() {
        teardown_exit
    } else {
        exit
    };

    match output {
        Some(OutputFormat::Json) => {
            let json = serde_json::json!({
                "run_id": front.run_id.as_ref(),
                "passed": summary.passed,
                "failed": summary.failed,
                "skipped": summary.skipped,
                "exit_code": exit.code(),
                "events": run_dir.join("events.jsonl").display().to_string(),
            });
            crate::render::outln!("{json}");
        }
        // TAP v13 from the run's own scenario outcomes (one test point each),
        // quarantined scenarios mapped to `# TODO`; redacted like every sink.
        Some(OutputFormat::Tap) => {
            let tap = crate::tap::render(&summary.outcomes, &non_gating, &redactions);
            crate::render::outln!("{}", tap.trim_end());
        }
        None => {}
    }

    if junit_failed {
        return ExitCode::SystemError;
    }
    exit
}

/// Wrap `inner` so scenario lifecycle events are stamped with run-relative
/// timing (`timestamp_ms`) and a stable 0-based `worker` index (ADR-0015). The
/// clock and thread-id reads happen here, in the CLI, on the emitting worker
/// thread — the sans-IO core never sees them. Non-scenario events pass through.
fn stamp_scenario_timing(inner: EventSink) -> EventSink {
    let start = Instant::now();
    let workers: Arc<Mutex<HashMap<ThreadId, u64>>> = Arc::new(Mutex::new(HashMap::new()));
    EventSink::new(move |event| {
        let now_ms = || u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let worker_index = || {
            let mut map = workers.lock().unwrap_or_else(PoisonError::into_inner);
            let next = u64::try_from(map.len()).unwrap_or(u64::MAX);
            *map.entry(std::thread::current().id()).or_insert(next)
        };
        match event {
            Event::ScenarioStarted { scenario, file, .. } => inner.emit(&Event::ScenarioStarted {
                scenario: scenario.clone(),
                file: file.clone(),
                timestamp_ms: Some(now_ms()),
                worker: Some(worker_index()),
            }),
            // `ScenarioFinished` is emitted from the main dispatcher thread, not
            // the worker that ran the scenario, so it carries only the end
            // timestamp — the worker identity comes from `ScenarioStarted`, which
            // *is* emitted on the worker thread. (The end time is the dispatcher's
            // processing instant, a close approximation — ADR-0015.)
            Event::ScenarioFinished {
                scenario,
                file,
                status,
                ..
            } => inner.emit(&Event::ScenarioFinished {
                scenario: scenario.clone(),
                file: file.clone(),
                status: *status,
                timestamp_ms: Some(now_ms()),
                worker: None,
            }),
            other => inner.emit(other),
        }
    })
}

/// Run a suite-level setup/teardown feature once, sequentially, against the
/// shared store/engines/sink (ADR-0014). `saveAs: global` promotions merge into
/// the shared store (so setup's state reaches the pool); the phase resolves its
/// own secrets. Returns its summary, or an exit code when the feature itself is
/// invalid (validation / missing secret) — surfaced immediately, never masked.
#[allow(clippy::too_many_arguments)]
fn run_phase(
    label: &str,
    path: &Path,
    run_id: &Arc<str>,
    config_vars: &Arc<BTreeMap<String, String>>,
    http: &HttpDefaults,
    store: &Arc<Mutex<GlobalStore>>,
    engines: &Arc<Vec<Box<dyn EngineFactory>>>,
    sink: &EventSink,
    cancel: &CancellationToken,
    artifacts_dir: &Path,
) -> Result<runner::RunSummary, ExitCode> {
    // ADR-0014: `[run] setup`/`teardown` names exactly one feature file. A
    // directory would run every feature under it as the phase AND leave them
    // in the pool (exclude_phase_features matches a single file path), running
    // each scenario twice. Reject it loudly instead of silently double-running.
    if path.is_dir() {
        eprintln!(
            "error: [run] {label} must be a feature file, not a directory ({})",
            path.display()
        );
        return Err(ExitCode::UserError);
    }

    let front = front::run(
        path,
        ResolveMode::DryRun,
        Some(run_id.to_string()),
        Arc::clone(config_vars),
    )
    .map_err(|err| {
        eprintln!("error: {label} feature failed to validate:");
        crate::commands::report_front_error(&err)
    })?;
    render::print_all(&front.warnings);

    let names: BTreeSet<String> = front
        .features
        .iter()
        .flat_map(|feature| feature.scenarios.iter())
        .flat_map(|scenario| scenario.lowered.secrets.iter().cloned())
        .collect();
    let secrets = match crate::secretstore::resolve_all(&names) {
        Ok(secrets) => Arc::new(secrets),
        Err(missing) => {
            eprintln!(
                "error: {label} missing secret value(s): {}",
                missing.join(", ")
            );
            return Err(ExitCode::UserError);
        }
    };

    let specs = build_specs(&front, None, None, None, None, artifacts_dir);
    if specs.is_empty() {
        eprintln!(
            "error: {label} feature `{}` has no scenarios",
            path.display()
        );
        return Err(ExitCode::UserError);
    }
    let run_config = RunConfig {
        run_id: Arc::clone(run_id),
        jobs: 1, // setup/teardown run sequentially
        default_batch_budget: Duration::from_millis(http.timeout_ms.saturating_mul(4).max(60_000)),
        secrets,
        http: *http,
    };
    Ok(runner::run(
        specs,
        engines,
        store,
        &run_config,
        sink,
        cancel,
    ))
}

/// Classify a setup/teardown summary: `None` when every scenario passed, else
/// the exit code to surface. A `System` fault → exit 3; a `User` fault → exit 2;
/// a plain test failure → `on_test_failure` (a broken *setup* is a user error,
/// a broken *teardown* a system/cleanup fault — never a test failure, which
/// would misread as the API under test being broken; ADR-0014).
fn phase_failed(summary: &runner::RunSummary, on_test_failure: ExitCode) -> Option<ExitCode> {
    let mut worst: Option<ExitCode> = None;
    for outcome in &summary.outcomes {
        let code = match (&outcome.fault, outcome.status) {
            (Some(runner::Fault::System(_)), _) => Some(ExitCode::SystemError),
            (Some(runner::Fault::User(_)), _) => Some(ExitCode::UserError),
            (None, proef_core::step::Status::Failed) => Some(on_test_failure),
            _ => None,
        };
        if let Some(code) = code
            && worst.is_none_or(|w| code.code() > w.code())
        {
            worst = Some(code);
        }
    }
    worst
}

/// Drop the setup/teardown feature(s) from the main suite so they never also
/// run as ordinary scenarios (ADR-0014). Matching is on canonical paths, so it
/// is robust whether the phase feature lives inside or outside the suite dir.
fn exclude_phase_features(
    front: &mut FrontEnd,
    setup: Option<&PathBuf>,
    teardown: Option<&PathBuf>,
) {
    let phase_files: Vec<PathBuf> = [setup, teardown]
        .into_iter()
        .flatten()
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .collect();
    if phase_files.is_empty() {
        return;
    }
    front.features.retain(|feature| {
        std::fs::canonicalize(feature.file.path.as_str())
            .ok()
            .is_none_or(|canonical| !phase_files.contains(&canonical))
    });
}

/// Build one `ScenarioSpec` per tag-selected scenario. The prepare closure
/// re-lowers with the **live** World (ADR-0005 lower-time globals), emits the
/// artifact, writes it into the run dir, and hands the same bytes to the
/// engine (ADR-0010).
fn build_specs(
    front: &FrontEnd,
    tags: Option<&proef_core::tags::TagExpr>,
    scenario_filter: Option<&str>,
    scenario_file_filter: Option<&str>,
    rerun_set: Option<&[(String, String)]>,
    artifacts_dir: &Path,
) -> Vec<ScenarioSpec> {
    let mut specs = Vec::new();
    for feature in &front.features {
        // `--scenario-file`: exact-path match (as printed by `proef flows`) —
        // scopes a name filter to one file so duplicate scenario names across
        // features stay one-Trial-one-scenario (US-12).
        if let Some(file) = scenario_file_filter
            && feature.file.path != file
        {
            continue;
        }
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
            // `--rerun`: keep only scenarios that failed in the prior run, keyed
            // on the run-wide (file, name) identity.
            if let Some(rerun) = rerun_set
                && !rerun.iter().any(|(file, name)| {
                    file.as_str() == feature.file.path.as_str()
                        && name.as_str() == scenario.lowered.name.as_str()
                })
            {
                continue;
            }
            let bound = scenario.bound.clone();
            let packs = Arc::clone(&front.packs);
            let env = Arc::clone(&front.env);
            let config_vars = Arc::clone(&front.config_vars);
            let kind_to_engine = Arc::clone(&front.kind_to_engine);
            let run_id = Arc::clone(&front.run_id);
            let feature_file = Arc::clone(&feature_arc);
            let stem = Arc::clone(&stem);
            let artifacts_dir = artifacts_dir.to_path_buf();
            let prepare: runner::PrepareFn = Box::new(move |world| {
                let ctx = LowerCtx {
                    feature: &feature_file,
                    packs: &packs,
                    kind_to_engine: &kind_to_engine,
                    env: &env,
                    config_vars: &config_vars,
                    run_id: &run_id,
                    world,
                    mode: ResolveMode::Strict,
                };
                let lowered = lower::lower(&bound, &ctx)?;
                let artifact = emit::emit(&lowered, &stem, world).map(|artifact| {
                    // The run dir holds the exact executed bytes. Record
                    // writes are best-effort (the run proceeds) but never
                    // silent — the record is the debugging surface.
                    write_or_warn(
                        &artifacts_dir.join(format!("{}.hurl", artifact.slug)),
                        &artifact.hurl_text,
                    );
                    if let Ok(map_json) = serde_json::to_string_pretty(&artifact.map) {
                        write_or_warn(
                            &artifacts_dir.join(format!("{}.map.json", artifact.slug)),
                            format!("{map_json}\n"),
                        );
                    }
                    if let Some(vars) = &artifact.vars {
                        write_or_warn(&artifacts_dir.join(format!("{}.vars", artifact.slug)), vars);
                    }
                    // Referenced `file,…;` assets ride along so the run-dir
                    // artifact replays under stock hurl (ADR-0010 hand-off).
                    // The run itself is unaffected (the engine reads bodies
                    // from the suite, fenced by its context dir) — an
                    // incomplete run record is a warning, not a failure.
                    if let Some(root) = Path::new(feature_file.path.as_str()).parent()
                        && let Err(err) =
                            crate::assets::copy_assets(&artifact.hurl_text, root, &artifacts_dir)
                    {
                        eprintln!("warning: run record for {}.hurl: {err}", artifact.slug);
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
                file_root: Path::new(feature.file.path.as_str())
                    .parent()
                    .map(Path::to_path_buf),
                prepare,
            });
        }
    }
    specs
}

/// Keep the newest [`RUN_RETENTION`] run records (uuid-v7 names sort by
/// time). Only directories *named by a run id* are candidates — `runs-dir`
/// may be `.` or otherwise shared with user content, and rotation must never
/// Best-effort run-record write: the run proceeds on failure, but never
/// silently (matches the asset-copy warning path in the same closure).
fn write_or_warn(path: &Path, contents: impl AsRef<[u8]>) {
    if let Err(err) = std::fs::write(path, contents) {
        eprintln!("warning: cannot write {}: {err}", path.display());
    }
}

/// touch anything proef did not create — and the in-flight run never is.
fn rotate_runs(runs_dir: &Path, current_run: &str) {
    let Ok(entries) = std::fs::read_dir(runs_dir) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            p.file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name != current_run && crate::fsutil::is_run_id(name))
        })
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
