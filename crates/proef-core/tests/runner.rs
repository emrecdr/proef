//! Orchestrator seam tests over a mock engine: batch-index routing (the M6
//! zero-core-diff prerequisite), write-set-only global merge-back, and
//! cancellation semantics (a cancelled run must never exit 0).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use proef_core::cancel::CancellationToken;
use proef_core::engine::{
    DoctorCheck, EngineFactory, EngineId, EngineSession, HttpDefaults, ScenarioCtx, StepKindSpec,
};
use proef_core::error::{EngineError, ExitCode};
use proef_core::event::EventSink;
use proef_core::runner::{Prepared, RunConfig, ScenarioSpec, run};
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
                artifact_span: None,
            })
            .collect();
        BatchResult { steps, error: None }
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
        retry: None,
        when: None,
        label: None,
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
        tags: Vec::new(),
        file_root: None,
        prepare: Box::new(move |_world| {
            Ok(Prepared {
                batches,
                artifact: None,
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
    assert_eq!(interrupted.steps.len(), 1, "first batch did run");
    assert_eq!(summary.exit_code(), ExitCode::TestFailure);
}
