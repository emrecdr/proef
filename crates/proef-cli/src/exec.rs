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

/// The persistent World's file name. Where it *sits* is
/// [`ProjectConfig::state_file`](crate::config::ProjectConfig::state_file)'s
/// answer, not this module's — the store belongs to the project, not to the
/// directory the shell happens to be in.
pub const STATE_FILE: &str = ".proef-state.json";

/// How many run records to keep when `[run] keep-runs` says nothing
/// (TECH-SPEC §11).
///
/// Sized for an archive rather than a laptop, which is why it is now only the
/// default: a record costs roughly its artifacts plus its event stream, and a
/// suite re-run on every save reaches the ceiling in a day of work while
/// wanting about five of them.
pub(crate) const DEFAULT_KEEP_RUNS: usize = 200;

/// Tell a first-time reader that a failed run never reached a target, rather
/// than leaving them with a bare connection error.
///
/// **The config literal alone proves nothing.** `[url] base` still equal to what
/// `init` writes looks init-specific and is not: `GETTING-STARTED` teaches that
/// exact line to people building a suite by hand, and proef's own `proef.toml`
/// uses it. Keyed on that alone, this note fired on a hand-built suite whose
/// server was up and whose assertion genuinely failed — every clause of it
/// false, moments after the suite reached a real verdict.
///
/// So the deciding evidence is the run itself: it fires only when **nothing was
/// reachable** — no scenario passed and every outcome carries a system fault,
/// which is what a connection failure produces. A suite that got an HTTP
/// response, even a 404, has a target; whether its *routes* are placeholders is
/// then a guess, and this said it as fact.
///
/// The remaining two conjuncts still matter as necessary conditions: an operator
/// who set `PROEF_BASE_URL`, or edited `[url] base`, did name a target and must
/// not be second-guessed.
///
/// The exit code is deliberately untouched. Whether an unreachable target is a
/// user fault or a system one is a taxonomy question (ADR-0009) decided in the
/// engine, and the reader's actual problem here is vocabulary, not exit code.
fn is_unconfigured_target(
    config_vars: &BTreeMap<String, String>,
    base_url_overridden: bool,
    summary: &runner::RunSummary,
) -> bool {
    let nothing_reachable = summary.passed == 0
        && !summary.outcomes.is_empty()
        && summary
            .outcomes
            .iter()
            .all(|o| matches!(o.fault, Some(runner::Fault::System(_))));

    nothing_reachable
        && !base_url_overridden
        && config_vars
            .get("url:base")
            .is_some_and(|base| base == crate::init::SCAFFOLD_BASE)
}

/// Are the suite's macro packs still byte-for-byte what `proef init` wrote?
///
/// The scaffold has two halves to fill in — the target and the routes — and a
/// reader can have done either one. `init` says so once, parenthetically, two
/// commands before the failure. Someone who follows that instruction and points
/// `[url] base` at their API then hits the *other* half: the placeholder routes
/// 404, and the target-side note above deliberately cannot fire, because they
/// did configure a target.
///
/// Decided from the file, never from what the server answered. A 404 proves a
/// route is missing, not that it is a placeholder — inferring the second from
/// the first is the class of claim #28 removed.
fn scaffold_routes_untouched(suite: &Path) -> bool {
    let pack = suite.join("packs").join("api.yaml");
    let Ok(text) = std::fs::read_to_string(&pack) else {
        return false;
    };
    // `init` installs the editor modeline as the pack's first line, so the file
    // is never byte-identical to the template on its own.
    let body = text
        .strip_prefix("# yaml-language-server:")
        .and_then(|rest| rest.split_once('\n'))
        .map_or(text.as_str(), |(_, rest)| rest);
    body == crate::init::PACK
}

/// One note, two mutually exclusive halves: nothing was reachable, or the
/// routes were never filled in. Never both — a reader with one unfinished half
/// should be told about that half, not handed a list.
fn note_scaffold_state(
    config_vars: &BTreeMap<String, String>,
    summary: &runner::RunSummary,
    suite: &Path,
) {
    let overridden = matches!(crate::envvar::read("PROEF_BASE_URL"), Ok(Some(_)));
    if is_unconfigured_target(config_vars, overridden, summary) {
        crate::render::errln!(
            "note: nothing answered at the default target, and `[url] base` is still \
             the starter value.\n      \
             point it at your API in proef.toml (or export PROEF_BASE_URL). If this \
             is a fresh `proef init` scaffold, its routes are placeholders too."
        );
    } else if scaffold_routes_untouched(suite) {
        crate::render::errln!(
            "note: the routes in {} are still the `proef init` placeholders — \
             `/health` and `/search` are examples, not your API.\n      \
             edit that pack to name your API's real routes.",
            suite.join("packs").display()
        );
    }
}

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
    max_fail: Option<u32>,
    shard: Option<(u32, u32)>,
    shuffle: bool,
    metadata: &std::collections::BTreeMap<String, String>,
    console_mode: proef_core::report::ConsoleMode,
    config: &ProjectConfig,
    external_cancel: Option<CancellationToken>,
) -> ExitCode {
    // Resolve the active environment once. All three calls consult `env_profile`,
    // so any of them surfaces an unknown `--env` (user error); the match below
    // reports the first such error.
    // Parsed before anything runs: a malformed expression must stop the run, not
    // leave every scenario it should have isolated quietly sharing the pool.
    let exclusive_tags = match crate::commands::exclusive_tags(config) {
        Ok(expr) => expr,
        Err(code) => return code,
    };
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

    // Every project file this run reads or writes, resolved once against the
    // config rather than against the working directory (`ProjectConfig::resolve`).
    let state_file = config.state_file();

    // Phase 1: full validation pass (fail fast on static errors, discover
    // secrets, produce the run id).
    // Read once for the whole invocation: this same corpus serves the suite's
    // validation pass, both phase validations and both phase runs (ADR-0018).
    let fragments = match crate::commands::corpus(config) {
        Ok(fragments) => fragments,
        Err(code) => return code,
    };
    let mut front = match front::run(
        path,
        ResolveMode::DryRun,
        run_id,
        Arc::clone(&config_vars),
        &fragments,
        &state_file,
        &crate::commands::naming(config),
    ) {
        Ok(front) => front,
        Err(err) => return crate::commands::report_front_error(&err),
    };
    render::print_all(&front.warnings);
    front::warn_if_exclusive_matches_nothing(
        &front,
        exclusive_tags.as_ref(),
        config.run.exclusive_tags.as_deref(),
    );

    // Teardown is validated here, beside the suite and before any run directory
    // or record exists. Otherwise a typo'd `[run] teardown` costs a full suite
    // execution — real requests, artifacts, a run record — before failing,
    // while the identical mistake in `[run] setup` failed in milliseconds. Same
    // mistake, same class, so it costs the same. Suite first, so a suite error
    // is not masked by a phase error.
    if let Some(teardown) = config.teardown()
        && let Err(code) = load_phase_feature(
            "teardown",
            &teardown,
            None,
            &config_vars,
            &fragments,
            config,
        )
    {
        return code;
    }

    let secret_names: BTreeSet<String> = front
        .features
        .iter()
        .flat_map(|f| f.scenarios.iter())
        // The *values* are the secret names to look up; the keys are the hurl
        // variables a binding may have renamed them to (ADR-0018).
        .flat_map(|s| s.lowered.secrets.values().cloned())
        .collect();
    let secrets = match crate::secretstore::resolve_all(&config.secrets_file(), &secret_names) {
        Ok(secrets) => Arc::new(secrets),
        Err(missing) => {
            crate::render::errln!("error: missing secret value(s): {}", missing.join(", "));
            return ExitCode::UserError;
        }
    };

    // Run directory. Rotation happens before the new dir exists so the
    // in-flight run can never be a rotation candidate.
    let runs_root = config.runs_dir();

    // `--rerun`: read the prior run's failures BEFORE this run's dir exists, so
    // `latest_run` sees the previous run, not this one. No prior record is a
    // user error; a clean prior run has nothing to rerun (exit 0).
    let mut rerun_base: Option<(String, Vec<Event>)> = None;
    let rerun_set = if rerun {
        let Some(dir) = crate::record::resolve_dir(&runs_root, None) else {
            crate::render::errln!("error: --rerun found no prior run record to rerun from");
            return ExitCode::UserError;
        };
        // The base is what the overlay needs later: its id names this
        // run's head (`rerun_of`), and its events carry the outcomes the
        // JUnit merge reconstructs for scenarios not re-run (E2's rerun
        // half — the one JUnit at the end covers the whole suite).
        rerun_base = crate::record::read_events(&dir).ok().map(|events| {
            (
                dir.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                events,
            )
        });
        match crate::record::rerun_candidates(&dir) {
            Ok(candidates) if candidates.scenarios.is_empty() => {
                crate::render::outln!("nothing to rerun — the last run has no failed scenarios");
                return ExitCode::Success;
            }
            Ok(candidates) => {
                // Continuing a partial run is different work than retrying
                // failures, and the developer should know which one this is —
                // a silent green over a mostly-unexecuted suite was the bug.
                if candidates.never_ran > 0 {
                    crate::render::outln!(
                        "note: the last run was cancelled before {} scenario(s) ran — \
                         rerunning them along with the failures",
                        candidates.never_ran
                    );
                }
                Some(candidates.scenarios)
            }
            Err(err) => {
                crate::render::errln!("error: {err}");
                return ExitCode::UserError;
            }
        }
    } else {
        None
    };

    rotate_runs(&runs_root, front.run_id.as_ref(), config.keep_runs());
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
            // run.log mirrors the console verbatim, dots included: the
            // record (events.jsonl) is the full truth (ADR-0008), and a
            // second full-mode reporter for a derived view is machinery
            // the contract does not need.
            console_mode,
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

    // `--max-fail`: wrapped after the timing stamp so every emitter flows
    // through it. Semantics live on `trip_on_max_fail` itself.
    let sink = trip_on_max_fail(sink, max_fail, cancel.clone());

    // `[run] setup`, the suite, and `[run] teardown` each call `runner::run`,
    // which brackets its own work with `RunStarted`/`RunFinished`. A record
    // must carry exactly one pair overall (ADR-0008), so the phases run
    // against a wrapper that drops that pair, and a `RunRecord` guard owns
    // the single pair for the whole run — opened below and closed by an
    // explicit `drop` after teardown, with `Drop` as the backstop for every
    // early return in between (see `RunRecord`).
    let pool_sink = suppress_run_head_tail(sink.clone());

    // The machine-body funnel (R17-2.3/2.4 and follow-ups): from here the run
    // exists — id, run dir, redactions — and every way out of it, the pool as
    // much as an empty shard, an empty selection, a setup abort or a store
    // failure, returns THROUGH this closure to the one `emit_machine_body`
    // below. Four audit rounds each found another terminating path emitting
    // prose or zero stdout bytes under `--output json`; a single exit is what
    // makes a fifth impossible to forget. Paths that end before the pool
    // report zeroed totals (ADR-0014) with their exit code.
    let run = || -> (ExitCode, runner::RunSummary, Vec<(String, String)>) {
        let mut record = RunRecord::open(
            &sink,
            &cancel,
            &front.run_id,
            active_env,
            metadata,
            shuffle,
            rerun_base.as_ref().map(|(id, _)| id.as_str()),
        );

        // Shared global store (scenario merge-back through the lock, §12).
        let store = match GlobalStore::load(&state_file) {
            Ok(store) => Arc::new(Mutex::new(store)),
            Err(err) => {
                crate::render::errln!("error: {err}");
                return (ExitCode::SystemError, empty_run_summary(), Vec::new());
            }
        };

        let engines = Arc::new(registry::engines());

        // Suite-level setup/teardown (ADR-0014): a feature run once before / after
        // the pool at the CLI edge, sharing the store so its `saveAs: global`
        // promotions reach every scenario. The core runner stays tag-agnostic.
        let setup_path = config.setup();
        let teardown_path = config.teardown();

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
                &phase_sink("setup", sink.clone()),
                &cancel,
                &artifacts_dir,
                &fragments,
                config,
            ) {
                // A phase that failed to even load (missing file, bad pack,
                // missing secret) is a terminating path like any other.
                Err(code) => return (code, empty_run_summary(), Vec::new()),
                Ok(summary) => {
                    // `RunRecord`'s totals are the main-suite verdict only (ADR-0014):
                    // setup's own outcome still drives the exit code below, and its
                    // scenarios are still visible as events in the record, but it is
                    // never folded into `passed`/`failed`/`skipped` — those must match
                    // what JUnit/`--output json`/TAP/the SLA gate/the exit code report.
                    if let Some(code) = phase_failed(&summary, ExitCode::UserError) {
                        crate::render::errln!(
                            "error: setup failed — aborting before the suite runs"
                        );
                        // R12-3: the abort must still reach CI's readers. The
                        // reports carry the setup scenario itself — honest, and
                        // better than the missing file a JUnit-gated job used to
                        // see. A JUnit write failure does not re-classify the
                        // exit: the setup fault is the more specific verdict.
                        write_ci_reports(
                            &Verdict {
                                summary: &summary,
                                teardown: None,
                                non_gating: &[],
                                carried: &[],
                            },
                            &front.run_id,
                            junit,
                            &run_dir,
                            &redactions,
                            machine_stdout,
                        );
                        return (code, empty_run_summary(), Vec::new());
                    }
                    // A setup that only skipped (interrupted, or watchdog-abandoned)
                    // carries no fault, so `phase_failed` waves it through — and the
                    // suite would then run against state setup never created. Abort
                    // for the same reason a failure aborts. This is also what keeps
                    // teardown gated on setup-success (ADR-0014): the early return is
                    // the gate, and teardown must not dismantle what was never built.
                    if !summary.outcomes.is_empty()
                        && summary
                            .outcomes
                            .iter()
                            .all(|o| o.status == proef_core::step::Status::Skipped)
                    {
                        crate::render::errln!(
                            "error: setup ran no scenario to completion — aborting before the suite runs"
                        );
                        write_ci_reports(
                            &Verdict {
                                summary: &summary,
                                teardown: None,
                                non_gating: &[],
                                carried: &[],
                            },
                            &front.run_id,
                            junit,
                            &run_dir,
                            &redactions,
                            machine_stdout,
                        );
                        return (ExitCode::SystemError, empty_run_summary(), Vec::new());
                    }
                }
            }
        }

        // A setup/teardown feature must not also run as an ordinary suite scenario.
        exclude_phase_features(&mut front, setup_path.as_ref(), teardown_path.as_ref());

        let specs = build_specs(
            &front,
            tags,
            exclusive_tags.as_ref(),
            scenario_filter,
            scenario_file_filter,
            rerun_set.as_deref(),
            &artifacts_dir,
        );
        // `--shard I/N` — applied AFTER every other filter, so a shard is always
        // "shard of what you selected" (the pinned filter→shard order): the same
        // expression on every matrix job partitions one agreed-on set.
        let before_shard = specs.len();
        let mut specs = specs;
        if let Some((index, count)) = shard {
            specs.retain(|spec| shard_bucket(&spec.file, &spec.name, count) == index - 1);
        }
        // `--shuffle` — after the shard filter, so each matrix job re-deals
        // exactly what it runs (bucketing hashes identity and is
        // order-independent, so membership is untouched). The permutation is
        // seeded by the run id: one determinism knob for order and fakes
        // alike (IMPROVEMENT-PLAN #14's rule — no parallel seed), so
        // `--shuffle --run-id <id from the failing run>` reproduces an
        // order-dependent failure exactly. Under `--watch`, each unpinned
        // rerun gets a fresh id and therefore a fresh order — deliberate:
        // the loop shakes order deps loose, each iteration reproducible.
        if shuffle {
            shuffle_specs(&mut specs, &front.run_id);
        }
        let selected = specs.len();
        if selected == 0 {
            // An empty *shard* of a non-empty selection is a small suite spread
            // over a big matrix — a fact, not a mistake, and a matrix job must not
            // fail on it. An empty *selection* stays the loud refusal below: a
            // typo'd --tags passing CI with zero tests run is the silent-green
            // failure mode (ADR-0009: user error), and sharding must not blunt it.
            if let Some((index, count)) = shard
                && before_shard > 0
            {
                let note = format!(
                    "shard {index}/{count} selected 0 of {before_shard} scenario(s) — nothing to run in this shard"
                );
                if machine_stdout {
                    crate::render::errln!("{note}");
                } else {
                    crate::render::outln!("{note}");
                }
                return (ExitCode::Success, empty_run_summary(), Vec::new());
            }
            // Loud exit 2 by design; the refusal's own return value is the one
            // source of the code, so the body can never disagree with the exit.
            return (
                front::no_scenarios_matched(),
                empty_run_summary(),
                Vec::new(),
            );
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
        let summary = runner::run(specs, &engines, &store, &run_config, &pool_sink, &cancel);
        record.add(&summary);

        // Suite-level teardown: runs after the pool (setup succeeded, or there was
        // none). Its failure is a distinct non-zero signal (exit 3), never a
        // silently-green suite — the suite's own verdict still stands.
        let mut teardown_exit = ExitCode::Success;
        // A failed teardown's outcomes reach JUnit as their own suite (R17-2.5) —
        // symmetric with #78's rule for setup: a phase appears in the reports
        // when it fails. A green teardown stays out, exactly as a green setup
        // does: the reports describe the suite, plus whatever phase broke.
        let mut teardown_summary: Option<runner::RunSummary> = None;
        if let Some(teardown) = &teardown_path {
            // Cleanup outlives the interrupt. Teardown runs on its OWN token, never
            // the run's and never `cancel.child_token()` — a child cancels with its
            // parent, which is exactly the behaviour being fixed. On Ctrl-C the pool
            // stops and cleanup still happens, so an interrupted run does not strand
            // whatever setup created (ADR-0014: cleanup is reliable).
            //
            // ADR-0007's responsive interrupt is preserved by the escape hatch that
            // already exists: a second Ctrl-C hard-exits (130) out of teardown too.
            // A hung teardown is bounded by the same batch budgets and watchdog as
            // any other phase, so this needs no timeout of its own.
            let teardown_cancel = CancellationToken::new();
            if cancel.is_cancelled() {
                crate::render::errln!(
                    "cleaning up — running teardown after the interrupt (Ctrl-C again to skip)"
                );
            }
            match run_phase(
                "teardown",
                teardown,
                &front.run_id,
                &config_vars,
                &http_defaults,
                &store,
                &engines,
                &phase_sink("teardown", sink.clone()),
                &teardown_cancel,
                &artifacts_dir,
                &fragments,
                config,
            ) {
                // The phase's own exit code, not a blanket 3: a teardown path that
                // does not exist is a user error like any other, and flattening it
                // told the operator "system fault" for their own typo.
                Err(code) => teardown_exit = code,
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
                        teardown_summary = Some(summary);
                    } else if !summary.outcomes.is_empty()
                        && summary
                            .outcomes
                            .iter()
                            .all(|o| o.status == proef_core::step::Status::Skipped)
                    {
                        // A phase that only skipped carries no fault and no failure,
                        // so `phase_failed` returns `None` and the whole thing passes
                        // in silence — the shape that let cancelled cleanup vanish.
                        // Whatever the cause (an interrupt that reached this token, a
                        // watchdog abandonment), cleanup did not run and saying so is
                        // the point of having a teardown phase at all.
                        crate::render::errln!(
                            "error: teardown ran no scenario to completion — cleanup did not run"
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
            && let Err(err) = guard.save(&state_file)
        {
            crate::render::errln!("warning: cannot persist global state: {err}");
        }

        // `@quarantine` scenarios run and report, but their test-failures do not
        // gate the run (a System/User fault still does — quarantine is for flaky
        // tests, not broken input or infra). Read from the outcomes' own tags —
        // the one derivation the record also carries — not re-derived from the
        // front (R17 deep-audit's one-owner rule, closed by tags-in-outcomes).
        let non_gating: Vec<(String, String)> = summary
            .outcomes
            .iter()
            .filter(|outcome| {
                outcome
                    .tags
                    .iter()
                    .any(|tag| tag == crate::front::reserved::QUARANTINE)
            })
            .map(|outcome| (outcome.file.to_string(), outcome.name.to_string()))
            .collect();

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
                        "  ✗ {}:{} — {}{}",
                        step.step.file,
                        step.step.line,
                        redactions.apply(detail),
                        crate::render::via(step.fragment.as_deref())
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
                    crate::render::errln!(
                        "  reproduce: hurl --test {}{vars_arg}",
                        artifact.display()
                    );
                }
            }
        }

        // Carried outcomes: the rerun base's suite scenarios this run did
        // not re-run, reconstructed from its record so the JUnit is whole
        // (E2's rerun half). Exit code and totals stay this run's own.
        let carried = rerun_base
            .as_ref()
            .map(|(_, events)| {
                let ran: std::collections::BTreeSet<(String, String)> = summary
                    .outcomes
                    .iter()
                    .map(|o| (o.file.to_string(), o.name.to_string()))
                    .collect();
                crate::record::carried_outcomes(events, &ran)
            })
            .unwrap_or_default();
        if !carried.is_empty() {
            crate::render::errln!(
                "note: {} scenario(s) carried from run {} into the reports",
                carried.len(),
                rerun_base.as_ref().map_or("?", |(id, _)| id.as_str())
            );
        }
        // CI reports (US-8): JUnit XML + GitHub job summary.
        let junit_failed = write_ci_reports(
            &Verdict {
                summary: &summary,
                teardown: teardown_summary.as_ref(),
                non_gating: &non_gating,
                carried: &carried,
            },
            &front.run_id,
            junit,
            &run_dir,
            &redactions,
            machine_stdout,
        );

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

        // A run that never reached its target leaves a first-time reader with a bare
        // `system error` and no way to know the tool is working as intended. The
        // run's own outcomes decide, not the config alone — see the predicate.
        if exit != ExitCode::Success {
            note_scaffold_state(&config_vars, &summary, path);
        }

        // Fold the JUnit-write failure in BEFORE anything serializes the verdict.
        // It used to be applied as a `return` after the machine-readable body had
        // already been printed, so `--output json` reported an `exit_code` the
        // process then exited past — a body that disagrees with its own program is
        // worse than no body, because a consumer has no way to notice.
        let exit = if junit_failed {
            ExitCode::SystemError
        } else {
            exit
        };

        (exit, summary, non_gating)
    };

    let (exit, summary, non_gating) = run();
    emit_machine_body(
        output,
        &RunHead {
            run_id: &front.run_id,
            env: active_env,
            metadata,
        },
        &summary,
        &non_gating,
        &redactions,
        &run_dir,
        exit,
    );
    exit
}
/// The suite verdict of a run whose pool never executed: zeros, not
/// cancelled. What ADR-0014 says the totals are on a path that ended before
/// the first scenario — the exit code carries the actual verdict.
fn empty_run_summary() -> runner::RunSummary {
    runner::RunSummary {
        outcomes: Vec::new(),
        passed: 0,
        failed: 0,
        skipped: 0,
        cancelled: false,
    }
}

/// The machine-readable stdout body — emitted from exactly one place, the
/// funnel at the end of `execute`, which every terminating path returns
/// through. R17-2.3/2.4 and follow-ups: this used to be called site-by-site,
/// and four audit rounds each found another path printing prose (which broke
/// `jq` mid-pipeline) or nothing at all (a `--output json` consumer read zero
/// bytes on a failed setup); the single call site is what ended that class.
/// Totals are the suite-only verdict (ADR-0014) — a path that never reached
/// the pool reports zeros with its exit code, and the record path is always
/// real: `RunRecord` opens inside the funnel and closes structurally on drop.
fn emit_machine_body(
    output: Option<OutputFormat>,
    head: &RunHead<'_>,
    summary: &runner::RunSummary,
    non_gating: &[(String, String)],
    redactions: &proef_core::report::Redactions,
    run_dir: &Path,
    exit: ExitCode,
) {
    match output {
        Some(OutputFormat::Json) => {
            // `env`/`metadata` are additive keys (a jq pipeline keyed on the
            // original five is unaffected); values ride pre-merged and are
            // masked like every sink — a secret pasted into --meta must not
            // round-trip through the body unredacted.
            let metadata: std::collections::BTreeMap<String, String> = head
                .metadata
                .iter()
                .map(|(key, value)| (redactions.apply(key), redactions.apply(value)))
                .collect();
            let json = serde_json::json!({
                "run_id": head.run_id,
                "passed": summary.passed,
                "failed": summary.failed,
                "skipped": summary.skipped,
                "exit_code": exit.code(),
                "events": run_dir.join("events.jsonl").display().to_string(),
                "env": head.env,
                "metadata": metadata,
            });
            crate::render::outln!("{json}");
        }
        // TAP v13 from the run's own scenario outcomes (one test point each),
        // quarantined scenarios mapped to `# TODO`; redacted like every sink.
        Some(OutputFormat::Tap) => {
            let tap = crate::tap::render(&summary.outcomes, non_gating, redactions);
            crate::render::outln!("{}", tap.trim_end());
        }
        None => {}
    }
}

/// Wrap `inner` so the run cancels once `max_fail` suite scenarios have
/// failed (`--max-fail N`); `None` passes every event straight through.
///
/// A sink wrapper rather than runner surface because the event spine already
/// carries exactly what the decision needs: `ScenarioFinished` says which
/// scenario, with what status, in which phase — emitted from the dispatcher
/// thread, so the cancel lands before the next scenario is scheduled and
/// `--jobs 1 --max-fail 1` stops after precisely one failure. Counting only
/// `phase: None` keeps `[run] setup`/`teardown` out of the threshold: a setup
/// failure aborts the run by itself (ADR-0014), and by the time teardown runs
/// the pool is already drained, so cancelling there would be a no-op that
/// still misread cleanup trouble as suite failures.
fn trip_on_max_fail(
    inner: EventSink,
    max_fail: Option<u32>,
    cancel: CancellationToken,
) -> EventSink {
    let Some(threshold) = max_fail else {
        return inner;
    };
    // A plain atomic moved into the closure: `EventSink::new` already wraps it
    // in an `Arc<dyn Fn>`, so an inner Arc implied sharing that does not
    // exist. `Relaxed` suffices — the counter synchronizes nothing; the cancel
    // token carries its own ordering.
    let failed = std::sync::atomic::AtomicU32::new(0);
    EventSink::new(move |event| {
        if let Event::ScenarioFinished {
            status: proef_core::step::Status::Failed,
            phase: None,
            ..
        } = event
        {
            // `fetch_add` returns the previous count, so exactly one emitter
            // crosses the threshold and prints — parallel failures cannot
            // trip it twice or double-print under `--jobs N`.
            if failed.fetch_add(1, Ordering::Relaxed) + 1 == threshold {
                crate::render::errln!(
                    "stopping: {threshold} scenario failure(s) reached (--max-fail) — \
                     cancelling after current batches"
                );
                cancel.cancel();
            }
        }
        inner.emit(event);
    })
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
            Event::ScenarioStarted {
                scenario,
                file,
                phase,
                exclusive,
                ..
            } => inner.emit(&Event::ScenarioStarted {
                scenario: scenario.clone(),
                file: file.clone(),
                timestamp_ms: Some(now_ms()),
                worker: Some(acquire_slot(scenario, file)),
                phase: phase.clone(),
                exclusive: *exclusive,
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
                phase,
                reason,
                tags,
                ..
            } => {
                release_slot(scenario, file);
                inner.emit(&Event::ScenarioFinished {
                    scenario: scenario.clone(),
                    file: file.clone(),
                    status: *status,
                    timestamp_ms: Some(now_ms()),
                    worker: None,
                    phase: phase.clone(),
                    reason: reason.clone(),
                    tags: tags.clone(),
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
/// Drop a nested runner's own head/tail: `RunRecord` owns the record's single
/// `RunStarted`/`RunFinished` pair (ADR-0008), and setup, the pool and teardown
/// each run their own `runner::run`.
fn suppress_run_head_tail(inner: EventSink) -> EventSink {
    EventSink::new(move |event| match event {
        Event::RunStarted { .. } | Event::RunFinished { .. } => {}
        other => inner.emit(other),
    })
}

/// The sink a `[run] setup`/`teardown` phase emits through: it drops the
/// phase's own head/tail (the record owns exactly one pair, ADR-0008) and
/// **stamps `phase` onto the phase's scenario events**.
///
/// Stamping happens here because this is the only place that knows a scenario
/// is part of a phase. Without it the record cannot tell a teardown scenario
/// from a suite one except by feature path, so every consumer re-derived phase
/// membership from `proef.toml` — and `explain`, `--rerun` and `diff` each got
/// it wrong in a different way. One signal, written once, read everywhere.
fn phase_sink(label: &str, inner: EventSink) -> EventSink {
    let label: Arc<str> = Arc::from(label);
    EventSink::new(move |event| match event {
        Event::RunStarted { .. } | Event::RunFinished { .. } => {}
        Event::ScenarioStarted {
            scenario,
            file,
            timestamp_ms,
            worker,
            exclusive,
            ..
        } => inner.emit(&Event::ScenarioStarted {
            scenario: scenario.clone(),
            file: file.clone(),
            timestamp_ms: *timestamp_ms,
            worker: *worker,
            phase: Some(Arc::clone(&label)),
            exclusive: *exclusive,
        }),
        Event::ScenarioFinished {
            scenario,
            file,
            status,
            timestamp_ms,
            worker,
            reason,
            tags,
            ..
        } => inner.emit(&Event::ScenarioFinished {
            scenario: scenario.clone(),
            file: file.clone(),
            status: *status,
            timestamp_ms: *timestamp_ms,
            worker: *worker,
            phase: Some(Arc::clone(&label)),
            reason: reason.clone(),
            tags: tags.clone(),
        }),
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
    fn open(
        sink: &'a EventSink,
        cancel: &'a CancellationToken,
        run_id: &Arc<str>,
        env: Option<&str>,
        metadata: &std::collections::BTreeMap<String, String>,
        shuffled: bool,
        rerun_of: Option<&str>,
    ) -> Self {
        sink.emit(&Event::RunStarted {
            schema: proef_core::event::EVENT_SCHEMA_VERSION,
            run_id: Arc::clone(run_id),
            env: env.map(Arc::from),
            metadata: metadata.clone(),
            shuffled,
            rerun_of: rerun_of.map(Arc::from),
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

/// Validate one phase feature through the front end — the single definition of
/// "is this `[run] setup`/`teardown` usable", shared by `--dry-run`, the
/// pre-flight in [`execute`], and [`run_phase`] itself.
///
/// ADR-0014 says the phase features are "validated like any other feature but
/// never executed" under `--dry-run`. That was true of nothing: `--dry-run` did
/// not look at them at all, and `execute` only discovered a broken teardown
/// after the whole suite had run. One loader, called everywhere, is what makes
/// the sentence true.
pub(crate) fn load_phase_feature(
    label: &str,
    path: &Path,
    run_id: Option<String>,
    config_vars: &Arc<BTreeMap<String, String>>,
    fragments: &proef_core::pack::FragmentCorpus,
    config: &ProjectConfig,
) -> Result<FrontEnd, ExitCode> {
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
    front::run(
        path,
        ResolveMode::DryRun,
        run_id,
        Arc::clone(config_vars),
        fragments,
        &config.state_file(),
        &crate::commands::naming(config),
    )
    .map_err(|err| {
        crate::render::errln!("error: {label} feature failed to validate:");
        crate::commands::report_front_error(&err)
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
    fragments: &proef_core::pack::FragmentCorpus,
    config: &ProjectConfig,
) -> Result<runner::RunSummary, ExitCode> {
    // ADR-0014: `[run] setup`/`teardown` names exactly one feature file. A
    // directory would run every feature under it as the phase AND leave them
    // in the pool (exclude_phase_features matches a single file path), running
    // each scenario twice. Reject it loudly instead of silently double-running.
    let front = load_phase_feature(
        label,
        path,
        Some(run_id.to_string()),
        config_vars,
        fragments,
        config,
    )?;
    render::print_all(&front.warnings);

    let names: BTreeSet<String> = front
        .features
        .iter()
        .flat_map(|feature| feature.scenarios.iter())
        .flat_map(|scenario| scenario.lowered.secrets.values().cloned())
        .collect();
    let secrets = match crate::secretstore::resolve_all(&config.secrets_file(), &names) {
        Ok(secrets) => Arc::new(secrets),
        Err(missing) => {
            crate::render::errln!(
                "error: {label} missing secret value(s): {}",
                missing.join(", ")
            );
            return Err(ExitCode::UserError);
        }
    };

    let specs = build_specs(&front, None, None, None, None, None, artifacts_dir);
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
/// The CI-facing reports for one summary: `JUnit` XML, the GitHub job summary,
/// and PR-gutter annotations. One function so every path that ends a run —
/// the ordinary pool, and a setup abort (R12-3: a failed setup used to return
/// before any of this, and a CI job reading `JUnit` saw no file at all) —
/// emits the same set. Returns whether a requested `JUnit` file could not be
/// written.
/// The run's own identity and provenance, bundled for the machine body:
/// the injected id plus the explicit env/metadata the head records
/// (ADR-0020).
struct RunHead<'a> {
    run_id: &'a str,
    env: Option<&'a str>,
    metadata: &'a std::collections::BTreeMap<String, String>,
}

/// The run's verdict, bundled for the CI sinks: the suite summary, a
/// failed teardown's own summary, and the quarantine (non-gating) list —
/// the three inputs every sink reads together.
struct Verdict<'a> {
    summary: &'a runner::RunSummary,
    teardown: Option<&'a runner::RunSummary>,
    non_gating: &'a [(String, String)],
    /// Outcomes carried over from a `--rerun` base for scenarios NOT
    /// re-run — the `JUnit` covers the whole suite, while the exit code
    /// and totals stay this run's own (ADR-0014).
    carried: &'a [runner::ScenarioOutcome],
}

fn write_ci_reports(
    verdict: &Verdict<'_>,
    run_id: &str,
    junit: Option<&str>,
    run_dir: &Path,
    redactions: &proef_core::report::Redactions,
    machine_stdout: bool,
) -> bool {
    let mut junit_failed = false;
    let junit_path = match junit {
        Some("auto") if std::env::var_os("GITHUB_ACTIONS").is_some() => {
            Some(run_dir.join("report.junit.xml"))
        }
        Some("auto") | None => None,
        Some(path) => Some(PathBuf::from(path)),
    };
    if let Some(junit_path) = junit_path {
        match crate::ci_reports::write_junit(
            verdict.summary,
            verdict.teardown,
            verdict.carried,
            verdict.non_gating,
            run_id,
            &junit_path,
            redactions,
        ) {
            Ok(()) => crate::render::errln!("junit report: {}", junit_path.display()),
            Err(message) => {
                // A CI job gating on this file must not see exit 0.
                crate::render::errln!("error: {message}");
                junit_failed = true;
            }
        }
    }
    crate::ci_reports::write_github_summary(verdict.summary, run_id, redactions);
    // GitHub annotations render each failure in the PR diff gutter. They are
    // stdout workflow commands, so emit only under Actions and only when the
    // human report (not `--output json`) owns stdout.
    if !machine_stdout && std::env::var_os("GITHUB_ACTIONS").is_some() {
        let annotations = crate::ci_reports::github_annotations(verdict.summary, redactions);
        if !annotations.is_empty() {
            crate::render::outln!("{}", annotations.trim_end());
        }
    }
    junit_failed
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

/// Fisher–Yates over the selected specs, seeded from the run id (FNV-1a of
/// its bytes into a `SplitMix64` stream — both hand-rolled like `shard_bucket`
/// below: dependency-free, and stable across proef versions so a recorded
/// run id replays its order forever). Core stays pure: the permutation
/// happens here at the CLI edge, and the runner receives an already-ordered
/// `Vec` exactly as it does without the flag.
fn shuffle_specs(specs: &mut [runner::ScenarioSpec], run_id: &str) {
    let mut seed = 0xcbf2_9ce4_8422_2325_u64;
    for byte in run_id.bytes() {
        seed ^= u64::from(byte);
        seed = seed.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut next = move || {
        seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    for i in (1..specs.len()).rev() {
        let j = usize::try_from(next() % (i as u64 + 1)).unwrap_or(0);
        specs.swap(i, j);
    }
}

/// Which shard a scenario belongs to: a stable hash of its run-wide identity,
/// modulo the shard count.
///
/// **The assignment is a contract.** A CI matrix runs `--shard 1/N..N/N` on
/// separate machines, and the value of hash-mode sharding — measured, and the
/// reason index-slicing was rejected at triage — is that adding one scenario
/// never re-buckets the others. That only holds if this function's output
/// never changes for a given input, across proef versions: FNV-1a is spelled
/// out here rather than borrowed from a std or crate hasher precisely because
/// those make no cross-version stability promise (`DefaultHasher`'s docs say
/// the opposite). The exact values are pinned by a unit test below; changing
/// this function is a breaking change to every matrix that uses sharding.
///
/// The NUL joint makes the identity injective: `("a", "b\0c")` and
/// `("a\0b", "c")` must not collide, and NUL appears in neither a path nor a
/// scenario name.
fn shard_bucket(file: &str, name: &str, count: u32) -> u32 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64; // FNV-1a offset basis
    for byte in file.bytes().chain(std::iter::once(0)).chain(name.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3); // FNV prime
    }
    // Murmur3's fmix64 finalizer (R18). Raw FNV's multiplier is odd, so the
    // accumulator's low bit is the XOR-parity of the input bytes' low bits —
    // and a scenario named after its feature file (the commonest Gherkin
    // convention) duplicates content between the two halves of the identity,
    // whose parity contributions cancel: the whole corpus lands in one shard
    // at N=2. The finalizer avalanches every input bit into every output bit,
    // which is the property `% count` actually needs.
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xff51_afd7_ed55_8ccd);
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    hash ^= hash >> 33;
    u32::try_from(hash % u64::from(count)).unwrap_or(0)
}

/// Build one `ScenarioSpec` per tag-selected scenario. The prepare closure
/// re-lowers with the **live** World (ADR-0005 lower-time globals), emits the
/// artifact, writes it into the run dir, and hands the same bytes to the
/// engine (ADR-0010).
fn build_specs(
    front: &FrontEnd,
    tags: Option<&proef_core::tags::TagExpr>,
    exclusive: Option<&proef_core::tags::TagExpr>,
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
                    secret_bindings: lowered.secrets,
                })
            });
            specs.push(ScenarioSpec {
                file: Arc::clone(&file_arc),
                name: Arc::from(scenario.lowered.name.as_str()),
                line: scenario.lowered.line,
                skip: crate::front::reserved::skip_reason(&scenario.lowered.tags).map(Arc::from),
                tags: scenario.lowered.tags.clone().into(),
                file_root: Some(crate::fsutil::parent_dir(Path::new(
                    feature.file.path.as_str(),
                ))),
                // Selected by the same expression language `--tags` uses, over
                // the same accumulated tags, so "which scenarios are exclusive"
                // is answered exactly as "which scenarios are selected".
                exclusive: exclusive
                    .is_some_and(|expr| front::tag_selected(&scenario.lowered.tags, Some(expr))),
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

/// Keep the newest `keep` run records (uuid-v7 names sort by time). Only
/// directories *named by a run id* are candidates — `runs-dir` may be `.` or
/// otherwise shared with user content, and rotation must never touch anything
/// proef did not create — and the in-flight run never is.
///
/// That safety rule has a cost worth knowing: `--run-id ci` writes a directory
/// this will never delete, so a caller minting a fresh id per build accumulates
/// records without bound whatever `keep` says. Deliberate — guessing at
/// user-named directories is the worse failure — and documented in CONFIG.md.
fn rotate_runs(runs_dir: &Path, current_run: &str, keep: usize) {
    // `record::all_runs` is the one answer to "what is a run record here" —
    // sorted, uuid-named directories only. Rotation adds a single further
    // exclusion (the in-flight run), not a second enumeration rule.
    let dirs: Vec<PathBuf> = crate::record::all_runs(runs_dir)
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name != current_run)
        })
        .collect();
    if dirs.len() > keep {
        let excess = dirs.len() - keep;
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

    use std::collections::BTreeMap;

    use super::{RunRecord, rotate_runs, runner};
    use proef_core::cancel::CancellationToken;
    use proef_core::event::{Event, EventSink};

    /// The permutation is a contract like the shard assignment below: seeded
    /// only by the run id, stable across proef versions, so a recorded id
    /// replays its order forever. Expected sequence computed independently.
    #[test]
    fn shuffle_is_seeded_by_the_run_id_alone() {
        use proef_core::runner::ScenarioSpec;
        let spec = |name: &str| ScenarioSpec {
            file: "f.feature".into(),
            name: name.into(),
            line: 1,
            file_root: None,
            exclusive: false,
            skip: None,
            tags: std::sync::Arc::from(Vec::new()),
            prepare: Box::new(|_| unreachable!("never prepared in this test")),
        };
        let mut specs: Vec<ScenarioSpec> = ["a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|n| spec(n))
            .collect();
        super::shuffle_specs(&mut specs, "pinned-seed");
        let order: Vec<&str> = specs.iter().map(|s| s.name.as_ref()).collect();
        assert_eq!(order, ["c", "f", "a", "b", "d", "e"]);
    }

    /// The shard assignment is a contract: these literals were computed once
    /// and frozen. If this test fails, the hash changed — which silently
    /// re-buckets every scenario across every CI matrix that shards. That is
    /// a breaking change to publish, never an implementation detail to slip.
    #[test]
    fn shard_assignment_is_frozen() {
        use super::shard_bucket;
        let file = "suite/case.feature";
        for (name, at2, at3) in [
            ("s1", 1, 1),
            ("s2", 0, 0),
            ("s3", 1, 2),
            ("s4", 0, 0),
            ("s5", 1, 0),
            ("s6", 0, 0),
        ] {
            assert_eq!(shard_bucket(file, name, 2), at2, "{name} at N=2");
            assert_eq!(shard_bucket(file, name, 3), at3, "{name} at N=3");
        }
        // The joint is what keeps `("ab","c")` and `("a","bc")` distinct —
        // without it both would hash `abc`. (Injectivity rests on NUL
        // appearing in neither component, which paths and scenario names
        // guarantee; a NUL-bearing input could still collide, and the first
        // draft of this test proved exactly that by violating the
        // precondition and demanding injectivity anyway.)
        assert_ne!(
            shard_bucket("ab", "c", 1_000_000),
            shard_bucket("a", "bc", 1_000_000),
            "the (file, name) joint must keep the pair boundaries distinct"
        );
    }

    /// R17-2.1, corrected by round 18. The first version of this test held
    /// the file path constant in all three corpora — precisely the condition
    /// under which raw FNV behaves — and its doc claimed collapse needed a
    /// degenerate corpus. Round 18 falsified that with the most common
    /// convention there is, the scenario named after its feature file:
    /// FNV's multiplier is odd, so the hash's low bit is the XOR-parity of
    /// the input bytes' low bits, and content mirrored between path and name
    /// contributes twice and cancels — a corpus-constant parity, every
    /// scenario in one shard at N=2 (reproduced: [20,0]; odd buckets empty
    /// at N=4). The `mirrored` corpus below is that shape, kept red against
    /// any future un-mixed hash. Bounds are calibrated to what a well-mixed
    /// hash yields at 20 items: no empty shard at any N (random emptiness
    /// here is ~1e-6 — an empty is proof of structure), and the 3× skew
    /// bound at N=2 only, because at N=3/4 a fair deal of 20 legitimately
    /// produces spreads like [2,7,5,6] that a ratio bound would misread.
    #[test]
    fn natural_corpora_spread_across_shards() {
        use super::shard_bucket;
        let numbered: Vec<(String, String)> = (1..=20)
            .map(|i| {
                (
                    "tests/features/api.feature".into(),
                    format!("scenario {i} probes the endpoint"),
                )
            })
            .collect();
        let outline: Vec<(String, String)> = (1..=20)
            .map(|i| {
                (
                    "tests/features/perm.feature".into(),
                    format!("Same name #{i}"),
                )
            })
            .collect();
        let prose: Vec<(String, String)> = [
            "an order is created",
            "an order is cancelled",
            "a cancelled order stays cancelled",
            "the catalog lists new items",
            "a search finds the record",
            "an empty search says so",
            "the admin resets a password",
            "a token expires mid-session",
            "a refund closes the ledger",
            "pagination survives a deletion",
            "the webhook retries twice",
            "a duplicate is refused",
            "the export contains headers",
            "an import round-trips",
            "the audit trail is ordered",
            "a locked user cannot login",
            "rate limits return 429",
            "the health check passes",
            "a slow endpoint times out",
            "teardown leaves no rows",
        ]
        .iter()
        .map(|n| ("tests/features/orders.feature".into(), (*n).into()))
        .collect();

        let mirrored: Vec<(String, String)> = [
            "checkout_flow",
            "user_signup",
            "password_reset",
            "order_cancel",
            "invoice_export",
            "team_invite",
            "token_refresh",
            "webhook_retry",
            "search_filter",
            "profile_update",
            "cart_merge",
            "refund_flow",
            "audit_export",
            "rate_limit",
            "session_expiry",
            "catalog_sync",
            "address_check",
            "email_verify",
            "plan_upgrade",
            "data_import",
        ]
        .iter()
        .map(|stem| {
            (
                format!("tests/features/{stem}.feature"),
                format!("{} works", stem.replace('_', " ")),
            )
        })
        .collect();

        for (label, corpus) in [
            ("numbered", &numbered),
            ("outline", &outline),
            ("prose", &prose),
            ("mirrored", &mirrored),
        ] {
            for count in [2u32, 3, 4] {
                let mut loads = vec![0usize; count as usize];
                for (file, name) in corpus {
                    loads[shard_bucket(file, name, count) as usize] += 1;
                }
                let (min, max) = (
                    *loads.iter().min().unwrap_or(&0),
                    *loads.iter().max().unwrap_or(&0),
                );
                assert!(
                    min > 0,
                    "{label} at N={count}: an empty shard buys zero wall-clock — {loads:?}"
                );
                // The skew bound holds only at N=2: with 20 items over 3-4
                // buckets a fully mixed hash legitimately deals [2,7,5,6],
                // which a ratio bound would reject — randomness itself would
                // fail it. Emptiness is the property; skew is a smell.
                if count == 2 {
                    assert!(
                        max <= 3 * min,
                        "{label} at N={count}: skew defeats sharding's purpose — {loads:?}"
                    );
                }
            }
        }
    }

    /// Rotation honours the configured budget, and still refuses to delete
    /// anything it did not name.
    ///
    /// The two halves are asserted together because they trade against each
    /// other: a budget tight enough to be useful on a laptop is exactly when a
    /// rotation that guessed at directory names would start eating user
    /// content, and `--run-id`-named records sit outside the budget for that
    /// reason — which is a cost, not an oversight, so it is pinned here rather
    /// than left to be rediscovered.
    #[test]
    fn rotation_keeps_the_budget_and_only_ever_deletes_what_it_named() {
        let tmp = tempfile::tempdir().unwrap();
        let runs = tmp.path();
        // uuid-v7 names sort by time, so these are oldest-first by construction.
        let ids: Vec<String> = (0..5)
            .map(|n| format!("0198f3c1-0000-7000-8000-00000000000{n}"))
            .collect();
        for id in &ids {
            std::fs::create_dir_all(runs.join(id)).unwrap();
        }
        for foreign in ["ci", "nightly-42", "notes"] {
            std::fs::create_dir_all(runs.join(foreign)).unwrap();
        }

        // Budget of 2, plus the in-flight run, over 5 existing records.
        rotate_runs(runs, &ids[4], 2);

        let left: Vec<String> = std::fs::read_dir(runs)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !left.contains(&ids[0]),
            "oldest record must be rotated away"
        );
        assert!(
            !left.contains(&ids[1]),
            "second-oldest must be rotated away"
        );
        for kept in [&ids[2], &ids[3], &ids[4]] {
            assert!(left.contains(kept), "{kept} must survive: {left:?}");
        }
        for foreign in ["ci", "nightly-42", "notes"] {
            assert!(
                left.contains(&foreign.to_owned()),
                "`{foreign}` is not a generated run id — rotation must not touch it: {left:?}"
            );
        }
    }

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
            let _record = RunRecord::open(
                &sink,
                &cancel,
                &run_id,
                None,
                &std::collections::BTreeMap::new(),
                false,
                None,
            );
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

    /// The note must fire on the untouched scaffold and stay silent the moment
    /// the operator has configured anything — an override means they *did*
    /// name a target, so their failure is about their API, not the scaffold.
    #[test]
    fn the_unconfigured_target_note_needs_an_unreachable_run() {
        use proef_core::step::Status;

        let vars = |base: Option<&str>| {
            base.map(|b| BTreeMap::from([("url:base".to_owned(), b.to_owned())]))
                .unwrap_or_default()
        };
        let scaffold = vars(Some(crate::init::SCAFFOLD_BASE));

        let outcome = |fault: Option<runner::Fault>| runner::ScenarioOutcome {
            file: "f.feature".into(),
            name: "s".into(),
            line: 1,
            status: Status::Failed,
            reason: None,
            tags: std::sync::Arc::from(Vec::new()),
            steps: Vec::new(),
            fault,
            artifact_slug: None,
        };
        let unreachable = runner::RunSummary {
            outcomes: vec![outcome(Some(runner::Fault::System("connect".to_owned())))],
            passed: 0,
            failed: 1,
            skipped: 0,
            cancelled: false,
        };

        assert!(super::is_unconfigured_target(
            &scaffold,
            false,
            &unreachable
        ));
        // PROEF_BASE_URL set — they named a target.
        assert!(!super::is_unconfigured_target(
            &scaffold,
            true,
            &unreachable
        ));
        // `[url] base` edited — likewise.
        assert!(!super::is_unconfigured_target(
            &vars(Some("https://api.example.com")),
            false,
            &unreachable
        ));

        // The regression this predicate exists to prevent: `GETTING-STARTED`
        // teaches the starter literal verbatim and proef's own proef.toml uses
        // it, so the config alone cannot mean "scaffold". A hand-built suite
        // whose server answered and whose assertion genuinely failed must not
        // be told its target and routes are placeholders.
        let real_assertion_failure = runner::RunSummary {
            outcomes: vec![outcome(None)],
            passed: 0,
            failed: 1,
            skipped: 0,
            cancelled: false,
        };
        assert!(!super::is_unconfigured_target(
            &scaffold,
            false,
            &real_assertion_failure
        ));
    }
}
