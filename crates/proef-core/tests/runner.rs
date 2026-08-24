//! Orchestrator seam tests over a mock engine: batch-index routing (the M6
//! zero-core-diff prerequisite), write-set-only global merge-back, and
//! cancellation semantics (a cancelled run must never exit 0).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use proef_core::cancel::CancellationToken;
use proef_core::engine::{
    DoctorCheck, EngineFactory, EngineId, EngineSession, HttpDefaults, ScenarioCtx, StepKindSpec,
};
use proef_core::error::{EngineError, ExitCode};
use proef_core::event::{Event, EventSink};
use proef_core::runner::{Fault, Prepared, RunConfig, ScenarioSpec, run};
use proef_core::step::{
    BatchResult, LoweredStep, Status, StepBatch, StepOutcome, StepPayload, StepRef,
};
use proef_core::world::{GlobalStore, Value, World};

const NO_KINDS: &[StepKindSpec] = &[];

/// Behavior hook: called per batch with (scenario, batch, world, cancel).
type OnBatch = Arc<dyn Fn(&str, &StepBatch, &mut World, &CancellationToken) + Send + Sync>;

struct MockFactory {
    id: &'static str,
    on_batch: OnBatch,
}

struct MockSession {
    scenario: Arc<str>,
    on_batch: OnBatch,
}

impl EngineFactory for MockFactory {
    fn id(&self) -> &'static str {
        self.id
    }

    fn step_kinds(&self) -> &'static [StepKindSpec] {
        NO_KINDS
    }

    fn doctor(&self) -> Vec<DoctorCheck> {
        Vec::new()
    }

    fn open(&self, ctx: &ScenarioCtx) -> Result<Box<dyn EngineSession>, EngineError> {
        Ok(Box::new(MockSession {
            scenario: Arc::clone(&ctx.scenario),
            on_batch: Arc::clone(&self.on_batch),
        }))
    }
}

impl EngineSession for MockSession {
    fn run_batch(
        &mut self,
        batch: &StepBatch,
        world: &mut World,
        _events: &EventSink,
        cancel: &CancellationToken,
    ) -> BatchResult {
        (self.on_batch)(&self.scenario, batch, world, cancel);
        let steps = batch
            .steps
            .iter()
            .map(|step| StepOutcome {
                step: step.step.clone(),
                status: Status::Passed,
                attempts: 1,
                duration: Duration::ZERO,
                detail: None,
                attempt_details: Vec::new(),
                reproduce_hint: None,
                fragment: step.fragment.clone(),
            })
            .collect();
        BatchResult { steps, error: None }
    }

    fn finish(&mut self) -> Result<(), EngineError> {
        Ok(())
    }
}

/// A session that fails its batch: outcomes carry `Failed` and the batch
/// errors, like an exhausted-retries batch would.
struct FailingFactory;
struct FailingSession;

impl EngineFactory for FailingFactory {
    fn id(&self) -> &'static str {
        "failing"
    }

    fn step_kinds(&self) -> &'static [StepKindSpec] {
        NO_KINDS
    }

    fn doctor(&self) -> Vec<DoctorCheck> {
        Vec::new()
    }

    fn open(&self, _ctx: &ScenarioCtx) -> Result<Box<dyn EngineSession>, EngineError> {
        Ok(Box::new(FailingSession))
    }
}

impl EngineSession for FailingSession {
    fn run_batch(
        &mut self,
        batch: &StepBatch,
        _world: &mut World,
        _events: &EventSink,
        _cancel: &CancellationToken,
    ) -> BatchResult {
        let steps = batch
            .steps
            .iter()
            .map(|step| StepOutcome {
                step: step.step.clone(),
                status: Status::Failed,
                attempts: 1,
                duration: Duration::ZERO,
                detail: Some("mock failure".to_owned()),
                attempt_details: Vec::new(),
                reproduce_hint: None,
                fragment: step.fragment.clone(),
            })
            .collect();
        BatchResult {
            steps,
            error: Some(EngineError::infra("mock batch failure")),
        }
    }

    fn finish(&mut self) -> Result<(), EngineError> {
        Ok(())
    }
}

/// Behavior variants for the error-path factory: which failure mode the
/// orchestrator must contain (checklist: error paths need direct tests).
#[derive(Clone, Copy)]
enum Misbehavior {
    /// `run_batch` panics — the dispatcher must contain it, not hang.
    Panic,
    /// `open` fails — a System fault under the scenario's identity.
    OpenFails,
    /// `run_batch` sleeps past every budget — the watchdog must abandon it.
    Hang,
}

struct MisbehavingFactory(Misbehavior);
struct MisbehavingSession(Misbehavior);

impl EngineFactory for MisbehavingFactory {
    fn id(&self) -> &'static str {
        "misbehaving"
    }

    fn step_kinds(&self) -> &'static [StepKindSpec] {
        NO_KINDS
    }

    fn doctor(&self) -> Vec<DoctorCheck> {
        Vec::new()
    }

    fn open(&self, _ctx: &ScenarioCtx) -> Result<Box<dyn EngineSession>, EngineError> {
        match self.0 {
            Misbehavior::OpenFails => Err(EngineError::infra("mock open failure")),
            other => Ok(Box::new(MisbehavingSession(other))),
        }
    }
}

impl EngineSession for MisbehavingSession {
    fn run_batch(
        &mut self,
        _batch: &StepBatch,
        _world: &mut World,
        _events: &EventSink,
        cancel: &CancellationToken,
    ) -> BatchResult {
        match self.0 {
            Misbehavior::Panic => panic!("mock engine panic"),
            Misbehavior::Hang => {
                // Poll the child token instead of one long sleep so the
                // abandonment cancel (swept by the dispatcher) ends the wait.
                let deadline = Instant::now() + Duration::from_secs(30);
                while !cancel.is_cancelled() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                BatchResult {
                    steps: Vec::new(),
                    error: Some(EngineError::infra("hang elapsed")),
                }
            }
            Misbehavior::OpenFails => unreachable!("open never succeeds"),
        }
    }

    fn finish(&mut self) -> Result<(), EngineError> {
        Ok(())
    }
}

fn lowered_step(text: &str) -> LoweredStep {
    LoweredStep {
        step: StepRef {
            file: Arc::from("mock.feature"),
            line: 1,
            text: Arc::from(text),
        },
        kind: "mock".into(),
        payload: StepPayload::Structured(serde_json::Value::Null),
        optional: false,
        when: None,
        label: None,
        fragment: None,
        save_as: BTreeMap::new(),
    }
}

/// A spec whose batches route to `engines_by_batch[i]` with scenario-wide
/// indexes, mirroring what `lower::segment` produces.
fn spec(name: &str, engines_by_batch: &[&'static str]) -> ScenarioSpec {
    let batches: Vec<StepBatch> = engines_by_batch
        .iter()
        .enumerate()
        .map(|(index, engine)| StepBatch {
            index,
            engine: EngineId::from(*engine),
            steps: vec![lowered_step(&format!("step of batch {index}"))],
        })
        .collect();
    ScenarioSpec {
        file: Arc::from("mock.feature"),
        name: Arc::from(name),
        line: 1,
        file_root: None,
        exclusive: false,
        skip: None,
        prepare: Box::new(move |_world| {
            Ok(Prepared {
                batches,
                artifact: None,
                secret_bindings: std::collections::BTreeMap::default(),
            })
        }),
    }
}

fn config(jobs: usize) -> RunConfig {
    RunConfig {
        run_id: Arc::from("test-run"),
        jobs,
        default_batch_budget: Duration::from_secs(10),
        secrets: Arc::new(BTreeMap::new()),
        http: HttpDefaults::default(),
    }
}

fn engines(factories: Vec<Box<dyn EngineFactory>>) -> Arc<Vec<Box<dyn EngineFactory>>> {
    Arc::new(factories)
}

/// The batch a session receives carries the *scenario-wide* ordinal, so a
/// session interleaved with another engine still selects the right sidecar
/// rows: `[mk1, mk2, mk1]` must reach mk1 as indexes 0 and 2 — never 0 and 1.
#[test]
fn interleaved_engines_see_scenario_wide_batch_indexes() {
    let seen: Arc<Mutex<Vec<(String, usize)>>> = Arc::new(Mutex::new(Vec::new()));
    let record: OnBatch = {
        let seen = Arc::clone(&seen);
        Arc::new(move |_, batch, _, _| {
            seen.lock()
                .unwrap()
                .push((batch.engine.as_str().to_owned(), batch.index));
        })
    };
    let engines = engines(vec![
        Box::new(MockFactory {
            id: "mk1",
            on_batch: Arc::clone(&record),
        }),
        Box::new(MockFactory {
            id: "mk2",
            on_batch: record,
        }),
    ]);
    let store = Arc::new(Mutex::new(GlobalStore::new()));

    let summary = run(
        vec![spec("interleaved", &["mk1", "mk2", "mk1"])],
        &engines,
        &store,
        &config(1),
        &EventSink::null(),
        &CancellationToken::new(),
    );

    assert_eq!(summary.exit_code(), ExitCode::Success);
    let seen = seen.lock().unwrap();
    assert_eq!(
        *seen,
        vec![
            ("mk1".to_owned(), 0),
            ("mk2".to_owned(), 1),
            ("mk1".to_owned(), 2),
        ]
    );
}

/// Lost-update regression: merge-back must write only the scenario's
/// promotions. Two overlapping scenarios both snapshot `x=1`; A promotes
/// `x=2`, then B (still holding the stale snapshot) promotes `y=3`. B's merge
/// must not write its stale `x=1` back over A's promotion.
#[test]
fn merge_back_is_write_set_only() {
    let store = Arc::new(Mutex::new(GlobalStore::new()));
    store.lock().unwrap().insert("x", Value::Int(1));

    // Both sessions rendezvous so both scenarios have snapshotted the store
    // before either merges; B then waits until A's promotion actually landed.
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let on_batch: OnBatch = {
        let barrier = Arc::clone(&barrier);
        let store = Arc::clone(&store);
        Arc::new(move |scenario, _, world, _| {
            barrier.wait();
            if scenario == "promotes-x" {
                world.set_global("x", Value::Int(2));
            } else {
                // Wait for A's merge-back to land before finishing B.
                let deadline = Instant::now() + Duration::from_secs(10);
                while store.lock().unwrap().get("x") != Some(&Value::Int(2)) {
                    assert!(Instant::now() < deadline, "A's promotion never landed");
                    std::thread::yield_now();
                }
                world.set_global("y", Value::Int(3));
            }
        })
    };
    let engines = engines(vec![Box::new(MockFactory {
        id: "mock",
        on_batch,
    })]);

    let summary = run(
        vec![spec("promotes-x", &["mock"]), spec("promotes-y", &["mock"])],
        &engines,
        &store,
        &config(2),
        &EventSink::null(),
        &CancellationToken::new(),
    );

    assert_eq!(summary.failed, 0);
    let store = store.lock().unwrap();
    assert_eq!(
        store.get("x"),
        Some(&Value::Int(2)),
        "A's promotion survives"
    );
    assert_eq!(store.get("y"), Some(&Value::Int(3)), "B's promotion lands");
}

/// An erroring `optional:` batch is warn-and-continue — and it was
/// *dispatched*, so the unreached-steps accounting must count it: its steps
/// already carry real outcomes, and later batches must not be re-reported as
/// `Skipped` on top of their own outcomes (ADR-0008 — one outcome per step).
#[test]
fn optional_batch_error_does_not_rereport_later_batches() {
    let ok: OnBatch = Arc::new(|_, _, _, _| {});
    let engines = engines(vec![
        Box::new(FailingFactory),
        Box::new(MockFactory {
            id: "mock",
            on_batch: ok,
        }),
    ]);
    let store = Arc::new(Mutex::new(GlobalStore::new()));
    let batches = vec![
        StepBatch {
            index: 0,
            engine: EngineId::from("failing"),
            steps: vec![LoweredStep {
                optional: true,
                ..lowered_step("optional probe")
            }],
        },
        StepBatch {
            index: 1,
            engine: EngineId::from("mock"),
            steps: vec![lowered_step("real step")],
        },
    ];
    let spec = ScenarioSpec {
        file: Arc::from("mock.feature"),
        name: Arc::from("optional-error"),
        line: 1,
        file_root: None,
        exclusive: false,
        skip: None,
        prepare: Box::new(move |_world| {
            Ok(Prepared {
                batches,
                artifact: None,
                secret_bindings: std::collections::BTreeMap::default(),
            })
        }),
    };

    let summary = run(
        vec![spec],
        &engines,
        &store,
        &config(1),
        &EventSink::null(),
        &CancellationToken::new(),
    );

    assert_eq!(summary.failed, 0, "an optional failure never fails the run");
    let outcome = summary
        .outcomes
        .iter()
        .find(|o| o.name.as_ref() == "optional-error")
        .unwrap();
    assert_eq!(outcome.steps.len(), 2, "one outcome per authored step");
    assert_eq!(outcome.steps[0].status, Status::Warned);
    assert_eq!(outcome.steps[1].status, Status::Passed);
}

/// A panicking engine is contained: the scenario reports a `System` fault
/// under its real identity (never a hang), sibling scenarios still run, and
/// the run exits `SystemError`.
#[test]
fn engine_panic_is_contained_as_a_system_fault() {
    let ok: OnBatch = Arc::new(|_, _, _, _| {});
    let engines = engines(vec![
        Box::new(MisbehavingFactory(Misbehavior::Panic)),
        Box::new(MockFactory {
            id: "mock",
            on_batch: ok,
        }),
    ]);
    let store = Arc::new(Mutex::new(GlobalStore::new()));

    let summary = run(
        vec![spec("panics", &["misbehaving"]), spec("healthy", &["mock"])],
        &engines,
        &store,
        &config(1),
        &EventSink::null(),
        &CancellationToken::new(),
    );

    assert_eq!(summary.failed, 1);
    assert_eq!(summary.passed, 1, "the healthy sibling still ran");
    let panicked = summary
        .outcomes
        .iter()
        .find(|o| o.name.as_ref() == "panics")
        .unwrap();
    assert_eq!(panicked.status, Status::Failed);
    assert!(
        matches!(&panicked.fault, Some(Fault::System(m)) if m.contains("panicked")),
        "{:?}",
        panicked.fault
    );
    assert_eq!(summary.exit_code(), ExitCode::SystemError);
}

/// A failing `open` is a `System` fault on the scenario, not a crash — and an
/// engine id no factory claims is the same class.
#[test]
fn open_failure_and_unknown_engine_are_system_faults() {
    let engines = engines(vec![Box::new(MisbehavingFactory(Misbehavior::OpenFails))]);
    let store = Arc::new(Mutex::new(GlobalStore::new()));

    let summary = run(
        vec![
            spec("open-fails", &["misbehaving"]),
            spec("ghost-engine", &["ghost"]),
        ],
        &engines,
        &store,
        &config(1),
        &EventSink::null(),
        &CancellationToken::new(),
    );

    assert_eq!(summary.failed, 2);
    let open_fails = summary
        .outcomes
        .iter()
        .find(|o| o.name.as_ref() == "open-fails")
        .unwrap();
    assert!(
        matches!(&open_fails.fault, Some(Fault::System(m)) if m.contains("cannot open engine")),
        "{:?}",
        open_fails.fault
    );
    let ghost = summary
        .outcomes
        .iter()
        .find(|o| o.name.as_ref() == "ghost-engine")
        .unwrap();
    assert!(
        matches!(&ghost.fault, Some(Fault::System(m)) if m.contains("no engine registered")),
        "{:?}",
        ghost.fault
    );
    assert_eq!(summary.exit_code(), ExitCode::SystemError);
}

/// A batch that outlives its budget is abandoned by the watchdog: `System`
/// fault, `Failed` status, and the dispatcher cancels the scenario's child
/// token so the detached thread stops (observed via the polled hang ending).
#[test]
fn watchdog_abandons_a_hung_scenario() {
    let engines = engines(vec![Box::new(MisbehavingFactory(Misbehavior::Hang))]);
    let store = Arc::new(Mutex::new(GlobalStore::new()));
    let mut config = config(1);
    config.default_batch_budget = Duration::from_millis(50);

    let summary = run(
        vec![spec("hangs", &["misbehaving"])],
        &engines,
        &store,
        &config,
        &EventSink::null(),
        &CancellationToken::new(),
    );

    assert_eq!(summary.failed, 1);
    let hung = summary
        .outcomes
        .iter()
        .find(|o| o.name.as_ref() == "hangs")
        .unwrap();
    assert_eq!(hung.status, Status::Failed);
    assert!(
        matches!(&hung.fault, Some(Fault::System(m)) if m.contains("abandoned")),
        "{:?}",
        hung.fault
    );
    assert_eq!(summary.exit_code(), ExitCode::SystemError);
}

/// A run cancelled mid-scenario: the interrupted scenario reports `Skipped`
/// (never `Passed` — it did not run to completion), queued scenarios skip, and
/// the run's exit code is non-zero.
#[test]
fn cancelled_run_is_never_success() {
    let root = CancellationToken::new();
    // The session cancels the *root* token during the first batch, as the
    // Ctrl-C handler would.
    let on_batch: OnBatch = {
        let root = root.clone();
        Arc::new(move |_, _, _, _| root.cancel())
    };
    let engines = engines(vec![Box::new(MockFactory {
        id: "mock",
        on_batch,
    })]);
    let store = Arc::new(Mutex::new(GlobalStore::new()));

    let summary = run(
        vec![
            spec("interrupted", &["mock", "mock"]),
            spec("never-dispatched", &["mock"]),
        ],
        &engines,
        &store,
        &config(1),
        &EventSink::null(),
        &root,
    );

    assert!(summary.cancelled);
    assert_eq!(summary.passed, 0, "an interrupted scenario is not a pass");
    assert_eq!(summary.skipped, 2);
    let interrupted = summary
        .outcomes
        .iter()
        .find(|o| o.name.as_ref() == "interrupted")
        .unwrap();
    assert_eq!(interrupted.status, Status::Skipped);
    // The dispatched batch's step plus the unreached batch's Skipped record —
    // every authored step gets an outcome.
    assert_eq!(interrupted.steps.len(), 2);
    assert_eq!(interrupted.steps[1].status, Status::Skipped);
    assert_eq!(summary.exit_code(), ExitCode::TestFailure);
}

/// The record's tail must actually be the tail. A watchdog-abandoned
/// scenario's thread is detached and notices its token only at its next batch
/// boundary, so without a gate it keeps emitting after the run is finalized.
///
/// Two batches are required to observe this: `Misbehavior::Hang` polls its
/// own cancellation token and returns (with an error) shortly after the
/// watchdog abandons it, so with a single batch `processed` already equals
/// `batches.len()` by the time it returns and `run_scenario`'s
/// unreached-batches loop (the one that reports never-dispatched batches as
/// `Skipped`) has nothing left to emit. With a second batch still
/// unprocessed, that loop emits a `StepFinished{Skipped}` for it — and it
/// does so *after* the dispatcher has already recorded the abandonment and
/// emitted `RunFinished` (that sequence runs in microseconds; the hung
/// thread needs a further ~20ms poll tick just to notice cancellation).
#[test]
fn abandoned_scenario_emits_nothing_after_run_finished() {
    let engines = engines(vec![Box::new(MisbehavingFactory(Misbehavior::Hang))]);
    let store = Arc::new(Mutex::new(GlobalStore::new()));
    let mut config = config(1);
    config.default_batch_budget = Duration::from_millis(50);

    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let seen = Arc::clone(&seen);
        EventSink::new(move |event| {
            let label = match event {
                Event::RunStarted { .. } => "run_started",
                Event::RunFinished { .. } => "run_finished",
                Event::ScenarioStarted { .. } => "scenario_started",
                Event::ScenarioFinished { .. } => "scenario_finished",
                Event::StepFinished { .. } => "step_finished",
                _ => "other",
            };
            seen.lock().unwrap().push(label.to_owned());
        })
    };

    let _summary = run(
        vec![spec("hangs", &["misbehaving", "misbehaving"])],
        &engines,
        &store,
        &config,
        &sink,
        &CancellationToken::new(),
    );

    // Give the abandoned thread time to reach its next boundary and try to
    // emit. Without the gate it appends here; with it, nothing arrives.
    std::thread::sleep(Duration::from_millis(500));

    let events = seen.lock().unwrap().clone();
    let tail = events
        .iter()
        .rposition(|e| e == "run_finished")
        .expect("record must contain run_finished");
    assert_eq!(
        tail,
        events.len() - 1,
        "run_finished must be the LAST event; full sequence (0-based) was \
         {events:#?}, with run_finished at index {tail} followed by {:?}",
        &events[tail + 1..]
    );

    // Position alone doesn't rule out the gate over-suppressing — dropping
    // the sweep's own `scenario_finished`, say — and slipping the assertion
    // above by accident. The sequence here is deterministic (one scenario,
    // one job, the hung batch never gets past `BatchStarted`), so pin the
    // whole thing: exactly what a well-behaved abandonment produces, no more
    // and no less.
    assert_eq!(
        events,
        vec![
            "run_started",
            "scenario_started",
            "other",             // BatchStarted for the hung first batch
            "scenario_finished", // the sweep's own terminal event
            "run_finished",
        ],
        "gate must drop only the late event, not real ones"
    );
}

/// An exclusive scenario runs with the pool to itself.
///
/// Asserted by **observed overlap**, not by counting starts: the property is
/// that nothing else is in flight, and only concurrency can show that. Each
/// scenario records the peak occupancy it witnessed, so a run where the
/// exclusive one ever saw a neighbour fails regardless of ordering — and the
/// non-exclusive pair still has to reach 2, or the test would pass on a runner
/// that had quietly become serial and proved nothing.
/// How many scenarios were live at the same time as this one, sampled **across
/// its whole hold** rather than once at entry.
///
/// The single-sample version read like a concurrency check and was not: a
/// neighbour that starts *after* this scenario recorded its count is invisible
/// to it, and "a neighbour joined the exclusive scenario already running" is
/// precisely that shape. Removing half the fill gate left the single-sample
/// tests green. Sampling is a peak *observation*, not a wall-clock assertion —
/// the run is as slow as it needs to be and the assertion is on what was seen.
static LIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn overlap_watcher(name: &str, exclusive: bool, peak: Arc<Mutex<usize>>) -> ScenarioSpec {
    use std::sync::atomic::Ordering;
    ScenarioSpec {
        file: Arc::from("mock.feature"),
        name: Arc::from(name),
        line: 1,
        file_root: None,
        exclusive,
        skip: None,
        prepare: Box::new(move |_world| {
            LIVE.fetch_add(1, Ordering::SeqCst);
            for _ in 0..12 {
                let live = LIVE.load(Ordering::SeqCst);
                {
                    // One acquisition: `*m.lock() = (*m.lock()).max(x)` locks
                    // the same non-reentrant mutex twice and deadlocks.
                    let mut seen = peak.lock().unwrap();
                    *seen = (*seen).max(live);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            LIVE.fetch_sub(1, Ordering::SeqCst);
            Ok(Prepared {
                batches: Vec::new(),
                artifact: None,
                secret_bindings: BTreeMap::default(),
            })
        }),
    }
}

#[test]
fn an_exclusive_scenario_never_shares_the_pool() {
    let peak_alone: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let peak_shared: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let engines = engines(vec![]);
    let store = Arc::new(Mutex::new(GlobalStore::new()));
    let summary = run(
        vec![
            overlap_watcher("shared-a", false, Arc::clone(&peak_shared)),
            overlap_watcher("shared-b", false, Arc::clone(&peak_shared)),
            overlap_watcher("alone", true, Arc::clone(&peak_alone)),
            overlap_watcher("shared-c", false, Arc::clone(&peak_shared)),
        ],
        &engines,
        &store,
        &config(4),
        &EventSink::null(),
        &CancellationToken::new(),
    );

    assert_eq!(summary.outcomes.len(), 4, "every scenario must still run");
    assert_eq!(
        *peak_alone.lock().unwrap(),
        1,
        "an exclusive scenario saw a neighbour — the pool was not its own"
    );
    assert!(
        *peak_shared.lock().unwrap() >= 2,
        "the non-exclusive scenarios never overlapped, so this proves nothing \
         about exclusivity"
    );
}

/// Two exclusive scenarios back to back, with ordinary work either side.
///
/// The pool has to drain, run one alone, refill, drain again, and run the second
/// alone. The interesting state is the flag that says "an exclusive scenario
/// owns the pool": it is cleared when the active set empties, and a version that
/// cleared it one turn late would let the second exclusive scenario start beside
/// the first's tail — or, cleared one turn early, would let a neighbour join.
/// Neither shows up with a single exclusive scenario in the queue.
#[test]
fn back_to_back_exclusive_scenarios_each_get_the_pool_alone() {
    let first: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let second: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let before: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    // Counted apart from `before` on purpose. Sharing one counter let the
    // leading pair's overlap satisfy the assertion, so a pool that never
    // refilled — `exclusive_active` left set once an exclusive scenario had
    // run, turning everything after it serial — passed unnoticed.
    let after: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let engines = engines(vec![]);
    let store = Arc::new(Mutex::new(GlobalStore::new()));
    let summary = run(
        vec![
            overlap_watcher("shared-a", false, Arc::clone(&before)),
            overlap_watcher("shared-b", false, Arc::clone(&before)),
            overlap_watcher("alone-1", true, Arc::clone(&first)),
            overlap_watcher("alone-2", true, Arc::clone(&second)),
            overlap_watcher("shared-c", false, Arc::clone(&after)),
            overlap_watcher("shared-d", false, Arc::clone(&after)),
        ],
        &engines,
        &store,
        &config(4),
        &EventSink::null(),
        &CancellationToken::new(),
    );

    assert_eq!(summary.outcomes.len(), 6, "every scenario must still run");
    assert_eq!(
        *first.lock().unwrap(),
        1,
        "the first exclusive scenario saw a neighbour"
    );
    assert_eq!(
        *second.lock().unwrap(),
        1,
        "the second exclusive scenario saw a neighbour — most likely the first one's tail"
    );
    assert!(
        *before.lock().unwrap() >= 2,
        "the pool never ran anything in parallel, so this proves nothing about exclusivity"
    );
    assert!(
        *after.lock().unwrap() >= 2,
        "the pool never refilled after the exclusive scenarios — parallelism has to \
         come back, or `exclusive-tags` silently serialises the rest of the run"
    );
}

/// Cancelling while an exclusive scenario is waiting for the pool to drain must
/// finish the run, not deadlock.
///
/// The fill gate refuses to start anything while an exclusive scenario is at the
/// head and the pool is non-empty. Cancellation drains the queue by *skipping*
/// its entries, and a gate that skipped past the exclusive head — or one that
/// waited for a pool that would never refill — leaves the loop unable to reach
/// its total. Every scenario must be accounted for either way.
#[test]
fn cancelling_during_an_exclusive_drain_still_completes_the_run() {
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();

    // Cancels from inside the first scenario, i.e. while the exclusive one
    // behind it is at the head of the queue waiting for this slot to free.
    let mut specs = vec![ScenarioSpec {
        file: Arc::from("mock.feature"),
        name: Arc::from("running-when-cancelled"),
        line: 1,
        file_root: None,
        exclusive: false,
        skip: None,
        prepare: Box::new(move |_world| {
            trigger.cancel();
            Ok(Prepared {
                batches: Vec::new(),
                artifact: None,
                secret_bindings: BTreeMap::default(),
            })
        }),
    }];
    for (i, exclusive) in [(1, true), (2, true), (3, false)] {
        specs.push(ScenarioSpec {
            file: Arc::from("mock.feature"),
            name: Arc::from(format!("queued-{i}").as_str()),
            line: 1,
            file_root: None,
            exclusive,
            skip: None,
            prepare: Box::new(|_world| {
                Ok(Prepared {
                    batches: Vec::new(),
                    artifact: None,
                    secret_bindings: BTreeMap::default(),
                })
            }),
        });
    }

    let engines = engines(vec![]);
    let store = Arc::new(Mutex::new(GlobalStore::new()));
    let summary = run(
        specs,
        &engines,
        &store,
        &config(2),
        &EventSink::null(),
        &cancel,
    );

    // The run terminated (a deadlock would hang this test rather than fail it)
    // and every scenario reached an outcome — run or skipped, never dropped.
    assert_eq!(
        summary.outcomes.len(),
        4,
        "a cancelled exclusive drain lost scenarios instead of skipping them"
    );
    assert!(
        summary.outcomes.iter().any(|o| o.status == Status::Skipped),
        "cancellation should have skipped the queued scenarios"
    );
}
