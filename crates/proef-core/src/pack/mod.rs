//! Macro packs: the YAML binding skeleton with embedded raw payload blocks
//! (ADR-0004, TECH-SPEC §6).
//!
//! Packs are parsed with `serde_norway` (`deny_unknown_fields` on the fixed
//! schema; the *payload* key of a step — `hurl:`, or a future engine's kind — is
//! dynamic and checked against the registered engines' [`StepKindSpec`]s in
//! validation pass 8). Loading is pure: the CLI discovers files and hands
//! [`PackSource`]s in; built-in packs are embedded at build time.

pub(crate) mod locate;
mod schema;
mod validate;

pub use schema::json_schema;

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;

use crate::diag::{Diag, FrontError, Span};
use crate::engine::StepKindSpec;
use crate::step::Retry;

/// One pack input: a name (path as authored, or `builtin:…`) plus its text.
#[derive(Debug, Clone)]
pub struct PackSource {
    /// Display name (file path as authored, or `builtin:<name>`).
    pub name: String,
    /// The raw YAML text.
    pub text: Arc<str>,
}

/// The built-in packs embedded into every proef binary.
pub fn builtin_sources() -> Vec<PackSource> {
    vec![PackSource {
        name: "builtin:core.yaml".to_owned(),
        text: Arc::from(include_str!("../../helpers/core.yaml")),
    }]
}

// ---------------------------------------------------------------------------
// Raw serde model (wire shape — TECH-SPEC §6)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPack {
    pub(crate) macros: BTreeMap<String, RawMacro>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawMacro {
    #[serde(default)]
    pub(crate) params: Vec<String>,
    #[serde(default)]
    pub(crate) defaults: BTreeMap<String, String>,
    #[serde(rename = "match")]
    pub(crate) match_: Option<String>,
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    #[serde(default)]
    pub(crate) steps: Vec<RawStep>,
    pub(crate) expect: Option<Vec<RawExpectItem>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct RawStep {
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) optional: bool,
    pub(crate) when: Option<String>,
    pub(crate) retry: Option<RawRetry>,
    /// Delay before the request, in milliseconds (baked into `[Options]`).
    pub(crate) delay: Option<u64>,
    #[serde(rename = "saveAs")]
    pub(crate) save_as: Option<BTreeMap<String, String>>,
    #[serde(rename = "use")]
    pub(crate) use_: Option<String>,
    pub(crate) with: Option<BTreeMap<String, String>>,
    /// The dynamic payload key (`hurl:`, or a future engine's kind) — validated
    /// against registered engine step kinds in pass 8.
    #[serde(flatten)]
    #[schemars(with = "BTreeMap<String, serde_json::Value>")]
    pub(crate) payload: BTreeMap<String, serde_norway::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawRetry {
    pub(crate) count: u32,
    #[serde(default = "default_retry_interval")]
    pub(crate) interval_ms: u64,
}

fn default_retry_interval() -> u64 {
    1000
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawExpectItem {
    pub(crate) status: Option<String>,
    /// Raw hurl assert lines appended to the previous entry's `[Asserts]`.
    pub(crate) hurl: Option<String>,
}

// ---------------------------------------------------------------------------
// Loaded model (what binding and lowering consume)
// ---------------------------------------------------------------------------

/// A validated set of packs: every macro, indexed by (globally unique) name.
#[derive(Debug, Default)]
pub struct PackSet {
    /// All macros by name (pass 3 guarantees global uniqueness).
    pub macros: BTreeMap<String, Macro>,
}

impl PackSet {
    /// `(pattern, macro name)` pairs for the step binder — macros with a
    /// `match:` only.
    pub fn step_defs(&self) -> Vec<(&str, &str)> {
        self.macros
            .values()
            .filter_map(|m| m.pattern.as_deref().map(|p| (p, m.name.as_str())))
            .collect()
    }

    /// Resolve a `use:` target (`name` or `pack.yaml#name`) to a macro.
    pub fn find_use_target(&self, target: &str) -> Option<&Macro> {
        match target.split_once('#') {
            Some((pack_ref, name)) => self
                .macros
                .get(name)
                .filter(|m| pack_ref_matches(&m.pack, pack_ref)),
            None => self.macros.get(target),
        }
    }
}

/// Path-boundary-aware pack-qualifier match: `api.yaml` qualifies
/// `packs/api.yaml` but never `legacy-api.yaml` — a suffix only counts when
/// it starts at a `/` boundary (or spans the whole name).
fn pack_ref_matches(pack: &str, pack_ref: &str) -> bool {
    let bounded_suffix = |hay: &str, needle: &str| {
        hay.strip_suffix(needle)
            .is_some_and(|rest| rest.is_empty() || rest.ends_with('/'))
    };
    bounded_suffix(pack, pack_ref) || bounded_suffix(pack_ref, pack)
}

/// One loaded macro.
#[derive(Debug, Clone)]
pub struct Macro {
    /// Macro name (globally unique across loaded packs).
    pub name: String,
    /// Source pack name this macro came from.
    pub pack: String,
    /// Declared params (required unless defaulted).
    pub params: Vec<String>,
    /// Default values for optional params.
    pub defaults: BTreeMap<String, String>,
    /// The Gherkin-reachable `match:` pattern (absent = `use:`-only macro).
    pub pattern: Option<String>,
    /// Documentation string.
    pub description: Option<String>,
    /// Macro tags.
    pub tags: Vec<String>,
    /// Request steps or assert-only body.
    pub body: MacroBody,
    /// The pack source text (for diagnostics).
    pub source: Arc<str>,
    /// Span of the macro's name in the pack file, when locatable.
    pub span: Option<Span>,
    /// Span of the macro's `match:` line in the pack file, when locatable.
    pub match_span: Option<Span>,
}

/// A macro is either a sequence of request steps or an assert-only `expect:`
/// (merged into the previous request entry — the Then-step rule, ADR-0004).
#[derive(Debug, Clone)]
pub enum MacroBody {
    /// Request steps.
    Steps(Vec<MacroStep>),
    /// Assert-only items.
    Expect(Vec<ExpectItem>),
}

/// One step of a request macro.
#[derive(Debug, Clone)]
pub struct MacroStep {
    /// Entry label (events/console).
    pub name: Option<String>,
    /// Delay before the request in milliseconds (baked into `[Options]`).
    pub delay_ms: Option<u64>,
    /// Payload or composition.
    pub kind: MacroStepKind,
    /// `optional:` — failure warns and the batch segments around it.
    pub optional: bool,
    /// `when:` skip guard (runs iff non-empty after resolution).
    pub when: Option<String>,
    /// Finite retry policy.
    pub retry: Option<Retry>,
    /// `saveAs:` promotions (capture name → `global`).
    pub save_as: BTreeMap<String, String>,
}

/// Payload or composition of a [`MacroStep`].
#[derive(Debug, Clone)]
pub enum MacroStepKind {
    /// An engine payload (`hurl: |` raw block, or structured for future engines).
    Payload {
        /// The step kind key as written (`hurl`, …).
        kind: String,
        /// The payload itself.
        payload: PayloadForm,
    },
    /// Composition: inline another macro's steps.
    Use {
        /// Target macro (`name` or `pack.yaml#name`).
        target: String,
        /// Arguments for the target's params.
        with: BTreeMap<String, String>,
    },
}

/// The two payload shapes (ADR-0004: raw text is primary; structured is
/// reserved for future non-hurl engines).
#[derive(Debug, Clone)]
pub enum PayloadForm {
    /// Raw engine text (`hurl:` block scalar), `${…}` still unresolved.
    Raw(String),
    /// Structured payload for future engines.
    Structured(serde_json::Value),
}

/// One assert-only item: a `status:` shorthand and/or raw hurl assert lines
/// (both may contain `${…}`).
#[derive(Debug, Clone)]
pub struct ExpectItem {
    /// Expected HTTP status.
    pub status: Option<String>,
    /// Raw assert lines appended to the previous entry's `[Asserts]`.
    pub fragment: Option<String>,
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Parse and validate `sources` against the registered engine step `kinds`
/// (validation passes 1–8, TECH-SPEC §4.1), returning the partial [`PackSet`]
/// built from every pack that parses+normalizes AND all diagnostics collected
/// along the way. A pack that fails to parse contributes only its diagnostic
/// and is excluded from the set — it never sinks its siblings. This is the
/// collect-all half that the LSP's `analyze_suite` needs so one broken pack
/// does not zero the whole suite; `load` is the fail-fast wrapper for a run.
pub(crate) fn load_collecting(
    sources: &[PackSource],
    kinds: &[StepKindSpec],
) -> (PackSet, Vec<Diag>) {
    let mut diags: Vec<Diag> = Vec::new();
    let mut set = PackSet::default();
    let mut raw_packs: Vec<(usize, String, RawPack)> = Vec::new();

    for (index, source) in sources.iter().enumerate() {
        match serde_norway::from_str::<RawPack>(&source.text) {
            Ok(raw) => raw_packs.push((index, source.name.clone(), raw)),
            Err(err) => {
                let span = err
                    .location()
                    .map(|loc| Span::clamped(loc.index(), loc.index() + 1, source.text.len()));
                let mut diag = Diag::error(
                    "proef::pack::yaml",
                    format!("pack is not valid YAML for the pack schema: {err}"),
                )
                .with_source(source.name.clone(), Arc::clone(&source.text));
                if let Some(span) = span {
                    diag = diag.with_span(span);
                }
                diags.push(diag);
            }
        }
    }

    // Normalize each raw macro (structural checks happen inline).
    for (source_index, pack_name, raw) in &raw_packs {
        let source = &sources[*source_index];
        for (macro_name, raw_macro) in &raw.macros {
            let normalized =
                validate::normalize_macro(macro_name, raw_macro, pack_name, source, &mut diags);
            if let Some(macro_) = normalized {
                // Pass 3: duplicate macro names across packs.
                if let Some(existing) = set.macros.get(macro_name) {
                    diags.push(
                        Diag::error(
                            "proef::pack::duplicate_macro",
                            format!(
                                "macro `{macro_name}` is defined in both `{}` and `{pack_name}`",
                                existing.pack
                            ),
                        )
                        .with_source(source.name.clone(), Arc::clone(&source.text))
                        .maybe_span(macro_.span)
                        .with_help("macro names are global — rename one of the definitions"),
                    );
                } else {
                    set.macros.insert(macro_name.clone(), macro_);
                }
            }
        }
    }

    validate::run_cross_macro_passes(&set, kinds, &mut diags);
    (set, diags)
}

/// Parse and validate `sources`, failing on the first error-severity diagnostic
/// (the fail-fast contract a real `proef` run depends on). All diagnostics are
/// still collected — one bad pack does not hide problems in another.
pub fn load(sources: &[PackSource], kinds: &[StepKindSpec]) -> Result<PackSet, FrontError> {
    let (set, diags) = load_collecting(sources, kinds);
    if diags
        .iter()
        .any(|d| d.severity == crate::diag::Severity::Error)
    {
        Err(FrontError::Diagnostics(diags))
    } else {
        Ok(set)
    }
}

impl Diag {
    /// Attach a span when one is available (loader convenience).
    #[must_use]
    pub(crate) fn maybe_span(self, span: Option<Span>) -> Self {
        match span {
            Some(span) => self.with_span(span),
            None => self,
        }
    }
}
