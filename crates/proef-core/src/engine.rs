//! The engine seam (ADR-0002): `EngineFactory` / `EngineSession`, step-kind routing,
//! and capability hooks (`step_kinds`, `doctor`).
//!
//! Both traits are **sync + dyn** (ADR-0006) and used as `Box<dyn …>`. Adding an
//! engine is a new crate implementing both traits plus one registry line in
//! `proef-cli` — with **zero changes to this crate** (the structural acceptance
//! test for M6).
//!
//! # Example: a minimal engine
//!
//! ```
//! use proef_core::cancel::CancellationToken;
//! use proef_core::engine::{
//!     DoctorCheck, DoctorResult, EngineFactory, EngineSession, ScenarioCtx, StepKindSpec,
//! };
//! use proef_core::error::EngineError;
//! use proef_core::event::EventSink;
//! use proef_core::step::{BatchResult, StepBatch};
//! use proef_core::world::World;
//!
//! struct NullEngine;
//! struct NullSession;
//!
//! impl EngineFactory for NullEngine {
//!     fn id(&self) -> &'static str {
//!         "null"
//!     }
//!     fn step_kinds(&self) -> &'static [StepKindSpec] {
//!         const KINDS: &[StepKindSpec] = &[StepKindSpec {
//!             prefix: "null",
//!             schema: "true",
//!             validate: None,
//!         }];
//!         KINDS
//!     }
//!     fn doctor(&self) -> Vec<DoctorCheck> {
//!         Vec::new()
//!     }
//!     fn open(&self, _ctx: &ScenarioCtx) -> Result<Box<dyn EngineSession>, EngineError> {
//!         Ok(Box::new(NullSession))
//!     }
//! }
//!
//! impl EngineSession for NullSession {
//!     fn run_batch(
//!         &mut self,
//!         batch: &StepBatch,
//!         _world: &mut World,
//!         _events: &EventSink,
//!         _cancel: &CancellationToken,
//!     ) -> BatchResult {
//!         BatchResult { steps: Vec::with_capacity(batch.steps.len()), error: None }
//!     }
//!     fn finish(&mut self) -> Result<(), EngineError> {
//!         Ok(())
//!     }
//! }
//!
//! let factory: Box<dyn EngineFactory> = Box::new(NullEngine);
//! assert_eq!(factory.id(), "null");
//! ```

use std::sync::Arc;

use crate::cancel::CancellationToken;
use crate::error::EngineError;
use crate::event::EventSink;
use crate::step::{BatchResult, StepBatch};
use crate::world::World;

/// Identifies an engine (`hurl`, …). A macro step's kind names the
/// engine that executes it (ADR-0002 routing).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EngineId(Arc<str>);

impl EngineId {
    /// The engine id as referenced by step kinds and the registry.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for EngineId {
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

impl std::fmt::Display for EngineId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// An engine's claim on a pack step kind: the key prefix (`hurl`, `web`, …) plus the
/// JSON-Schema fragment describing that step's payload, merged into `proef schema`
/// output (TECH-SPEC §6), and an optional static payload validator used by pack
/// validation pass 7 (probe-instantiation parse — TECH-SPEC §4.1).
#[derive(Debug, Clone, Copy)]
pub struct StepKindSpec {
    /// Step-kind prefix as written in packs (without the trailing `:`).
    pub prefix: &'static str,
    /// JSON-Schema fragment for the step payload (`"true"` = any, until refined).
    pub schema: &'static str,
    /// Probe-validate a lowered payload text (`None` = no static validation).
    /// Keeps the core engine-agnostic: the hurl parser stays behind the seam.
    pub validate: Option<PayloadValidator>,
}

/// An engine-contributed static payload validator (pack validation pass 7).
pub type PayloadValidator = fn(&str) -> Result<(), PayloadProbeError>;

/// A syntax problem found while probe-validating a step payload
/// (1-based line/column **within the payload text**; the pack loader maps it
/// onto the pack file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadProbeError {
    /// 1-based line within the payload text.
    pub line: usize,
    /// 1-based column within that line.
    pub column: usize,
    /// Parser message.
    pub message: String,
}

/// Outcome of one environment check contributed by an engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorResult {
    /// Pass / warn / fail.
    pub status: DoctorStatus,
    /// Human-readable detail (library version, remediation hint, …).
    pub detail: String,
}

impl DoctorResult {
    /// A passing check.
    pub fn pass(detail: impl Into<String>) -> Self {
        Self {
            status: DoctorStatus::Pass,
            detail: detail.into(),
        }
    }

    /// A concerning-but-not-fatal check.
    pub fn warn(detail: impl Into<String>) -> Self {
        Self {
            status: DoctorStatus::Warn,
            detail: detail.into(),
        }
    }

    /// A failing check (the engine cannot run).
    pub fn fail(detail: impl Into<String>) -> Self {
        Self {
            status: DoctorStatus::Fail,
            detail: detail.into(),
        }
    }
}

/// Severity of a [`DoctorResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DoctorStatus {
    /// The prerequisite is satisfied.
    Pass,
    /// Usable, but attention is advised.
    Warn,
    /// The engine cannot run in this environment.
    Fail,
}

/// One named environment check (native libraries, tool availability, …) surfaced
/// through `proef doctor` (ADR-0002 capability hook).
pub struct DoctorCheck {
    /// Short human-readable check name.
    pub name: &'static str,
    /// The check itself; must be cheap and side-effect free.
    pub run: fn() -> DoctorResult,
}

impl std::fmt::Debug for DoctorCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoctorCheck")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Per-scenario context handed to [`EngineFactory::open`]. Fields grow additively
/// as milestones land (artifact dirs, config, directives, …).
#[derive(Debug, Clone)]
pub struct ScenarioCtx {
    /// Injected run identifier.
    pub run_id: Arc<str>,
    /// Scenario name as authored.
    pub scenario: Arc<str>,
    /// The scenario's emitted artifact — for the hurl engine this *is* the
    /// executed input (ADR-0010: same bytes as the parse-validated emission).
    pub artifact: Option<ArtifactRef>,
    /// Secret name → value pairs referenced by this scenario. Engines inject
    /// them via their redacting mechanisms (`insert_secret`); values never
    /// enter events or artifacts (ADR-0005).
    pub secrets: Arc<std::collections::BTreeMap<String, String>>,
    /// Engine option defaults from project config (`timeout-ms`, …).
    pub http: HttpDefaults,
    /// Root directory for file bodies (`context_dir` confinement, §13) —
    /// the feature file's directory.
    pub file_root: Option<std::path::PathBuf>,
}

/// Batch-level HTTP defaults (per-entry `[Options]` in artifacts override them
/// — clone-then-override, verified TECH-SPEC §5).
#[derive(Debug, Clone, Copy)]
pub struct HttpDefaults {
    /// Per-request timeout in milliseconds (clamped default — ADR-0007).
    pub timeout_ms: u64,
    /// Follow redirects.
    pub follow_location: bool,
}

impl Default for HttpDefaults {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            follow_location: false,
        }
    }
}

/// A scenario's emitted artifact, shared with the engine that executes it.
#[derive(Debug, Clone)]
pub struct ArtifactRef {
    /// The artifact slug (`<slug>.hurl` — failure messages point here).
    pub slug: Arc<str>,
    /// The canonical `.hurl` text (the executed input).
    pub text: Arc<str>,
    /// The sidecar map: entry line ranges ↔ feature anchors ↔ batch indices.
    pub map: Arc<crate::emit::SidecarMap>,
}

/// Compiled-in engine entry point: identity, capability discovery, and session
/// opening (ADR-0002). Registered in `proef-cli`'s registry, one line per engine.
pub trait EngineFactory: Send + Sync {
    /// Stable engine id (`hurl`, …).
    fn id(&self) -> &'static str;

    /// The pack step kinds this engine claims, with their payload schemas.
    fn step_kinds(&self) -> &'static [StepKindSpec];

    /// Environment checks surfaced through `proef doctor`.
    fn doctor(&self) -> Vec<DoctorCheck>;

    /// Open a session for one scenario. Sessions are opened lazily on the first
    /// batch routed to this engine and torn down via [`EngineSession::finish`].
    fn open(&self, ctx: &ScenarioCtx) -> Result<Box<dyn EngineSession>, EngineError>;
}

/// A live per-scenario engine session (ADR-0002). Only a session runs batches —
/// lifecycle is enforced by this ownership shape, not typestate.
pub trait EngineSession: Send {
    /// Execute one batch of contiguous same-engine steps, threading captures
    /// through `world` and emitting progress on `events`. Engines *may* honor
    /// `cancel` at finer grain than batch boundaries when they can (ADR-0007).
    fn run_batch(
        &mut self,
        batch: &StepBatch,
        world: &mut World,
        events: &EventSink,
        cancel: &CancellationToken,
    ) -> BatchResult;

    /// The wall-clock budget for the *next* dispatch of `batch` (ADR-0007:
    /// Σ(entry timeout × (retries + 1)) + intervals + margin). `None` when the
    /// engine cannot estimate — the orchestrator falls back to its default.
    /// The watchdog abandons the scenario thread when the budget expires.
    fn batch_budget(&mut self, _batch: &StepBatch) -> Option<std::time::Duration> {
        None
    }

    /// Tear the session down (reverse open order; `Drop` is the backstop).
    fn finish(&mut self) -> Result<(), EngineError>;
}
