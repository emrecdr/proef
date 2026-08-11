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
    /// Asserts an `expect:` macro merged into the *previous* request entry
    /// (ADR-0004): the step owns the last `lines` assert lines appended to
    /// that entry's text. It renders no bytes of its own — the sidecar
    /// anchors those lines so results attribute to the authored `Then`.
    MergedAsserts {
        /// How many assert lines this step appended to the previous entry.
        lines: usize,
    },
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

/// A `when:` skip guard: the step runs unless the resolved expression is
/// empty or a literal false (TECH-SPEC §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guard(pub String);

impl Guard {
    /// Should the guarded step be skipped? Empty means "no condition met",
    /// and a literal `false`/`0` (case-insensitive) skips too — an author
    /// writing `when: ${flag}` with `flag=false` means *skip*, not "the
    /// string is non-empty, run anyway".
    pub fn skips(&self) -> bool {
        let value = self.0.trim();
        value.is_empty() || value.eq_ignore_ascii_case("false") || value == "0"
    }
}

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
    /// Skip guard, when configured (resolved text; the runtime skips the step
    /// per [`Guard::skips`]).
    pub when: Option<Guard>,
    /// Pack-step entry label (events/console), when authored.
    pub label: Option<String>,
    /// The fragment this step executes, qualified as `file.hurl#name`
    /// (ADR-0018); `None` for an inline `hurl:` block.
    ///
    /// Qualified rather than bare because this is what a *reader* of a run
    /// record needs: ADR-0018 accepted "a test spans three files" as a cost on
    /// the condition that tooling earn it back, and a bare `admin.search`
    /// still leaves you grepping for the file it lives in. The `file#name`
    /// spelling is the one `ref:` itself accepts, so what a record prints can
    /// be pasted straight back into a pack.
    pub fragment: Option<String>,
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
    /// Messages from earlier, failed attempts of a step that ultimately passed
    /// — the flaky-failure detail. Empty for a clean single-attempt step;
    /// engine-agnostic, so any engine with retries can fill it.
    pub attempt_details: Vec<String>,
    /// Engine-provided command to reproduce this step alone (engine-hurl fills
    /// it with the redacted `curl` of the failing request). Set only on failure;
    /// `None` otherwise and for engines that offer no hint.
    pub reproduce_hint: Option<String>,
    /// The fragment this step ran, copied from [`LoweredStep::fragment`]
    /// (ADR-0018); `None` for an inline block.
    ///
    /// Carried on the outcome as well as the event because the two feed
    /// different readers: the event stream reaches `explain`, while `JUnit` and
    /// the GitHub summary are built from [`crate::runner::RunSummary`]. CI is
    /// where a reader is *least* able to go looking, so it is the last place
    /// provenance should drop out.
    pub fragment: Option<String>,
}

/// Result of dispatching one [`StepBatch`] to an engine.
#[derive(Debug)]
pub struct BatchResult {
    /// One outcome per step the engine reached.
    pub steps: Vec<StepOutcome>,
    /// A batch-level failure, if the batch stopped early.
    pub error: Option<EngineError>,
}

#[cfg(test)]
mod tests {
    use super::Guard;

    /// `when:` semantics (TECH-SPEC §6): empty and literal-false skip; any
    /// other non-empty text runs.
    #[test]
    fn guard_skips_on_empty_and_literal_false() {
        for skipping in ["", "  ", "false", "FALSE", "0"] {
            assert!(Guard(skipping.to_owned()).skips(), "{skipping:?}");
        }
        for running in ["true", "yes", "1", "anything"] {
            assert!(!Guard(running.to_owned()).skips(), "{running:?}");
        }
    }
}
