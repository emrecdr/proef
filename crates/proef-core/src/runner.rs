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

use std::collections::{BTreeMap, HashSet};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
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
    /// This scenario's secrets as **engine variable name → secret name**. The
    /// two differ when a fragment binding renamed one (ADR-0018); an inline
    /// `${secret:X}` maps `X` to itself, so this covers every secret the
    /// scenario needs, not just the renamed ones.
    pub secret_bindings: std::collections::BTreeMap<String, String>,
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
        self.exit_code_excluding(&[])
    }

    /// [`Self::exit_code`], treating the given `(file, name)` scenarios as
    /// non-gating (`@quarantine`): their *test-failures* no longer count toward
    /// the exit code, but a `System`/`User` fault still does — quarantine is for
    /// flaky tests, not broken input or infra.
    pub fn exit_code_excluding(&self, non_gating: &[(String, String)]) -> ExitCode {
        let mut worst = if self.cancelled {
            ExitCode::TestFailure
        } else {
            ExitCode::Success
        };
        for outcome in &self.outcomes {
            let quarantined = non_gating.iter().any(|(file, name)| {
                file.as_str() == outcome.file.as_ref() && name.as_str() == outcome.name.as_ref()
            });
            let code = match (&outcome.fault, outcome.status) {
                (Some(Fault::System(_)), _) => ExitCode::SystemError,
                (Some(Fault::User(_)), _) => ExitCode::UserError,
                (None, Status::Failed) if !quarantined => ExitCode::TestFailure,
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
    /// Slug of the emitted artifact, when one exists (drives the CLI's
    /// `reproduce:` line — the emitter's naming is never re-derived).
    pub artifact_slug: Option<Arc<str>>,
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

/// A scenario's run-wide identity: `(file, scenario)`, matching the pairing
/// `identities`/`ScenarioOutcome` already use.
type ScenarioId = (Arc<str>, Arc<str>);

/// The gate's mutable state, behind one lock (see [`RecordGate`]).
struct GateState {
    /// Scenario identities that have been finalized — a worker's late event
    /// for one of these is dropped rather than reaching `inner`.
    closed: HashSet<ScenarioId>,
    /// Set once, after `RunFinished` is written. Once `true`, nothing reaches
    /// `inner` again, for any identity.
    run_closed: bool,
}

/// Wraps the run's sink so a finalized scenario's late events never reach the
/// record. An abandoned scenario's thread is detached and observes its
/// cancellation token only at its next batch boundary (ADR-0007), so it can
/// still try to emit after the sweep recorded its outcome — and after the run
/// itself was finalized. The record's tail must be the tail.
///
/// One gate, two write paths, one lock: [`Self::scenario_sink`] hands each
/// worker thread a sink pre-bound to its own scenario identity (so it filters
/// without needing to inspect every event variant for `scenario`/`file`
/// fields — not all of them carry both); [`Self::finish_scenario`] is called
/// directly by the dispatcher thread, from both the normal completion path
/// and `sweep_expired`, to emit a scenario's terminal event and mark it
/// closed. Every read *and* write of [`GateState`] — including the emit
/// itself — happens under the same `Mutex`, so "is this identity still open"
/// and "write the event" are one atomic step rather than a check racing a
/// concurrent close. [`Self::close_run`] is called only after `RunFinished`
/// has already been written, per [`Self::emit_run_level`].
#[derive(Clone)]
struct RecordGate {
    inner: EventSink,
    state: Arc<Mutex<GateState>>,
}

impl RecordGate {
    fn new(inner: EventSink) -> Self {
        Self {
            inner,
            state: Arc::new(Mutex::new(GateState {
                closed: HashSet::new(),
                run_closed: false,
            })),
        }
    }

    /// A sink bound to one scenario's identity, handed to its worker thread.
    /// Every event passed through it is dropped once the run is closed, or
    /// once this scenario has been finalized via [`Self::finish_scenario`] —
    /// including when that happened on the *dispatcher* thread (the watchdog
    /// sweep), racing ahead of this scenario's own thread. The state lock is
    /// held across the emit itself: a check-then-act (read the flag, drop
    /// the lock, then emit) would leave a window for a worker to be
    /// preempted right there and still write after the tail — the emit is
    /// part of the same critical section as the check, not a step after it.
    fn scenario_sink(&self, file: Arc<str>, scenario: Arc<str>) -> EventSink {
        let inner = self.inner.clone();
        let state = Arc::clone(&self.state);
        let id: ScenarioId = (file, scenario);
        EventSink::new(move |event| {
            let guard = state.lock().unwrap_or_else(PoisonError::into_inner);
            if guard.run_closed || guard.closed.contains(&id) {
                return;
            }
            inner.emit(event);
            // `guard` drops here, after the emit — not before it.
        })
    }

    /// Emit a scenario's terminal `ScenarioFinished` and mark it closed under
    /// one lock acquisition, so no [`Self::scenario_sink`] call for this
    /// identity — nor a concurrent [`Self::close_run`] — can land between the
    /// two: the terminal event and the closing of its identity are atomic.
    /// Called from both the dispatcher's normal completion path and
    /// `sweep_expired` — the two places a scenario is ever finalized.
    fn finish_scenario(&self, event: &Event, file: &Arc<str>, scenario: &Arc<str>) {
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !guard.run_closed {
            self.inner.emit(event);
        }
        guard
            .closed
            .insert((Arc::clone(file), Arc::clone(scenario)));
    }

    /// Emit an event that carries no single-scenario identity (`RunStarted`,
    /// `RunFinished`) — gated only by the run-level close.
    fn emit_run_level(&self, event: &Event) {
        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !guard.run_closed {
            self.inner.emit(event);
        }
    }

    /// Shut the gate for good. Call only after `RunFinished` has already
    /// been written via [`Self::emit_run_level`] — the tail must be written
    /// before the gate closes, never the other way around.
    fn close_run(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .run_closed = true;
    }
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
    // Every emission in this function goes through the gate, never `events`
    // directly (one mechanism deciding what reaches the record — see
    // `RecordGate`), so late writes from an abandoned scenario's detached
    // thread can never slip past it.
    let gate = RecordGate::new(events.clone());
    gate.emit_run_level(&Event::RunStarted {
        schema: EVENT_SCHEMA_VERSION,
        run_id: Arc::clone(&config.run_id),
    });

    let (tx, rx) = mpsc::channel::<Msg>();
    // Identities survive the specs' move into the queue, so an abandoned
    // scenario is reported as itself, never as a synthetic placeholder.
    let identities: Vec<(Arc<str>, Arc<str>, usize)> = specs
        .iter()
        .map(|spec| (Arc::clone(&spec.file), Arc::clone(&spec.name), spec.line))
        .collect();
    let mut queue: std::collections::VecDeque<(usize, ScenarioSpec)> =
        specs.into_iter().enumerate().collect();
    let total = queue.len();
    // Deadline plus the scenario's child cancellation token — retained so
    // abandonment can cancel the detached thread's work instead of leaving it
    // appending events forever (record hygiene; engines gain real stop points
    // as they support cancellation).
    let mut active: BTreeMap<usize, (Instant, CancellationToken)> = BTreeMap::new();
    let mut outcomes: Vec<ScenarioOutcome> = Vec::new();
    let grace = Duration::from_secs(2);

    while outcomes.len() < total {
        // Fill free slots.
        while active.len() < config.jobs.max(1) {
            let Some((index, spec)) = queue.pop_front() else {
                break;
            };
            if cancel.is_cancelled() {
                gate.finish_scenario(
                    &Event::ScenarioFinished {
                        scenario: Arc::clone(&spec.name),
                        file: Arc::clone(&spec.file),
                        status: Status::Skipped,
                        timestamp_ms: None,
                        worker: None,
                        phase: None,
                    },
                    &spec.file,
                    &spec.name,
                );
                outcomes.push(ScenarioOutcome {
                    file: spec.file,
                    name: spec.name,
                    line: spec.line,
                    status: Status::Skipped,
                    steps: Vec::new(),
                    fault: None,
                    artifact_slug: None,
                });
                continue;
            }
            let initial_deadline = Instant::now() + config.default_batch_budget + grace;
            let child = cancel.child_token();
            active.insert(index, (initial_deadline, child.clone()));
            let scenario_events =
                gate.scenario_sink(Arc::clone(&spec.file), Arc::clone(&spec.name));
            spawn_scenario(
                index,
                spec,
                Arc::clone(engines),
                Arc::clone(store),
                config.clone(),
                scenario_events,
                child,
                tx.clone(),
            );
        }
        if active.is_empty() {
            continue; // only skipped scenarios remained in the queue
        }

        // Wait for progress, bounded by the earliest active deadline.
        let now = Instant::now();
        let next_deadline = active
            .values()
            .map(|(deadline, _)| *deadline)
            .min()
            .unwrap_or(now + grace);
        let wait = next_deadline
            .saturating_duration_since(now)
            .max(Duration::from_millis(20));
        match rx.recv_timeout(wait) {
            Ok(Msg::BatchBegin { scenario, deadline }) => {
                if let Some(entry) = active.get_mut(&scenario) {
                    entry.0 = deadline + grace;
                }
            }
            Ok(Msg::Done { scenario, outcome }) => {
                if active.remove(&scenario).is_some() {
                    gate.finish_scenario(
                        &Event::ScenarioFinished {
                            scenario: Arc::clone(&outcome.name),
                            file: Arc::clone(&outcome.file),
                            status: outcome.status,
                            timestamp_ms: None,
                            worker: None,
                            phase: None,
                        },
                        &outcome.file,
                        &outcome.name,
                    );
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
        sweep_expired(&mut active, &mut outcomes, &gate, &identities);
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
    gate.emit_run_level(&Event::RunFinished {
        passed,
        failed,
        skipped,
        cancelled,
    });
    // The tail is written — only now does the gate shut. Any scenario thread
    // still detached out there (an abandoned one, cooperatively cancelled but
    // not yet reaped, ADR-0007) can no longer reach the sink at all.
    gate.close_run();
    RunSummary {
        outcomes,
        passed,
        failed,
        skipped,
        cancelled,
    }
}

/// Abandon every scenario whose deadline has passed: cancel its child token,
/// record a `System` fault, and detach the thread (ADR-0007 — the process
/// reaps it at exit; the cancelled token is the thread's cooperative signal
/// to stop). Cooperation is not guaranteed by every engine, so `gate` is the
/// actual backstop: `finish_scenario` closes this scenario's identity right
/// here, before the detached thread can notice its token and try to keep
/// appending to the record.
fn sweep_expired(
    active: &mut BTreeMap<usize, (Instant, CancellationToken)>,
    outcomes: &mut Vec<ScenarioOutcome>,
    gate: &RecordGate,
    identities: &[(Arc<str>, Arc<str>, usize)],
) {
    let now = Instant::now();
    let expired: Vec<usize> = active
        .iter()
        .filter(|(_, (deadline, _))| *deadline <= now)
        .map(|(index, _)| *index)
        .collect();
    for index in expired {
        if let Some((_, token)) = active.remove(&index) {
            token.cancel();
        }
        // `active` keys are spec indices and `identities` is built 1:1 from
        // the same specs — the lookup cannot miss.
        let (file, name, line) = &identities[index];
        let outcome = ScenarioOutcome {
            file: Arc::clone(file),
            name: Arc::clone(name),
            line: *line,
            status: Status::Failed,
            steps: Vec::new(),
            fault: Some(Fault::System(
                "batch budget exceeded — scenario thread abandoned (ADR-0007)".to_owned(),
            )),
            artifact_slug: None,
        };
        gate.finish_scenario(
            &Event::ScenarioFinished {
                scenario: Arc::clone(&outcome.name),
                file: Arc::clone(&outcome.file),
                status: Status::Failed,
                timestamp_ms: None,
                worker: None,
                phase: None,
            },
            &outcome.file,
            &outcome.name,
        );
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
        let identity = (Arc::clone(&spec.file), Arc::clone(&spec.name), spec.line);
        let heartbeat_tx = tx.clone();
        // A panicking engine or prepare closure must never look like a hang:
        // contain it, report a System fault under the real identity, and let
        // the dispatcher move on immediately instead of waiting out the budget.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_scenario(
                spec,
                &engines,
                &store,
                &config,
                &events,
                &cancel,
                |budget| {
                    let _ = heartbeat_tx.send(Msg::BatchBegin {
                        scenario: index,
                        deadline: Instant::now() + budget,
                    });
                },
            )
        }));
        let outcome = result.unwrap_or_else(|panic| {
            let message = panic
                .downcast_ref::<&str>()
                .map(ToString::to_string)
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "opaque panic payload".to_owned());
            ScenarioOutcome {
                file: identity.0,
                name: identity.1,
                line: identity.2,
                status: Status::Failed,
                steps: Vec::new(),
                fault: Some(Fault::System(format!(
                    "scenario thread panicked: {message}"
                ))),
                artifact_slug: None,
            }
        });
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
    let outcome = move |status, steps, fault, artifact_slug| ScenarioOutcome {
        file: Arc::clone(&file),
        name: Arc::clone(&name),
        line,
        status,
        steps,
        fault,
        artifact_slug,
    };

    events.emit(&Event::ScenarioStarted {
        scenario: Arc::clone(&spec.name),
        file: Arc::clone(&spec.file),
        timestamp_ms: None,
        worker: None,
        phase: None,
    });

    // Prepare against a snapshot of the shared globals (lower-time reads).
    let snapshot = match store.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => {
            return outcome(
                Status::Failed,
                Vec::new(),
                Some(Fault::System("global store lock poisoned".to_owned())),
                None,
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
            return outcome(Status::Failed, Vec::new(), Some(Fault::User(detail)), None);
        }
    };

    let mut sessions: Vec<(String, Box<dyn crate::engine::EngineSession>)> = Vec::new();
    let mut steps: Vec<StepOutcome> = Vec::new();
    let mut fault: Option<Fault> = None;
    let mut failed = false;

    let mut interrupted = false;
    let mut processed = 0usize;
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
                secret_bindings: Arc::new(prepared.secret_bindings.clone()),
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
                // The batch WAS dispatched and its steps already carry real
                // outcomes: count it, or the unreached-steps loop below would
                // re-report it as Skipped on top of them (ADR-0008).
                processed += 1;
                continue;
            }
            match err.class {
                crate::error::EngineErrorClass::AssertFailed => {}
                crate::error::EngineErrorClass::UserInput => {
                    fault = Some(Fault::User(err.message.clone()));
                }
                crate::error::EngineErrorClass::Infra | crate::error::EngineErrorClass::Setup => {
                    fault = Some(Fault::System(err.message.clone()));
                }
            }
            failed = true;
        }
        processed += 1;
    }

    // Every authored step gets an outcome: batches never dispatched (earlier
    // failure or cancellation) report their steps as Skipped instead of
    // silently vanishing from console, record, and JUnit alike.
    let unreached_reason = if interrupted {
        "not run (run cancelled)"
    } else {
        "not run (an earlier step failed)"
    };
    for batch in prepared.batches.iter().skip(processed) {
        for step in &batch.steps {
            events.emit(&Event::StepFinished {
                scenario: Arc::clone(&spec.name),
                engine: Arc::from(batch.engine.as_str()),
                step: step.step.clone(),
                status: Status::Skipped,
                attempts: 0,
                duration_ms: 0,
                captures: Vec::new(),
                // A step that never ran still says where it would have run
                // from: "not run" is exactly when a reader is reconstructing
                // what the suite was about to do.
                fragment: step.fragment.clone(),
                detail: Some(unreached_reason.to_owned()),
                attempt_details: Vec::new(),
            });
            steps.push(StepOutcome {
                step: step.step.clone(),
                status: Status::Skipped,
                attempts: 0,
                duration: std::time::Duration::ZERO,
                detail: Some(unreached_reason.to_owned()),
                attempt_details: Vec::new(),
                reproduce_hint: None,
                fragment: step.fragment.clone(),
            });
        }
    }

    for (engine_id, session) in sessions.iter_mut().rev() {
        if let Err(err) = session.finish()
            && fault.is_none()
        {
            // Teardown failure on an otherwise-clean scenario is a real infra
            // signal (never silently swallowed); a scenario that already
            // failed keeps its primary fault.
            fault = Some(Fault::System(format!(
                "engine `{engine_id}` teardown failed: {}",
                err.message
            )));
        }
    }

    // Merge global promotions back through the store lock (§12) — the write
    // set only. Writing the whole world back would clobber keys another
    // scenario promoted after this one took its snapshot (lost update).
    // Poison recovery is sound here: store inserts are plain map writes with
    // no cross-key invariant a panicked holder could have torn — dropping the
    // write set instead would silently lose `saveAs: global` promotions from
    // a scenario that reports Passed.
    {
        let mut guard = store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    let artifact_slug = prepared
        .artifact
        .as_ref()
        .map(|artifact| Arc::clone(&artifact.slug));
    outcome(status, steps, fault, artifact_slug)
}
