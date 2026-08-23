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
//!             fragments: None,
//!             options: None,
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

/// An engine's claim on a pack step kind: the key prefix (`hurl`; other
/// prefixes reserved — ADR-0002 errata) plus the
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
    /// Fragment support, or `None` when the kind has no fragment form
    /// (ADR-0018). One `Option` rather than a separate extension and scanner,
    /// so the two can never disagree: a kind that claims `.http` files but
    /// cannot read them is not expressible.
    pub fragments: Option<FragmentSupport>,
    /// Recognise this kind's raw option keys, so the core can apply ADR-0007's
    /// budget rules without knowing how the engine spells them.
    pub options: Option<OptionRecogniser>,
}

/// An engine-contributed static payload validator (pack validation pass 7).
pub type PayloadValidator = fn(&str) -> Result<(), PayloadProbeError>;

/// How the core bounds one raw option's value (ADR-0007 budgets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawOptionValue {
    /// A repetition count: `-1` is infinite and anything over the cap is
    /// budget-hostile. hurl has no cancellation, so an unbounded count leaves
    /// the watchdog abandoning a thread it cannot stop.
    Count,
    /// A duration, capped so one entry cannot outlast a run.
    Duration,
}

/// What a raw option key means to the core's budget and double-declaration
/// rules — the engine's vocabulary translated into the core's policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawOption {
    /// The pack-visible family a step may *also* declare, when there is one.
    /// Always an element of [`OPTION_FAMILIES`]. `None` for an option with no
    /// YAML twin, which is therefore value-capped but cannot be declared twice.
    pub family: Option<&'static str>,
    /// How the value is bounded, or `None` when the core has no policy for it.
    pub value: Option<RawOptionValue>,
}

/// An engine-contributed recogniser: a raw option key (the text left of the
/// `:` in an `[Options]` line) → what the core's rules should make of it.
///
/// The seam that keeps option *spellings* out of `proef-core`. The fragment
/// half of this rule already crossed the seam — an engine maps its own AST to
/// [`ScannedFragment::declared_options`] — while the inline half matched
/// `"retry-interval:"` as a literal in core, so one rule lived at two
/// altitudes and a second engine would have got its fragments linted and its
/// inline blocks not.
///
/// `None` = the kind has no raw options the core bounds.
pub type OptionRecogniser = fn(&str) -> Option<RawOption>;

/// An engine-contributed reader for one fragment file's whole text (ADR-0018).
pub type FragmentScanner = fn(&str) -> Result<ScannedFile, FragmentScanError>;

/// One fragment file as its claiming engine read it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScannedFile {
    /// The entries carrying a `# @proef` annotation — the referenceable ones.
    pub fragments: Vec<ScannedFragment>,
    /// 1-based start lines of entries carrying **no** annotation.
    ///
    /// Lines only, and deliberately so. An unannotated entry is not a fragment
    /// — nothing can `ref:` it — and a corpus proef did not write is expected to
    /// be mostly those, so building a whole [`ScannedFragment`] for each would
    /// be the bulk of a scan for no one's benefit. A line number costs a push
    /// and is all a listing can point at, there being no name to print.
    ///
    /// Collected rather than dropped because "which entries did I forget to
    /// annotate?" is otherwise unanswerable: a missing annotation produces a
    /// green run and a silently absent test, and the entry that would prove it
    /// was never built.
    pub unannotated: Vec<usize>,
}

/// What a step kind needs to own a fragment file format: the extension that
/// identifies one and the parser that reads it. Source discovery asks the
/// registry for the extension rather than naming a file type itself, so adding
/// an engine never teaches the CLI a new one (ADR-0002).
#[derive(Debug, Clone, Copy)]
pub struct FragmentSupport {
    /// File extension without the dot (`"hurl"`).
    pub ext: &'static str,
    /// Reader for a whole file of that extension.
    pub scan: FragmentScanner,
    /// The **variable** names a single template value reads, as the engine's
    /// own parser answers it. This is the same question `scan` answers for a
    /// whole file, asked of one `bind:` value — and it must be the engine's
    /// answer because the engine's grammar decides what is a variable: hurl's
    /// `{{newUuid}}` is a *function call*, and a text-level scan that reported
    /// it as a variable made proef refuse input the engine itself runs
    /// (R17-2.2). Unparseable text reports no reads: the emitted artifact is
    /// parse-validated anyway, so a malformed template still fails loudly —
    /// there, where the engine's own error names it.
    pub template_reads: fn(&str) -> Vec<String>,
}

impl FragmentSupport {
    /// Does `name` carry the extension this kind claims?
    ///
    /// **The one place the question is answered.** It was answered in three:
    /// CLI discovery via `Path::extension`, the core scan via `rsplit('.')`, and
    /// the LSP's corpus invalidation case-insensitively — so they disagreed
    /// about `api.HURL` (the editor rebuilt its corpus for a file nothing would
    /// ever scan) and about a dotfile named `.hurl` (a stem, not an extension,
    /// which only the `rsplit` spelling accepted).
    ///
    /// Extension, not membership in a discovered set: a fragment file created
    /// while the editor is open is in no corpus yet, and it still has to
    /// invalidate the one being held.
    ///
    /// Path semantics, so `.hurl` is a stem and not an extension — the same
    /// answer a user gets from every other tool that classifies files.
    #[must_use]
    pub fn claims(&self, name: &str) -> bool {
        std::path::Path::new(name)
            .extension()
            .is_some_and(|ext| ext == self.ext)
    }
}

/// One entry of a fragment file, as the claiming engine's own parser sees it.
///
/// Engine-agnostic by construction: `proef-core` never learns a hurl type, and
/// a future engine fills the same shape from its own AST. Everything here is
/// *read* from the entry — nothing is declared separately and nothing can
/// therefore drift from the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFragment {
    /// The name its `# @proef <name>` annotation gave it.
    ///
    /// A scanner reports **only annotated entries**. An unannotated one is not a
    /// fragment — nothing can `ref:` it — and a corpus proef did not write is
    /// expected to be mostly those, so building them only to be discarded is
    /// the bulk of a scan for no one's benefit.
    pub name: String,
    /// The entry's own source text, annotation included (provenance survives
    /// into the artifact).
    pub text: String,
    /// 1-based line the entry starts on, for diagnostics.
    pub line: usize,
    /// Every variable the entry *reads*, in first-seen order — its required
    /// inputs.
    pub placeholders: Vec<String>,
    /// Option families the entry sets for itself, which a referencing step may
    /// then not also set (`proef::pack::option_declared_twice`). A list rather
    /// than one flag per option, so the core applies its general rule to
    /// whatever it knows about and a new family costs no engine change.
    ///
    /// **Every element must be one of [`OPTION_FAMILIES`]** — these strings are
    /// matched against the pack's own option keys, so a spelling only the engine
    /// knows silences the check rather than failing it.
    pub declared_options: Vec<String>,
    /// Every variable the entry *supplies to itself*, in first-seen order — the
    /// engine equivalent of a `bind:`, written into the fragment file.
    ///
    /// Kept apart from [`Self::declared_options`] because the two clash on
    /// different keys: an option family is a closed vocabulary compared
    /// family-to-family, while a supplied variable is an open set compared
    /// *name to name* — `token` clashes with a `bind:` of `token` and with
    /// nothing else. Folding them together would make [`OPTION_FAMILIES`]'
    /// "every element is one of these" invariant unstatable.
    ///
    /// Both halves of this field are load-bearing. A name here **satisfies** a
    /// placeholder of the same name (the fragment answers its own question, so
    /// the file still runs standalone under the engine's own binary — ADR-0018's
    /// premise), and it **collides** with a `bind:` of that name
    /// (`proef::pack::option_declared_twice`), because the engine may resolve
    /// the pair silently rather than refusing it.
    pub supplied_variables: Vec<String>,
}

/// The option families a pack step and a fragment can *both* declare, spelled as
/// the pack spells them.
///
/// This is the vocabulary [`ScannedFragment::declared_options`] must use: the
/// double-declaration rule works by string equality against the keys a step
/// writes in YAML, so an engine reporting hurl's own spelling (`retry-interval`,
/// say) would match nothing and the clash would go quiet — which is exactly the
/// silent last-wins `proef::pack::option_declared_twice` exists to refuse.
/// Engines fold their spellings into these (hurl's `retry-interval` is `retry`:
/// one policy, and a step's `retry:` sets both).
///
/// Kept in step with `MacroStep::declared_options`, which derives the other half
/// of the same comparison.
pub const OPTION_FAMILIES: &[&str] = &["retry", "delay"];

/// A fragment file the claiming engine's parser could not read (1-based
/// line/column **within that file**).
///
/// Distinct from [`PayloadProbeError`] despite the same shape: that one is
/// positioned inside a pack's payload block and gets mapped onto the pack
/// file, this one already points at a real file of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentScanError {
    /// 1-based line within the fragment file.
    pub line: usize,
    /// 1-based column within that line.
    pub column: usize,
    /// Parser message.
    pub message: String,
}

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
/// as milestones land (artifact dirs, config, …).
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
    /// Engine variable name → secret name for this scenario. Do not join this
    /// against `secrets` by hand — call [`secret_variables`], the one place
    /// that knows how (ADR-0018).
    pub secret_bindings: Arc<std::collections::BTreeMap<String, String>>,
    /// Engine option defaults from project config (`timeout-ms`, …).
    pub http: HttpDefaults,
    /// Root directory for file bodies (`context_dir` confinement, §13) —
    /// the feature file's directory.
    pub file_root: Option<std::path::PathBuf>,
}

/// Every engine variable a scenario must inject as a secret, paired with its
/// value — [`ScenarioCtx::secret_bindings`] joined against
/// [`ScenarioCtx::secrets`]. **The one place that join is written.**
///
/// It lives in core, not in each engine, because it is easy to get subtly
/// wrong: inject under the *secret* name rather than the *variable* name and a
/// renamed binding (ADR-0018) resolves to nothing, so the request goes out with
/// an unresolved `{{…}}` and fails far from the cause.
///
/// It yields borrows on purpose. Returning an owned variable→value map would put
/// a second copy of every secret value in memory for each scenario; ADR-0005
/// keeps values in exactly one place, the run-level `secrets` map.
///
/// A binding whose secret is absent is skipped — the CLI already refuses a run
/// whose secrets it cannot resolve, so that is defence in depth, not a path.
pub fn secret_variables<'a>(
    bindings: &'a std::collections::BTreeMap<String, String>,
    secrets: &'a std::collections::BTreeMap<String, String>,
) -> impl Iterator<Item = (&'a str, &'a str)> {
    bindings.iter().filter_map(|(variable, secret)| {
        secrets
            .get(secret)
            .map(|value| (variable.as_str(), value.as_str()))
    })
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
