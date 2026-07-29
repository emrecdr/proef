//! Lowered, engine-agnostic step and batch types (TECH-SPEC §3).
//!
//! A scenario lowers to an ordered list of [`LoweredStep`]s, segmented into
//! [`StepBatch`]es of **contiguous same-engine steps** (batched maximally — splits
//! happen only at `optional:` boundaries and engine changes, ADR-0010). Engines
//! return a [`BatchResult`] with one [`StepOutcome`] per executed step.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::engine::EngineId;
use crate::error::EngineError;

/// Anchor back to the authored `.feature` source (file, 1-based line, step text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRef {
    /// Feature file path as authored.
    pub file: Arc<str>,
    /// 1-based line of the step keyword in the feature file.
    pub line: usize,
    /// The full step text (keyword stripped).
    pub text: Arc<str>,
}

/// Identifies the *kind* of a macro step (`hurl`, …). A step kind
/// names the engine that claims it via [`crate::engine::StepKindSpec`] (ADR-0002).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StepKindId(Arc<str>);

impl StepKindId {
    /// The kind name as written in packs (without the trailing `:`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for StepKindId {
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

impl std::fmt::Display for StepKindId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The engine-facing payload of a lowered step.
#[derive(Debug, Clone, PartialEq)]
pub enum StepPayload {
    /// Lowered hurl text (`${…}` resolved, `{{…}}` untouched) — one or more entries.
    HurlEntries(String),
    /// Structured payload — reserved for future non-hurl engines (ADR-0004).
    Structured(serde_json::Value),
}

/// Finite retry policy for a step (`retry:` — infinite retries are rejected at pack
/// load by the finite-retry lint, ADR-0007).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retry {
    /// Maximum number of retries (finite by construction).
    pub count: u32,
    /// Interval between attempts, in milliseconds.
    pub interval_ms: u64,
}

/// A `when:` skip guard: the step runs iff the expression is non-empty after
/// `${…}` resolution (TECH-SPEC §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guard(pub String);

/// One step after macro expansion and `${…}` lowering — engine-agnostic.
#[derive(Debug, Clone, PartialEq)]
pub struct LoweredStep {
    /// Anchor to the authored feature line.
    pub step: StepRef,
    /// Which step kind (and therefore which engine) executes this step.
    pub kind: StepKindId,
    /// The engine-facing payload.
    pub payload: StepPayload,
    /// `optional:` steps warn instead of failing (and segment the batch).
    pub optional: bool,
    /// Finite retry policy, when configured.
    pub retry: Option<Retry>,
    /// Skip guard, when configured (resolved text; the runtime skips the step
    /// when it is empty).
    pub when: Option<Guard>,
    /// Pack-step entry label (events/console), when authored.
    pub label: Option<String>,
    /// `saveAs:` promotions: capture name → `global` (ADR-0005).
    pub save_as: std::collections::BTreeMap<String, String>,
}

/// A contiguous run of same-engine steps, dispatched as one unit (ADR-0002).
#[derive(Debug, Clone, PartialEq)]
pub struct StepBatch {
    /// Ordinal of this batch within the *scenario* (the sidecar map's `batch`
    /// key). Engines must select sidecar entries by this index — a per-session
    /// counter diverges as soon as another engine's batch interleaves.
    pub index: usize,
    /// The engine that executes this batch.
    pub engine: EngineId,
    /// The steps, in authored order.
    pub steps: Vec<LoweredStep>,
}

/// Outcome status of a step or scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Ran and passed.
    Passed,
    /// Ran and failed.
    Failed,
    /// Not run (guard, earlier failure, filter).
    Skipped,
    /// An `optional:` step failed — reported as a warning, run continues.
    Warned,
}

/// Per-step result reported by an engine.
#[derive(Debug, Clone)]
pub struct StepOutcome {
    /// Anchor to the authored feature line.
    pub step: StepRef,
    /// Outcome status.
    pub status: Status,
    /// Number of attempts made (≥ 1 once the step ran).
    pub attempts: u32,
    /// Wall-clock duration of all attempts.
    pub duration: Duration,
    /// Engine-specific detail (assert message, timing breakdown, …).
    pub detail: Option<String>,
    /// Line span in the emitted artifact, when one exists (ADR-0010 sidecar).
    pub artifact_span: Option<(u32, u32)>,
}

/// Result of dispatching one [`StepBatch`] to an engine.
#[derive(Debug)]
pub struct BatchResult {
    /// One outcome per step the engine reached.
    pub steps: Vec<StepOutcome>,
    /// A batch-level failure, if the batch stopped early.
    pub error: Option<EngineError>,
}
