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

/// Every fragment file's text, scanned **at most once** however many times the
/// packs around it are loaded (ADR-0018).
///
/// One `proef test` loads packs up to four times — the suite, then `[run] setup`
/// and `[run] teardown`, each validated and then run — against different feature
/// paths but always the *same* corpus. Rescanning per load measured ~75% of a
/// run's total work on a 200-file corpus, and it grows with the corpus, which is
/// the direction adoption goes.
///
/// The memo lives here rather than in the caller because the scan must stay
/// **lazy**: `load_collecting` runs it only when some pack actually has a
/// `ref:`, which is what makes CONFIG.md's "pointing at a corpus you did not
/// write costs nothing" true of the scan. A caller that scanned eagerly in
/// order to share the result would buy speed by breaking that promise. Nothing
/// here reads a file — the texts arrive already read, so core stays sans-IO.
#[derive(Debug)]
pub struct FragmentCorpus {
    sources: Vec<PackSource>,
    /// Captured at construction so the memo cannot be filled under one set of
    /// kinds and then read under another.
    kinds: Vec<StepKindSpec>,
    /// Files the caller could not read, already shaped as diagnostics.
    ///
    /// Held rather than raised at read time because a corpus is *foreign by
    /// design*: one unreadable file — a binary, a latin-1 export — must not
    /// take down commands that never look at fragments at all. They surface
    /// through the same gate as scan diagnostics, so a suite with no `ref:`
    /// stays silent and "pointing at a corpus you did not write costs nothing"
    /// keeps meaning what it says.
    read_errors: Vec<Diag>,
    scanned: std::sync::OnceLock<Scanned>,
}

/// One scan's product: the named fragments, plus what was wrong with the files.
#[derive(Debug, Default)]
pub(crate) struct Scanned {
    pub(crate) fragments: Arc<BTreeMap<String, Fragment>>,
    /// Per file, the 1-based lines of entries carrying no annotation. Keyed by
    /// source name so a listing can group them under the file they belong to;
    /// a file with none contributes no entry.
    pub(crate) unannotated: BTreeMap<String, Vec<usize>>,
    pub(crate) diags: Vec<Diag>,
}

impl FragmentCorpus {
    /// A corpus over already-read file texts.
    pub fn new(sources: Vec<PackSource>, kinds: &[StepKindSpec]) -> Self {
        Self {
            sources,
            kinds: kinds.to_vec(),
            read_errors: Vec::new(),
            scanned: std::sync::OnceLock::new(),
        }
    }

    /// Record files the caller could not read. They are reported like any other
    /// per-file corpus problem — never sinking their siblings, and never at all
    /// unless something `ref:`s the corpus.
    #[must_use]
    pub fn with_read_errors(mut self, errors: Vec<Diag>) -> Self {
        self.read_errors = errors;
        self
    }

    /// The empty corpus — no `[run] fragments` configured, so no `ref:` can
    /// resolve and nothing is ever scanned.
    pub fn empty() -> Self {
        Self::new(Vec::new(), &[])
    }

    /// The scan, run on first use and shared by every load thereafter.
    pub(crate) fn scanned(&self) -> &Scanned {
        self.scanned.get_or_init(|| {
            let mut scanned = scan_fragments(&self.sources, &self.kinds);
            // Unreadable files first: they explain an `unknown_ref` that would
            // otherwise read as a typo.
            let mut diags = self.read_errors.clone();
            diags.append(&mut scanned.diags);
            scanned.diags = diags;
            scanned
        })
    }

    /// Every annotated fragment in the corpus, keyed by name. Scans on first
    /// use, like every other reader.
    ///
    /// Public because the scan is otherwise gated: [`load`] parses the corpus
    /// only when some pack actually names a fragment, so `PackSet::fragments`
    /// is empty for a suite that references none — which is exactly the suite a
    /// listing has the most to say about.
    pub fn fragments(&self) -> &BTreeMap<String, Fragment> {
        &self.scanned().fragments
    }

    /// Per file, the 1-based lines of entries carrying no `# @proef`
    /// annotation — the one class of corpus content nothing else can report,
    /// since an unannotated entry has no name to be listed by.
    pub fn unannotated(&self) -> &BTreeMap<String, Vec<usize>> {
        &self.scanned().unannotated
    }

    /// Whatever was wrong with the corpus: unreadable files first, then scan
    /// failures. Already shaped as diagnostics.
    pub fn diagnostics(&self) -> &[Diag] {
        &self.scanned().diags
    }
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
    /// Pack-scope fragment bindings (ADR-0018): the plumbing every macro in the
    /// file needs, written once. Macro and step scope override it.
    #[serde(default)]
    pub(crate) bind: BTreeMap<String, String>,
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
    /// Macro-scope fragment bindings (ADR-0018).
    #[serde(default)]
    pub(crate) bind: BTreeMap<String, String>,
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
    /// A named fragment this step executes (ADR-0018) — the alternative to an
    /// inline payload, never both on one step.
    #[serde(rename = "ref")]
    pub(crate) ref_: Option<String>,
    /// Step-scope fragment bindings, the most specific of the three.
    #[serde(default)]
    pub(crate) bind: BTreeMap<String, String>,
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

/// A validated set of packs: every macro, indexed by (globally unique) name,
/// plus the fragments packs may reference and the pack-scope bindings.
#[derive(Debug, Default)]
pub struct PackSet {
    /// All macros by name (pass 3 guarantees global uniqueness).
    pub macros: BTreeMap<String, Macro>,
    /// All named fragments by name — globally unique for the same reason macro
    /// names are: a `ref:` names one thing, wherever it was declared (ADR-0018).
    ///
    /// Shared rather than owned: one scan serves every load of the same corpus
    /// (see [`FragmentCorpus`]), so handing it to four loads costs four
    /// refcounts, not four copies of the corpus.
    pub fragments: Arc<BTreeMap<String, Fragment>>,
    /// Pack-scope `bind:` tables, keyed by pack name so a binding stays
    /// attributable to the file that declared it.
    pub bind: BTreeMap<String, BTreeMap<String, String>>,
}

impl Fragment {
    /// This fragment as `file.hurl#name` — the spelling a `ref:` accepts, and
    /// what a run record carries so it can be read back long after the pack
    /// that named it changed.
    ///
    /// Next to [`PackSet::find_fragment`], which parses the same form via
    /// `pack_ref_matches`: producing it 400 lines from where it is consumed is
    /// how the two stop agreeing on what the separator means.
    #[must_use]
    pub fn qualified(&self) -> String {
        format!("{}#{}", self.file, self.name)
    }
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

    /// Resolve a `ref:` target (`name` or `file.hurl#name`) to a fragment —
    /// the same two spellings `use:` accepts, qualified the same way, because
    /// they answer the same question.
    pub fn find_fragment(&self, target: &str) -> Option<&Fragment> {
        match target.split_once('#') {
            Some((file_ref, name)) => self
                .fragments
                .get(name)
                .filter(|f| pack_ref_matches(&f.file, file_ref)),
            None => self.fragments.get(target),
        }
    }
}

/// One named entry of a fragment file (ADR-0018).
///
/// Every field but `name` is *read* from the entry by the claiming engine's own
/// parser — nothing is declared twice, so nothing can drift from the file.
#[derive(Debug, Clone)]
pub struct Fragment {
    /// The name its `# @proef` annotation gave it (globally unique).
    pub name: String,
    /// Source file name as authored — the `file.hurl#name` qualifier and the
    /// diagnostic source.
    pub file: String,
    /// The step kind whose engine scanned this file, taken from the
    /// `StepKindSpec` whose `fragments.ext` claimed it. A `ref:` step routes by
    /// this exactly as an inline step routes by its payload key (ADR-0002).
    pub kind: String,
    /// The entry's own text, annotation included.
    pub text: String,
    /// 1-based line the entry starts on.
    pub line: usize,
    /// Variables the entry reads: its required inputs.
    pub placeholders: Vec<String>,
    /// Option families the entry sets for itself (`"retry"`, `"delay"`).
    pub declared_options: Vec<String>,
    /// Variables the entry supplies to itself (`[Options] variable:`): both an
    /// answer to its own placeholders and a clash with a `bind:` of that name.
    pub supplied_variables: Vec<String>,
    /// The fragment file's text (for diagnostics).
    pub source: Arc<str>,
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
    /// Macro-scope fragment bindings (ADR-0018), overriding pack scope.
    pub bind: BTreeMap<String, String>,
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
    /// Step-scope fragment bindings (ADR-0018), the most specific of the three.
    pub bind: BTreeMap<String, String>,
}

impl MacroStep {
    /// The [`crate::engine::OPTION_FAMILIES`] this step sets for itself.
    ///
    /// The single derivation of "which options does the YAML declare", so the
    /// double-declaration rule reads the same answer for both body forms —
    /// an inline block's `[Options]` and a fragment's `declared_options` are
    /// checked against *this*, not against two hand-written lists that could
    /// drift apart and let hurl's silent last-wins back in.
    ///
    /// A new family is added here and in `OPTION_FAMILIES` together; both
    /// call sites then cover it with no further edit.
    pub fn declared_options(&self) -> impl Iterator<Item = &'static str> {
        [
            ("retry", self.retry.is_some()),
            ("delay", self.delay_ms.is_some()),
        ]
        .into_iter()
        .filter_map(|(family, declared)| declared.then_some(family))
    }
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
    /// A named fragment declared in an engine-native file (ADR-0018).
    Ref {
        /// Target fragment (`name` or `file.hurl#name`).
        target: String,
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
/// (validation passes 1–13, TECH-SPEC §4.1), returning the partial [`PackSet`]
/// built from every pack that parses+normalizes AND all diagnostics collected
/// along the way. A pack that fails to parse contributes only its diagnostic
/// and is excluded from the set — it never sinks its siblings. This is the
/// collect-all half that the LSP's `analyze_suite` needs so one broken pack
/// does not zero the whole suite; `load` is the fail-fast wrapper for a run.
pub(crate) fn load_collecting(
    sources: &[PackSource],
    fragments: &FragmentCorpus,
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

    // Fragments parse only when some pack actually names one. hurl-parsing a
    // corpus is the dominant cost of loading it and the root may be large and
    // external, so a suite that references none must not pay it on every
    // `test`, `dry-run`, `flows` and `macros`.
    //
    // This skips the *parse*, not the read: `sources` and `fragments` both
    // arrive already read, because core performs no IO (the caller does, and
    // hands the bytes in). So "pointing at a corpus you did not write costs
    // nothing" (CONFIG.md) is exact about the scan and approximate about the
    // file read — do not restate it here as though the whole cost were gated.
    if raw_packs.iter().any(|(_, _, raw)| {
        raw.macros
            .values()
            .any(|m| m.steps.iter().any(|s| s.ref_.is_some()))
    }) {
        let scanned = fragments.scanned();
        set.fragments = Arc::clone(&scanned.fragments);
        diags.extend(scanned.diags.iter().cloned());
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
        if !raw.bind.is_empty() {
            set.bind.insert(pack_name.clone(), raw.bind.clone());
        }
    }

    validate::run_cross_macro_passes(&set, kinds, &mut diags);
    (set, diags)
}

/// Scan every fragment file through the claiming engine's parser, indexing the
/// annotated entries by name. Unannotated entries are dropped without comment:
/// a corpus proef did not write is mostly those, and naming is the author's
/// way of saying which ones proef may use.
///
/// A file the engine cannot parse contributes its diagnostic and nothing else —
/// the same "never sinks its siblings" rule packs get.
fn scan_fragments(sources: &[PackSource], kinds: &[StepKindSpec]) -> Scanned {
    let mut fragments: BTreeMap<String, Fragment> = BTreeMap::new();
    let mut unannotated: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut diags: Vec<Diag> = Vec::new();
    for source in sources {
        // The extension decides which kind claims the file. A file no kind
        // claims is skipped rather than handed to whichever scanner happens to
        // be first: that guess would blame one engine's parser for another
        // engine's file, and route the fragment to the wrong engine at run time.
        let Some((kind_name, scan)) = kinds.iter().find_map(|kind| {
            let support = kind.fragments?;
            (source.name.rsplit('.').next() == Some(support.ext))
                .then_some((kind.prefix, support.scan))
        }) else {
            continue;
        };
        let scanned = match scan(&source.text) {
            Ok(scanned) => scanned,
            Err(err) => {
                diags.push(
                    Diag::error(
                        "proef::pack::bad_annotation",
                        format!("{}: {}", source.name, err.message),
                    )
                    .with_source(source.name.clone(), Arc::clone(&source.text))
                    .maybe_span(locate::line_span(&source.text, err.line)),
                );
                continue;
            }
        };
        if !scanned.unannotated.is_empty() {
            unannotated.insert(source.name.clone(), scanned.unannotated);
        }
        for entry in scanned.fragments {
            let name = entry.name;
            if let Some(existing) = fragments.get(&name) {
                // Two branches, because the cross-file remedy is wrong for a
                // same-file collision: `file.hurl#name` qualifies by *file*, so
                // it cannot separate two entries inside one. Annotating a corpus
                // adds many names to few files, which makes same-file the likely
                // collision — and "declared in both `x` and `x`" reads as a bug
                // in proef rather than a duplicate in the corpus.
                let (message, help) = if existing.file == source.name {
                    (
                        format!(
                            "fragment `{name}` is declared twice in `{}` (first at line {})",
                            source.name, existing.line
                        ),
                        "fragment names are global — rename one of the two annotations",
                    )
                } else {
                    (
                        format!(
                            "fragment `{name}` is declared in both `{}` and `{}`",
                            existing.file, source.name
                        ),
                        "fragment names are global — rename one, or qualify the `ref:` \
                         as `file.hurl#name`",
                    )
                };
                diags.push(
                    Diag::error("proef::pack::duplicate_fragment", message)
                        .with_source(source.name.clone(), Arc::clone(&source.text))
                        .maybe_span(locate::line_span(&source.text, entry.line))
                        .with_help(help),
                );
                continue;
            }
            fragments.insert(
                name.clone(),
                Fragment {
                    name,
                    file: source.name.clone(),
                    kind: kind_name.to_owned(),
                    text: entry.text,
                    line: entry.line,
                    placeholders: entry.placeholders,
                    declared_options: entry.declared_options,
                    supplied_variables: entry.supplied_variables,
                    source: Arc::clone(&source.text),
                },
            );
        }
    }
    Scanned {
        fragments: Arc::new(fragments),
        unannotated,
        diags,
    }
}

/// Parse and validate `sources`, failing on the first error-severity diagnostic
/// (the fail-fast contract a real `proef` run depends on). All diagnostics are
/// still collected — one bad pack does not hide problems in another.
pub fn load(
    sources: &[PackSource],
    fragments: &FragmentCorpus,
    kinds: &[StepKindSpec],
) -> Result<PackSet, FrontError> {
    let (set, diags) = load_collecting(sources, fragments, kinds);
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
