//! The run orchestrator (TECH-SPEC §1, §12; ADR-0007): scenario-per-OS-thread,
//! `--jobs`-bounded, cooperative cancellation at batch boundaries, and a
//! heartbeat watchdog that **abandons** over-budget scenario threads (recording
//! a `System` failure and detaching — the process reaps them at exit).
//!
//! Scenarios *prepare* lazily at dispatch time: `${global:key}` reads happen at
//! lower time of the scenario (ADR-0005), so lowering + emission run inside the
//! scenario thread against a snapshot of the shared global store; `saveAs:
//! global` promotions merge back through the store lock when the scenario ends.
//!
//! Clock note: the pipeline stays clock-free (core purity); the orchestrator's
//! monotonic-clock use is confined to budget enforcement and never enters
//! events (durations are engine-measured).

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cancel::CancellationToken;
use crate::diag::Diag;
use crate::engine::{ArtifactRef, EngineFactory, HttpDefaults, ScenarioCtx};
use crate::error::ExitCode;
use crate::event::{EVENT_SCHEMA_VERSION, Event, EventSink};
use crate::step::{Status, StepBatch, StepOutcome};
use crate::world::{GlobalStore, World};

/// Everything a scenario needs at dispatch time. Built by the CLI edge from
/// owned/`Arc`ed data so scenario threads are `'static` (abandonable).
pub struct ScenarioSpec {
    /// Feature file path.
    pub file: Arc<str>,
    /// Scenario name (post-expansion).
    pub name: Arc<str>,
    /// 1-based header line.
    pub line: usize,
    /// Accumulated tags.
    pub tags: Vec<String>,
    /// Root for file bodies referenced by the scenario's entries (the feature
    /// file's directory — hurl `context_dir` confinement, TECH-SPEC §13).
    pub file_root: Option<std::path::PathBuf>,
    /// Lower + emit against the live World snapshot (pure; runs in-thread).
    pub prepare: PrepareFn,
}

/// The dispatch-time preparation: World snapshot in, batches + artifact out.
pub type PrepareFn = Box<dyn FnOnce(&World) -> Result<Prepared, Vec<Diag>> + Send>;

/// A scenario ready to execute.
pub struct Prepared {
    /// Engine batches in authored order.
    pub batches: Vec<StepBatch>,
    /// The emitted artifact (the hurl engine's executed input, ADR-0010).
    pub artifact: Option<ArtifactRef>,
}

/// Run-level configuration.
#[derive(Clone)]
pub struct RunConfig {
    /// Injected run identifier.
    pub run_id: Arc<str>,
    /// Parallel scenario workers.
    pub jobs: usize,
    /// Watchdog budget for a batch whose engine cannot estimate one.
    pub default_batch_budget: Duration,
    /// Secret name → value (engines inject via their redacting mechanisms).
    pub secrets: Arc<BTreeMap<String, String>>,
    /// Batch-level HTTP defaults.
    pub http: HttpDefaults,
}

/// Aggregate result of a run.
#[derive(Debug)]
pub struct RunSummary {
    /// Per-scenario outcomes, in completion order.
    pub outcomes: Vec<ScenarioOutcome>,
    /// Scenarios that passed (warnings allowed).
    pub passed: usize,
    /// Scenarios that failed.
    pub failed: usize,
    /// Scenarios skipped (cancellation).
    pub skipped: usize,
    /// The run was cancelled (Ctrl-C / token) — some scenarios did not run.
    pub cancelled: bool,
}

impl RunSummary {
    /// The run's exit code: system faults dominate, then user faults, then
    /// test failures (ADR-0009). A cancelled run is never `Success` — the
    /// suite did not pass; it was interrupted (folds in as a test failure).
    pub fn exit_code(&self) -> ExitCode {
        let mut worst = if self.cancelled {
            ExitCode::TestFailure
        } else {
            ExitCode::Success
        };
        for outcome in &self.outcomes {
            let code = match (&outcome.fault, outcome.status) {
                (Some(Fault::System(_)), _) => ExitCode::SystemError,
                (Some(Fault::User(_)), _) => ExitCode::UserError,
                (None, Status::Failed) => ExitCode::TestFailure,
                _ => ExitCode::Success,
            };
            worst = pick_worse(worst, code);
        }
        worst
    }
}

/// Prefer system errors over user errors over test failures.
fn pick_worse(a: ExitCode, b: ExitCode) -> ExitCode {
    let rank = |c: ExitCode| match c {
        ExitCode::SystemError => 3,
        ExitCode::UserError => 2,
        ExitCode::TestFailure => 1,
        ExitCode::Success => 0,
    };
    if rank(b) > rank(a) { b } else { a }
}

/// One scenario's outcome.
#[derive(Debug)]
pub struct ScenarioOutcome {
    /// Feature file path.
    pub file: Arc<str>,
    /// Scenario name.
    pub name: Arc<str>,
    /// 1-based header line.
    pub line: usize,
    /// Aggregate status.
    pub status: Status,
    /// Step outcomes, in execution order.
    pub steps: Vec<StepOutcome>,
    /// Non-test fault, when one occurred.
    pub fault: Option<Fault>,
}

/// A non-test fault attributed per ADR-0009.
#[derive(Debug)]
pub enum Fault {
    /// The user's input is at fault (runtime resolution, missing secret, …).
    User(String),
    /// The environment or proef is at fault (engine infra, abandoned budget).
    System(String),
}

enum Msg {
    BatchBegin {
        scenario: usize,
        deadline: Instant,
    },
    Done {
        scenario: usize,
        outcome: ScenarioOutcome,
    },
}

/// Execute `specs` and return the summary. Emits the full event stream on
/// `events` (`RunStarted` … `RunFinished` — ADR-0008).
// One cohesive listing of the dispatch/watchdog loop; splitting hides the order.
#[allow(clippy::too_many_lines)]
pub fn run(
    specs: Vec<ScenarioSpec>,
    engines: &Arc<Vec<Box<dyn EngineFactory>>>,
    store: &Arc<Mutex<GlobalStore>>,
    config: &RunConfig,
    events: &EventSink,
    cancel: &CancellationToken,
) -> RunSummary {
    events.emit(&Event::RunStarted {
        schema: EVENT_SCHEMA_VERSION,
        run_id: Arc::clone(&config.run_id),
    });

    let (tx, rx) = mpsc::channel::<Msg>();
    let mut queue: std::collections::VecDeque<(usize, ScenarioSpec)> =
        specs.into_iter().enumerate().collect();
    let total = queue.len();
    let mut active: BTreeMap<usize, Instant> = BTreeMap::new();
    let mut outcomes: Vec<ScenarioOutcome> = Vec::new();
    let grace = Duration::from_secs(2);

    while outcomes.len() < total {
        // Fill free slots.
        while active.len() < config.jobs.max(1) {
            let Some((index, spec)) = queue.pop_front() else {
                break;
            };
            if cancel.is_cancelled() {
                events.emit(&Event::ScenarioFinished {
                    scenario: Arc::clone(&spec.name),
                    status: Status::Skipped,
                });
                outcomes.push(ScenarioOutcome {
                    file: spec.file,
                    name: spec.name,
                    line: spec.line,
                    status: Status::Skipped,
                    steps: Vec::new(),
                    fault: None,
                });
                continue;
            }
            let initial_deadline = Instant::now() + config.default_batch_budget + grace;
            active.insert(index, initial_deadline);
            spawn_scenario(
                index,
                spec,
                Arc::clone(engines),
                Arc::clone(store),
                config.clone(),
                events.clone(),
                cancel.child_token(),
                tx.clone(),
            );
        }
        if active.is_empty() {
            continue; // only skipped scenarios remained in the queue
        }

        // Wait for progress, bounded by the earliest active deadline.
        let now = Instant::now();
        let next_deadline = active.values().min().copied().unwrap_or(now + grace);
        let wait = next_deadline
            .saturating_duration_since(now)
            .max(Duration::from_millis(20));
        match rx.recv_timeout(wait) {
            Ok(Msg::BatchBegin { scenario, deadline }) => {
                if let Some(entry) = active.get_mut(&scenario) {
                    *entry = deadline + grace;
                }
            }
            Ok(Msg::Done { scenario, outcome }) => {
                if active.remove(&scenario).is_some() {
                    events.emit(&Event::ScenarioFinished {
                        scenario: Arc::clone(&outcome.name),
                        status: outcome.status,
                    });
                    outcomes.push(outcome);
                }
                // else: a previously-abandoned thread finished late — ignored.
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        // Sweep expired deadlines on *every* turn of the loop — a steady
        // stream of messages from healthy scenarios must not keep a hung
        // one alive past its budget.
        sweep_expired(&mut active, &mut outcomes, events);
    }

    let passed = outcomes
        .iter()
        .filter(|o| matches!(o.status, Status::Passed | Status::Warned))
        .count();
    let failed = outcomes
        .iter()
        .filter(|o| o.status == Status::Failed)
        .count();
    let skipped = outcomes
        .iter()
        .filter(|o| o.status == Status::Skipped)
        .count();
    let cancelled = cancel.is_cancelled();
    events.emit(&Event::RunFinished {
        passed,
        failed,
        skipped,
        cancelled,
    });
    RunSummary {
        outcomes,
        passed,
        failed,
        skipped,
        cancelled,
    }
}

/// Abandon every scenario whose deadline has passed: record a `System` fault
/// and detach the thread (ADR-0007 — the process reaps it at exit).
fn sweep_expired(
    active: &mut BTreeMap<usize, Instant>,
    outcomes: &mut Vec<ScenarioOutcome>,
    events: &EventSink,
) {
    let now = Instant::now();
    let expired: Vec<usize> = active
        .iter()
        .filter(|(_, deadline)| **deadline <= now)
        .map(|(index, _)| *index)
        .collect();
    for index in expired {
        active.remove(&index);
        let outcome = ScenarioOutcome {
            file: Arc::from("(abandoned)"),
            name: Arc::from(format!("scenario #{index}")),
            line: 0,
            status: Status::Failed,
            steps: Vec::new(),
            fault: Some(Fault::System(
                "batch budget exceeded — scenario thread abandoned (ADR-0007)".to_owned(),
            )),
        };
        events.emit(&Event::ScenarioFinished {
            scenario: Arc::clone(&outcome.name),
            status: Status::Failed,
        });
        outcomes.push(outcome);
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_scenario(
    index: usize,
    spec: ScenarioSpec,
    engines: Arc<Vec<Box<dyn EngineFactory>>>,
    store: Arc<Mutex<GlobalStore>>,
    config: RunConfig,
    events: EventSink,
    cancel: CancellationToken,
    tx: mpsc::Sender<Msg>,
) {
    std::thread::spawn(move || {
        let outcome = run_scenario(
            spec,
            &engines,
            &store,
            &config,
            &events,
            &cancel,
            |budget| {
                let _ = tx.send(Msg::BatchBegin {
                    scenario: index,
                    deadline: Instant::now() + budget,
                });
            },
        );
        let _ = tx.send(Msg::Done {
            scenario: index,
            outcome,
        });
    });
}

// One cohesive listing of the scenario lifecycle; splitting hides the order.
#[allow(clippy::too_many_lines)]
fn run_scenario(
    spec: ScenarioSpec,
    engines: &[Box<dyn EngineFactory>],
    store: &Mutex<GlobalStore>,
    config: &RunConfig,
    events: &EventSink,
    cancel: &CancellationToken,
    heartbeat: impl Fn(Duration),
) -> ScenarioOutcome {
    let (file, name, line) = (Arc::clone(&spec.file), Arc::clone(&spec.name), spec.line);
    let outcome = move |status, steps, fault| ScenarioOutcome {
        file: Arc::clone(&file),
        name: Arc::clone(&name),
        line,
        status,
        steps,
        fault,
    };

    events.emit(&Event::ScenarioStarted {
        scenario: Arc::clone(&spec.name),
        file: Arc::clone(&spec.file),
    });

    // Prepare against a snapshot of the shared globals (lower-time reads).
    let snapshot = match store.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => {
            return outcome(
                Status::Failed,
                Vec::new(),
                Some(Fault::System("global store lock poisoned".to_owned())),
            );
        }
    };
    let mut world = World::new(snapshot);
    let prepared = match (spec.prepare)(&world) {
        Ok(prepared) => prepared,
        Err(diags) => {
            let detail = diags
                .iter()
                .map(|d| d.message.clone())
                .collect::<Vec<_>>()
                .join("; ");
            return outcome(Status::Failed, Vec::new(), Some(Fault::User(detail)));
        }
    };

    let mut sessions: Vec<(String, Box<dyn crate::engine::EngineSession>)> = Vec::new();
    let mut steps: Vec<StepOutcome> = Vec::new();
    let mut fault: Option<Fault> = None;
    let mut failed = false;

    let mut interrupted = false;
    for batch in &prepared.batches {
        if failed {
            break;
        }
        if cancel.is_cancelled() {
            // Batches remain — the scenario did not run to completion and must
            // not report `Passed` (a cancelled run would otherwise exit 0).
            interrupted = true;
            break;
        }
        let engine_id = batch.engine.as_str().to_owned();
        if !sessions.iter().any(|(id, _)| *id == engine_id) {
            let Some(factory) = engines.iter().find(|f| f.id() == engine_id) else {
                fault = Some(Fault::System(format!(
                    "no engine registered for `{engine_id}`"
                )));
                failed = true;
                break;
            };
            let ctx = ScenarioCtx {
                run_id: Arc::clone(&config.run_id),
                scenario: Arc::clone(&spec.name),
                artifact: prepared.artifact.clone(),
                secrets: Arc::clone(&config.secrets),
                http: config.http,
                file_root: spec.file_root.clone(),
            };
            match factory.open(&ctx) {
                Ok(session) => sessions.push((engine_id.clone(), session)),
                Err(err) => {
                    fault = Some(Fault::System(format!(
                        "cannot open engine `{engine_id}`: {err}"
                    )));
                    failed = true;
                    break;
                }
            }
        }
        let Some((_, session)) = sessions.iter_mut().find(|(id, _)| *id == engine_id) else {
            break; // unreachable: just ensured above
        };

        let budget = session
            .batch_budget(batch)
            .unwrap_or(config.default_batch_budget);
        heartbeat(budget);
        events.emit(&Event::BatchStarted {
            scenario: Arc::clone(&spec.name),
            engine: Arc::from(engine_id.as_str()),
            steps: batch.steps.len(),
        });

        let result = session.run_batch(batch, &mut world, events, cancel);
        let all_optional = batch.steps.iter().all(|s| s.optional);
        for mut step_outcome in result.steps {
            if step_outcome.status == Status::Failed && all_optional {
                step_outcome.status = Status::Warned;
            }
            steps.push(step_outcome);
        }
        if let Some(err) = result.error {
            if all_optional {
                // `optional:` — warn and continue (segmentation isolates it).
                continue;
            }
            match err.class {
                crate::error::EngineErrorClass::AssertFailed => {}
                crate::error::EngineErrorClass::Infra | crate::error::EngineErrorClass::Setup => {
                    fault = Some(Fault::System(err.message.clone()));
                }
            }
            failed = true;
        }
    }

    for (_, session) in sessions.iter_mut().rev() {
        let _ = session.finish();
    }

    // Merge global promotions back through the store lock (§12) — the write
    // set only. Writing the whole world back would clobber keys another
    // scenario promoted after this one took its snapshot (lost update).
    if let Ok(mut guard) = store.lock() {
        for (key, value) in world.promotions() {
            guard.insert(key, value.clone());
        }
    }

    let status = if failed || steps.iter().any(|s| s.status == Status::Failed) {
        Status::Failed
    } else if interrupted {
        Status::Skipped
    } else {
        Status::Passed
    };
    outcome(status, steps, fault)
}
