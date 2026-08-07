//! `proef test` execution: validate (front end), then run via the core
//! orchestrator with the hurl engine — run records under
//! `.proef-runs/<run-id>/` (events.jsonl **is** the record, ADR-0008),
//! Ctrl-C graceful/hard (ADR-0007), state persisted atomically (ADR-0005).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
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
            crate::render::errln!("error: {message}");
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
            crate::render::errln!("error: missing secret value(s): {}", missing.join(", "));
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
            crate::render::errln!("error: --rerun found no prior run record to rerun from");
            return ExitCode::UserError;
        };
        match crate::record::failed_scenarios(&dir) {
            Ok(failed) if failed.is_empty() => {
                crate::render::outln!("no failed scenarios in the last run — nothing to rerun");
                return ExitCode::Success;
            }
            Ok(failed) => Some(failed),
            Err(err) => {
                crate::render::errln!("error: {err}");
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
        crate::render::errln!("error: cannot create run dir {}: {err}", run_dir.display());
        return ExitCode::SystemError;
    }

    // Reporters: console (stdout + run.log tee) and the JSONL record.
    let events_file = match std::fs::File::create(run_dir.join("events.jsonl")) {
        Ok(file) => file,
        Err(err) => {
            crate::render::errln!("error: cannot create events.jsonl: {err}");
            return ExitCode::SystemError;
        }
    };
    let log_file = match std::fs::File::create(run_dir.join("run.log")) {
        Ok(file) => Some(file),
        Err(err) => {
            // Best-effort mirror — the run proceeds, but never silently.
            crate::render::errln!(
                "warning: cannot create run.log (console mirror disabled): {err}"
            );
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

    // `[run] setup`, the suite, and `[run] teardown` each call `runner::run`,
    // which brackets its own work with `RunStarted`/`RunFinished`. A record
    // must carry exactly one pair overall (ADR-0008), so the phases run
    // against a wrapper that drops that pair, and a `RunRecord` guard owns
    // the single pair for the whole run — opened below and closed by an
    // explicit `drop` after teardown, with `Drop` as the backstop for every
    // early return in between (see `RunRecord`).
    let phase_sink = suppress_run_head_tail(sink.clone());

    // Ctrl-C: first = graceful cancel, second = hard exit (ADR-0007). Under
    // `--watch` the loop owns the handler and hands us its token instead.
    let cancel = external_cancel.unwrap_or_else(|| {
        let cancel = CancellationToken::new();
        let handler_token = cancel.clone();
        let once = AtomicBool::new(false);
        let _ = ctrlc::set_handler(move || {
            if once.swap(true, Ordering::SeqCst) {
                crate::render::errln!("\nsecond interrupt — hard exit");
                std::process::exit(crate::INTERRUPT_EXIT_CODE);
            }
            crate::render::errln!(
                "\ninterrupt — cancelling after current batches (Ctrl-C again to force)"
            );
            handler_token.cancel();
        });
        cancel
    });

    let mut record = RunRecord::open(&sink, &cancel, &front.run_id);

    // Shared global store (scenario merge-back through the lock, §12).
    let store = match GlobalStore::load(Path::new(".proef-state.json")) {
        Ok(store) => Arc::new(Mutex::new(store)),
        Err(err) => {
            crate::render::errln!("error: {err}");
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
            &phase_sink,
            &cancel,
            &artifacts_dir,
        ) {
            Err(code) => return code,
            Ok(summary) => {
                // `RunRecord`'s totals are the main-suite verdict only (ADR-0014):
                // setup's own outcome still drives the exit code below, and its
                // scenarios are still visible as events in the record, but it is
                // never folded into `passed`/`failed`/`skipped` — those must match
                // what JUnit/`--output json`/TAP/the SLA gate/the exit code report.
                if let Some(code) = phase_failed(&summary, ExitCode::UserError) {
                    crate::render::errln!("error: setup failed — aborting before the suite runs");
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
        crate::render::errln!("{status_line}");
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
    let summary = runner::run(specs, &engines, &store, &run_config, &phase_sink, &cancel);
    record.add(&summary);

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
            &phase_sink,
            &cancel,
            &artifacts_dir,
        ) {
            Err(_) => teardown_exit = ExitCode::SystemError,
            Ok(summary) => {
                // Excluded from `record`'s totals for the same reason setup is
                // (above): teardown's own scenario events still land in the
                // record, and its failure still forces the exit code (below),
                // but a green suite with a broken teardown must not misread as
                // the API under test having failed (ADR-0014).
                if phase_failed(&summary, ExitCode::SystemError).is_some() {
                    crate::render::errln!(
                        "error: teardown failed — cleanup did not complete (the suite verdict stands)"
                    );
                    teardown_exit = ExitCode::SystemError;
                }
            }
        }
    }

    // Close the record here, at the same point every phase has finished and
    // before the failure details / JUnit / SLA report below are printed —
    // `ConsoleReporter` keys its `summary:` line off `RunFinished`
    // (report.rs), so closing later would print that line after the failure
    // detail instead of before it. Every early return above this point closes
    // the record too, via `RunRecord::drop`, with whatever totals had
    // accumulated so far — never zero-but-silently-missing.
    drop(record);

    // Persist the World (atomic temp+rename, 0600 — ADR-0005).
    if let Ok(guard) = store.lock()
        && let Err(err) = guard.save(Path::new(".proef-state.json"))
    {
        crate::render::errln!("warning: cannot persist global state: {err}");
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
            crate::render::errln!(
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
                crate::render::errln!(
                    "  ✗ {}:{} — {}",
                    step.step.file,
                    step.step.line,
                    redactions.apply(detail)
                );
                if let Some(hint) = &step.reproduce_hint {
                    crate::render::errln!("    curl: {}", redactions.apply(hint));
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
                crate::render::errln!("  reproduce: hurl --test {}{vars_arg}", artifact.display());
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
            Ok(()) => crate::render::errln!("junit report: {}", junit_path.display()),
            Err(message) => {
                // A CI job gating on this file must not see exit 0.
                crate::render::errln!("error: {message}");
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
        crate::render::errln!(
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
            crate::render::errln!("{report}");
            ExitCode::TestFailure
        }
        Some(report) => {
            crate::render::errln!("{report}");
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
/// clock reads happen here, in the CLI, on the emitting worker thread — the
/// sans-IO core never sees them. Non-scenario events pass through.
fn stamp_scenario_timing(inner: EventSink) -> EventSink {
    let start = Instant::now();
    // `worker` is the 0-based slot the scenario occupied, so the timeline shows
    // occupancy of the `--jobs` workers (ADR-0015). A fresh OS thread is
    // spawned per scenario, so thread identity would yield a per-scenario
    // ordinal instead; slots are assigned on start and released on finish.
    // Release keys on scenario identity because an abandoned scenario's
    // finish is emitted by the watchdog sweep, not by the worker thread.
    let slots: Arc<Mutex<HashMap<(String, String), u64>>> = Arc::new(Mutex::new(HashMap::new()));
    EventSink::new(move |event| {
        let now_ms = || u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let acquire_slot = |scenario: &str, file: &str| {
            let mut map = slots.lock().unwrap_or_else(PoisonError::into_inner);
            let taken: BTreeSet<u64> = map.values().copied().collect();
            // Pigeonhole: with `taken.len()` occupied slots, some slot in
            // `0..=taken.len()` is always free, so bounding here (unlike an
            // unbounded `0u64..`) is exact, not a truncation.
            let bound = u64::try_from(taken.len()).unwrap_or(u64::MAX);
            let slot = (0..=bound).find(|i| !taken.contains(i)).unwrap_or(0);
            map.insert((scenario.to_owned(), file.to_owned()), slot);
            slot
        };
        let release_slot = |scenario: &str, file: &str| {
            let mut map = slots.lock().unwrap_or_else(PoisonError::into_inner);
            map.remove(&(scenario.to_owned(), file.to_owned()));
        };
        match event {
            Event::ScenarioStarted { scenario, file, .. } => inner.emit(&Event::ScenarioStarted {
                scenario: scenario.clone(),
                file: file.clone(),
                timestamp_ms: Some(now_ms()),
                worker: Some(acquire_slot(scenario, file)),
            }),
            // `ScenarioFinished` is emitted from the main dispatcher thread, not
            // the worker that ran the scenario, so it carries only the end
            // timestamp — the worker identity comes from `ScenarioStarted`, which
            // *is* emitted on the worker thread. (The end time is the dispatcher's
            // processing instant, a close approximation — ADR-0015.) Releasing the
            // slot here — keyed on scenario identity, not thread — is what lets an
            // abandoned scenario's watchdog-swept finish (dispatcher thread) free
            // it correctly.
            Event::ScenarioFinished {
                scenario,
                file,
                status,
                ..
            } => {
                release_slot(scenario, file);
                inner.emit(&Event::ScenarioFinished {
                    scenario: scenario.clone(),
                    file: file.clone(),
                    status: *status,
                    timestamp_ms: Some(now_ms()),
                    worker: None,
                });
            }
            other => inner.emit(other),
        }
    })
}

/// A sink that drops `RunStarted`/`RunFinished` and passes everything else
/// through. Each phase calls `runner::run`, which brackets its own work with
/// that pair; a record must carry exactly one pair overall (ADR-0008), so the
/// phases run against this wrapper and `RunRecord` emits the single pair.
fn suppress_run_head_tail(inner: EventSink) -> EventSink {
    EventSink::new(move |event| match event {
        Event::RunStarted { .. } | Event::RunFinished { .. } => {}
        other => inner.emit(other),
    })
}

/// Owns the run's single `RunStarted`/`RunFinished` pair (ADR-0008).
/// `RunRecord::open` emits the head; `Drop` emits the tail — structurally,
/// not by remembering to emit it at every `return` between the two. Totals
/// are the **main-suite verdict only** (ADR-0014): `add` is called once, for
/// the suite's own `RunSummary`, never for `[run] setup`/`teardown` — so
/// `run_finished`'s `passed`/`failed`/`skipped` agree with the console
/// `summary:` line, `explain`, `--output json`, `JUnit`, TAP, the SLA gate, and
/// the exit code, all of which already read the suite alone. Phase outcomes
/// stay fully visible as their own `scenario_started`/`scenario_finished`
/// events, and phase failures still drive the exit code through
/// `phase_failed`, independent of these totals. Whichever path closes the
/// record — the explicit `drop` after teardown, or `Drop` firing on an early
/// return — reports whatever the suite had accumulated, never a phase's
/// counts or a silently-empty tail.
struct RunRecord<'a> {
    sink: &'a EventSink,
    cancel: &'a CancellationToken,
    passed: usize,
    failed: usize,
    skipped: usize,
}

impl<'a> RunRecord<'a> {
    /// Emit `RunStarted` and open the guard that will emit `RunFinished` when
    /// it drops.
    fn open(sink: &'a EventSink, cancel: &'a CancellationToken, run_id: &Arc<str>) -> Self {
        sink.emit(&Event::RunStarted {
            schema: proef_core::event::EVENT_SCHEMA_VERSION,
            run_id: Arc::clone(run_id),
        });
        Self {
            sink,
            cancel,
            passed: 0,
            failed: 0,
            skipped: 0,
        }
    }

    /// Fold the main suite's outcome counts into the run's totals — called
    /// once, for the suite's own `RunSummary` only (see the struct doc).
    fn add(&mut self, summary: &runner::RunSummary) {
        self.passed += summary.passed;
        self.failed += summary.failed;
        self.skipped += summary.skipped;
    }
}

impl Drop for RunRecord<'_> {
    fn drop(&mut self) {
        // A panic unwinding through this frame means the run died mid-flight —
        // emitting a well-formed `RunFinished` here would certify a crashed run
        // as complete (`RunCompletion::Completed`), the exact hole the
        // truncated-record work exists to close. Leave the record without a
        // tail; it reads back as `RunCompletion::Incomplete`.
        if std::thread::panicking() {
            return;
        }
        self.sink.emit(&Event::RunFinished {
            passed: self.passed,
            failed: self.failed,
            skipped: self.skipped,
            cancelled: self.cancel.is_cancelled(),
        });
    }
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
        crate::render::errln!(
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
        crate::render::errln!("error: {label} feature failed to validate:");
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
            crate::render::errln!(
                "error: {label} missing secret value(s): {}",
                missing.join(", ")
            );
            return Err(ExitCode::UserError);
        }
    };

    let specs = build_specs(&front, None, None, None, None, artifacts_dir);
    if specs.is_empty() {
        crate::render::errln!(
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
                    let root = crate::fsutil::parent_dir(Path::new(feature_file.path.as_str()));
                    write_run_record(artifact, &artifacts_dir, &root)
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
                file_root: Some(crate::fsutil::parent_dir(Path::new(
                    feature.file.path.as_str(),
                ))),
                prepare,
            });
        }
    }
    specs
}

/// Write one scenario's run-dir record — the `.hurl`/`.map.json`/`.vars`
/// sidecars plus any referenced assets — and hand back the artifact the
/// engine executes against. The run dir holds the exact executed bytes.
/// Record writes are best-effort (the run proceeds) but never silent — the
/// record is the debugging surface.
fn write_run_record(artifact: emit::Artifact, artifacts_dir: &Path, root: &Path) -> ArtifactRef {
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
    // Referenced `file,…;` assets ride along so the run-dir artifact replays
    // under stock hurl (ADR-0010 hand-off). The run itself is unaffected
    // (the engine reads bodies from the suite, fenced by its context dir) —
    // an incomplete run record is a warning, not a failure.
    if let Err(err) = crate::assets::copy_assets(&artifact.hurl_text, root, artifacts_dir) {
        crate::render::errln!("warning: run record for {}.hurl: {err}", artifact.slug);
    }
    ArtifactRef {
        slug: Arc::from(artifact.slug.as_str()),
        text: Arc::from(artifact.hurl_text.as_str()),
        map: Arc::new(artifact.map),
    }
}

/// Best-effort run-record write: the run proceeds on failure, but never
/// silently (matches the asset-copy warning path in `write_run_record`).
fn write_or_warn(path: &Path, contents: impl AsRef<[u8]>) {
    if let Err(err) = std::fs::write(path, contents) {
        crate::render::errln!("warning: cannot write {}: {err}", path.display());
    }
}

/// Keep the newest [`RUN_RETENTION`] run records (uuid-v7 names sort by
/// time). Only directories *named by a run id* are candidates — `runs-dir`
/// may be `.` or otherwise shared with user content, and rotation must never
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
        // Console first: the mirror copies what the console accepted, so a
        // short write cannot duplicate the tail into `run.log`.
        let written = self.0.write(buf)?;
        if let Some(file) = &mut self.1 {
            let _ = file.write_all(&buf[..written]);
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(file) = &mut self.1 {
            let _ = file.flush();
        }
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::panic::AssertUnwindSafe;
    use std::sync::{Arc, Mutex};

    use super::RunRecord;
    use proef_core::cancel::CancellationToken;
    use proef_core::event::{Event, EventSink};

    /// A run that panics mid-flight must stay `RunCompletion::Incomplete`, not
    /// certify as complete. Nothing sets `panic = "abort"`, so unwind is live —
    /// a panic reaching `execute`'s frame runs `RunRecord::drop` while
    /// unwinding, and this pins that `Drop::drop`'s `thread::panicking()` guard
    /// really does suppress the tail `RunFinished` in that case, not just in
    /// principle. `catch_unwind` (rather than `#[should_panic]`) lets the test
    /// inspect what the sink received *after* the unwind, which is the whole
    /// point.
    #[test]
    fn drop_during_panic_does_not_emit_run_finished() {
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = {
            let events = Arc::clone(&events);
            EventSink::new(move |event| events.lock().unwrap().push(event.clone()))
        };
        let cancel = CancellationToken::new();
        let run_id: Arc<str> = Arc::from("test-run");

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _record = RunRecord::open(&sink, &cancel, &run_id);
            panic!("simulated mid-run crash");
        }));
        assert!(result.is_err(), "the closure must have panicked");

        let events = events.lock().unwrap();
        assert!(
            events.iter().any(|e| matches!(e, Event::RunStarted { .. })),
            "RunStarted must still have been emitted by `open`: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::RunFinished { .. })),
            "a panicking drop must never emit RunFinished: {events:?}"
        );
    }

    #[test]
    fn the_tee_mirrors_only_the_bytes_the_console_accepted() {
        use std::io::Write;

        use super::Tee;

        /// A console that accepts three bytes per call, like a pipe under
        /// pressure — `write_all` then loops on the remainder.
        struct ShortWriter(Vec<u8>);
        impl Write for ShortWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let take = buf.len().min(3);
                self.0.extend_from_slice(&buf[..take]);
                Ok(take)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("run.log");
        let file = std::fs::File::create(&path).expect("create");
        let mut tee = Tee(Box::new(ShortWriter(Vec::new())), Some(file));

        tee.write_all(b"abcdefghij").expect("write_all");
        tee.flush().expect("flush");
        drop(tee);

        let mirrored = std::fs::read_to_string(&path).expect("read");
        assert_eq!(
            mirrored, "abcdefghij",
            "the mirror must be a faithful copy of what the console received"
        );
    }
}
