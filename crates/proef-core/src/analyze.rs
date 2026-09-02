//! Collect-all suite analysis: the LSP's whole-suite recompute in one function.
//!
//! Where `front::run` fails fast and emits artifacts, `analyze_suite`
//! accumulates every diagnostic and emits the relations editors need
//! (`bindings` for go-to-def/references, `macros` for completion/def targets).
//! A parse-failed unit reports its own diagnostic and is skipped downstream —
//! no cascade of bogus follow-on errors.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::bind;
use crate::diag::{Diag, Span};
use crate::emit;
use crate::engine::StepKindSpec;
use crate::feature;
use crate::lower::{self, LowerCtx};
use crate::pack::{self, MacroBody, MacroStepKind, PackSet, PackSource};
use crate::provider::SourceProvider;
use crate::world::{GlobalStore, World};

/// One prose step bound to a macro — powers go-to-definition and references.
#[derive(Debug, Clone)]
pub struct Binding {
    /// Source name of the feature file the step lives in.
    pub feature: String,
    /// Byte span of the step text in the *normalized* feature source.
    pub step_span: Span,
    /// The macro this step resolved to.
    pub macro_name: String,
}

/// A macro definition — powers completion and is the go-to-definition target.
#[derive(Debug, Clone)]
pub struct MacroRef {
    /// Macro name (globally unique across the loaded packs).
    pub name: String,
    /// The `match:` pattern (`None` for `use:`-only macros).
    pub pattern: Option<String>,
    /// Declared params, in declaration order.
    pub params: Vec<String>,
    /// Source name of the pack the macro is defined in.
    pub pack: String,
    /// Byte span of the macro's name key in the *normalized* pack source, when
    /// locatable. This is the definition anchor go-to-definition jumps to.
    pub def_span: Option<Span>,
    /// Byte span of the macro's `match:` line, when locatable — the preferred
    /// go-to-definition landing anchor (falls back to `def_span`).
    pub match_span: Option<Span>,
}

/// One `use:` reference inside a pack → the macro it resolves to. Powers
/// go-to-definition from a `use:` line to the target macro's definition.
#[derive(Debug, Clone)]
pub struct UseRef {
    /// Source name of the pack the `use:` line lives in.
    pub pack: String,
    /// Byte span of the `use:` line in the *normalized* pack source.
    pub span: Span,
    /// The macro the reference resolves to (globally unique name).
    pub target_macro: String,
}

/// One `ref:` reference inside a pack → the fragment it resolves to. Powers
/// go-to-definition from a `ref:` line to the annotation in the `.hurl` file.
#[derive(Debug, Clone)]
pub struct FragmentRef {
    /// Source name of the pack the `ref:` line lives in.
    pub pack: String,
    /// Byte span of the `ref:` line in the *normalized* pack source.
    pub span: Span,
    /// The fragment the reference resolves to (globally unique name).
    pub target_fragment: String,
}

/// A fragment definition — the go-to-definition target for a `ref:`, and the
/// vocabulary a `ref:` line completes against.
#[derive(Debug, Clone)]
pub struct FragmentDef {
    /// Fragment name (globally unique across the scanned files).
    pub name: String,
    /// Source name of the file declaring it.
    pub file: String,
    /// Byte span of its `# @proef` annotation line, when locatable — the
    /// landing anchor.
    pub span: Option<Span>,
    /// The exact text `span` was measured against. Carried rather than re-read:
    /// a consumer converting the span needs a line index built from *these*
    /// bytes, and a fresh read could observe a newer edit and mis-anchor.
    pub source: Arc<str>,
    /// Every variable the entry reads, in first-seen order — exactly the names a
    /// `bind:` in scope has to supply (ADR-0018).
    ///
    /// Read off the engine's own AST at scan time, so an editor offering them is
    /// offering the file's real interface rather than a second description of it
    /// that could disagree. Without this the only way to learn a foreign
    /// corpus's variable names is to run the suite and read
    /// `proef::lower::unbound_placeholder`.
    ///
    /// Faithful to what the entry *reads*, so subtract [`Self::supplied_variables`]
    /// before offering these as `bind:` keys.
    pub placeholders: Vec<String>,
    /// Every variable the entry supplies to itself (`[Options] variable:`).
    ///
    /// A name here needs no `bind:` and may not have one
    /// (`proef::pack::option_declared_twice`), so an editor offering it as a
    /// completion would be proposing an edit its own diagnostics then reject.
    pub supplied_variables: Vec<String>,
}

/// The product of one wholesale recompute: every feature's read from here.
#[derive(Debug, Default)]
pub struct SuiteAnalysis {
    /// source name → its diagnostics (features and packs alike).
    pub diagnostics: BTreeMap<String, Vec<Diag>>,
    /// Every prose-step-to-macro binding across the suite.
    pub bindings: Vec<Binding>,
    /// Every macro definition across the loaded packs.
    pub macros: Vec<MacroRef>,
    /// Every `use:` reference across the loaded packs, resolved to its target.
    pub use_refs: Vec<UseRef>,
    /// Every `ref:` reference across the loaded packs, resolved to its target.
    pub fragment_refs: Vec<FragmentRef>,
    /// Every fragment definition across the scanned files.
    pub fragments: Vec<FragmentDef>,
    /// Every scenario across the discovered features, in authored order.
    pub scenarios: Vec<ScenarioRef>,
}

/// One scenario, as an editor needs to list it: what it is called, where it
/// starts, and what it is tagged.
///
/// Taken from the *parse*, not from binding, so a scenario whose steps do not
/// resolve still appears — an outline that hides exactly the scenarios you are
/// debugging would be worse than no outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioRef {
    /// Source name of the feature the scenario is defined in.
    pub feature: String,
    /// Scenario name, with outline placeholders already substituted.
    pub name: String,
    /// 1-based line of the scenario header.
    pub line: usize,
    /// Accumulated tags (feature + rule + scenario + examples), without `@`.
    pub tags: Vec<String>,
}

/// Everything `analyze_suite` needs, injected at the IO edge (sans-IO core).
pub struct AnalyzeCtx<'a> {
    /// The source of feature and pack bytes (the IO edge lives behind it).
    pub provider: &'a dyn SourceProvider,
    /// Registered engine step kinds (drives pack validation and artifact probes).
    pub kinds: &'a [StepKindSpec],
    /// Step-kind prefix → engine id, the lowering routing table.
    pub kind_to_engine: &'a BTreeMap<String, String>,
    /// Injected environment snapshot (`${env:…}`).
    pub env: &'a BTreeMap<String, String>,
    /// Injected `proef.toml` config scope (`${url:…}` / `${vars:…}`), with the
    /// active `[env.<name>]` already deep-merged in.
    pub config_vars: &'a BTreeMap<String, String>,
    /// Injected run identifier (`${run:id}`).
    pub run_id: &'a str,
    /// The fragment corpus (ADR-0018), read by the caller.
    ///
    /// Injected rather than built here, for the same reason every other input
    /// is: core performs no IO. It also fixes what building it internally cost —
    /// a fresh corpus means a fresh scan memo, so the LSP re-read and
    /// re-hurl-parsed the whole corpus on **every** request: each completion
    /// popup, each go-to-definition, each debounce tick. The caller holds one
    /// and rebuilds it only when a fragment file actually changes.
    pub fragments: &'a pack::FragmentCorpus,
}

impl SuiteAnalysis {
    fn push_diags(&mut self, name: &str, diags: impl IntoIterator<Item = Diag>) {
        // Ensure the primary source has a bucket even with no diagnostics, so a
        // now-clean file still surfaces an empty set that clears stale marks.
        self.diagnostics.entry(name.to_owned()).or_default();
        for d in diags {
            // Prefer the diagnostic's own source name when it carries one, so a
            // pack error raised while analyzing a feature lands on the pack.
            let target = d.source_name.clone().unwrap_or_else(|| name.to_owned());
            self.diagnostics.entry(target).or_default().push(d);
        }
    }
}

/// Recompute the whole suite in one pass: read every pack and feature through
/// the provider, accumulate every diagnostic per source name, and record the
/// binding and macro relations editors need. A broken pack contributes its own
/// diagnostic and is excluded from the loaded set, but does not stop the rest
/// of the suite from binding; a parse-failed feature is skipped, not fatal.
pub fn analyze_suite(ctx: &AnalyzeCtx<'_>) -> SuiteAnalysis {
    let mut out = SuiteAnalysis::default();

    // Packs first: a broken pack contributes its own diagnostic below and is
    // excluded from the loaded set, but its siblings still load.
    let mut sources = pack::builtin_sources();
    let pack_names = ctx.provider.discover_packs().unwrap_or_default();
    for name in &pack_names {
        match ctx.provider.read(name) {
            Ok(text) => sources.push(PackSource {
                name: name.clone(),
                text,
            }),
            Err(e) => out.push_diags(name, [read_error_diag(name, &e.0)]),
        }
    }

    // Collect-all load: a broken pack contributes its diagnostic and is
    // excluded from the set, but its siblings still load — the editor keeps
    // binding against the good packs instead of going dark (v0.5.1 fix).
    // Fragments come in already read, carrying their own read errors, or every
    // `ref:` reads as unknown in the editor while the same suite runs green —
    // the drift that makes diagnostics untrustworthy.
    let (loaded, pack_diags) = pack::load_collecting(&sources, ctx.fragments, ctx.kinds);
    for d in pack_diags {
        let name = d.source_name.clone().unwrap_or_default();
        out.push_diags(&name, [d]);
    }
    let packs: Arc<PackSet> = Arc::new(loaded);

    // Macro vocabulary for completion / go-to-def targets.
    for m in packs.macros.values() {
        out.macros.push(MacroRef {
            name: m.name.clone(),
            pattern: m.pattern.clone(),
            params: m.params.clone(),
            pack: m.pack.clone(),
            def_span: m.span,
            match_span: m.match_span,
        });
    }

    out.use_refs = index_use_refs(&packs);
    out.fragment_refs = index_fragment_refs(&packs);
    out.fragments = index_fragments(&packs);

    let world = World::new(GlobalStore::default());

    let feature_names = ctx.provider.discover_features().unwrap_or_default();
    for name in &feature_names {
        let text = match ctx.provider.read(name) {
            Ok(t) => t,
            Err(e) => {
                out.push_diags(name, [read_error_diag(name, &e.0)]);
                continue;
            }
        };
        let file = match feature::parse(name, &text) {
            Ok(f) => f,
            Err(errs) => {
                out.push_diags(name, errs);
                continue; // parse failed → skip downstream, no cascade
            }
        };

        for scenario in &file.scenarios {
            out.scenarios.push(ScenarioRef {
                feature: name.clone(),
                name: scenario.name.clone(),
                line: scenario.line,
                tags: scenario.tags.clone(),
            });
        }

        let (bound, bind_diags) = bind::bind_collect(&file, &packs);
        out.push_diags(name, bind_diags);

        for scenario in &bound {
            for step in &scenario.steps {
                out.bindings.push(Binding {
                    feature: name.clone(),
                    step_span: step.defn.span,
                    macro_name: step.macro_name.clone(),
                });
            }
        }

        let ctx_lower = LowerCtx {
            feature: &file,
            packs: &packs,
            kind_to_engine: ctx.kind_to_engine,
            env: ctx.env,
            config_vars: ctx.config_vars,
            run_id: ctx.run_id,
            world: &world,
            mode: crate::resolve::ResolveMode::DryRun,
        };
        for scenario in &bound {
            match lower::lower(scenario, &ctx_lower) {
                Ok(lowered) => {
                    out.push_diags(name, lowered.warnings.iter().cloned());
                    // Emit + artifact validation is executed for its diagnostics
                    // only; the artifact text is discarded.
                    if let Some(artifact) = emit::emit(&lowered, name, &world) {
                        let mut diags = Vec::new();
                        validate_artifact(&artifact, &lowered, ctx.kinds, &mut diags);
                        out.push_diags(name, diags);
                    }
                }
                Err(errs) => out.push_diags(name, errs),
            }
        }
    }

    out
}

/// Every fragment definition, with its annotation line as the landing anchor.
/// Names are unique across the corpus (pass 10), so unlike the `use:`/`ref:`
/// indexes below this one needs no positional pairing and no guard.
fn index_fragments(packs: &PackSet) -> Vec<FragmentDef> {
    packs
        .fragments
        .values()
        .map(|f| FragmentDef {
            name: f.name.clone(),
            file: f.file.clone(),
            span: crate::pack::locate::line_span(&f.source, f.line),
            source: Arc::clone(&f.source),
            placeholders: f.placeholders.clone(),
            supplied_variables: f.supplied_variables.clone(),
        })
        .collect()
}

/// Index one kind of cross-reference line → its resolved target, for
/// go-to-definition.
///
/// `pick` selects the step kind being indexed and `spans_of` finds that key's
/// lines; `make` resolves a target name to the record, returning `None` when it
/// resolves to nothing (an unknown target contributes no reference — pack
/// validation already reported it).
///
/// **The pairing is positional, and that is the delicate part.** Each macro's
/// parsed targets pair in order with the line spans the scanner finds. Both
/// counts come from the macro's own steps and source, so a mismatch means the
/// scanner missed a step it cannot see (a flow-style `- {use: base}` parses to a
/// step but contributes no line). That macro is then skipped entirely rather
/// than risk anchoring a reference to the wrong line. Written once here because
/// `use:` and `ref:` had separate copies of this guard, and a fix to the reasoning
/// above would have had to land in both to be true.
fn index_refs<T>(
    packs: &PackSet,
    pick: impl Fn(&MacroStepKind) -> Option<&str>,
    key: &str,
    make: impl Fn(&crate::pack::Macro, Span, &str) -> Option<T>,
) -> Vec<T> {
    // One index per pack file, shared by every macro in it — the scan is a
    // whole-file pass, and doing it per macro made this quadratic.
    let mut anchors: BTreeMap<&str, crate::pack::locate::MacroIndex<'_>> = BTreeMap::new();
    let mut out = Vec::new();
    for m in packs.macros.values() {
        let MacroBody::Steps(steps) = &m.body else {
            continue;
        };
        let targets: Vec<&str> = steps.iter().filter_map(|step| pick(&step.kind)).collect();
        // Most macros reference nothing of this kind. The count guard below
        // already rejects the empty case; leaving early also declines to build
        // an index for a file nothing here will look up.
        if targets.is_empty() {
            continue;
        }
        let index = anchors
            .entry(m.pack.as_str())
            .or_insert_with(|| crate::pack::locate::MacroIndex::new(&m.source));
        let spans = index.key_line_spans(&m.name, key);
        if spans.len() != targets.len() {
            continue;
        }
        out.extend(
            spans
                .into_iter()
                .zip(targets)
                .filter_map(|(span, target)| make(m, span, target)),
        );
    }
    out
}

/// Index every `ref:` reference → its resolved fragment.
fn index_fragment_refs(packs: &PackSet) -> Vec<FragmentRef> {
    index_refs(
        packs,
        |kind| match kind {
            MacroStepKind::Ref { target } => Some(target.as_str()),
            MacroStepKind::Use { .. } | MacroStepKind::Payload { .. } => None,
        },
        "ref",
        |m, span, target| {
            packs.find_fragment(target).map(|fragment| FragmentRef {
                pack: m.pack.clone(),
                span,
                target_fragment: fragment.name.clone(),
            })
        },
    )
}

/// Index every `use:` reference → its resolved target macro.
fn index_use_refs(packs: &PackSet) -> Vec<UseRef> {
    index_refs(
        packs,
        |kind| match kind {
            MacroStepKind::Use { target, .. } => Some(target.as_str()),
            // Only `use:` lines are indexed here; a `ref:` resolves to a
            // fragment, not a macro, so it is not a go-to-macro target.
            MacroStepKind::Payload { .. } | MacroStepKind::Ref { .. } => None,
        },
        "use",
        |m, span, target| {
            packs.find_use_target(target).map(|target_macro| UseRef {
                pack: m.pack.clone(),
                span,
                target_macro: target_macro.name.clone(),
            })
        },
    )
}

fn read_error_diag(name: &str, msg: &str) -> Diag {
    Diag::error(
        "proef::source::unreadable",
        format!("cannot read {name}: {msg}"),
    )
    .with_source(name.to_owned(), Arc::from(""))
}

/// Parse-validate the exact emitted artifact text with the claiming engine's
/// real parser (`--dry-run` = §4.1–4.5 including artifact parse-validation).
/// The diagnostic's source is the emitted text itself, span at the broken line.
///
/// This is the single implementation of artifact parse-validation, shared by the
/// CLI's fail-fast `front::run` and the LSP's collect-all `analyze_suite`. It
/// reaches the hurl parser only through the injected [`StepKindSpec::validate`]
/// function pointer, so it stays engine-agnostic and lives in the sans-IO core.
pub fn validate_artifact(
    artifact: &emit::Artifact,
    lowered: &lower::LoweredScenario,
    kinds: &[StepKindSpec],
    diags: &mut Vec<Diag>,
) {
    let Some(kind) = lowered
        .batches
        .iter()
        .flat_map(|b| b.steps.iter())
        .find(|s| matches!(s.payload, crate::step::StepPayload::HurlEntries(_)))
        .map(|s| s.kind.as_str().to_owned())
    else {
        return;
    };
    let Some(validate) = kinds
        .iter()
        .find(|k| k.prefix == kind)
        .and_then(|k| k.validate)
    else {
        return;
    };
    if let Err(err) = validate(&artifact.hurl_text) {
        let offset: usize = artifact
            .hurl_text
            .split_inclusive('\n')
            .take(err.line.saturating_sub(1))
            .map(str::len)
            .sum();
        let line_len = artifact.hurl_text[offset..]
            .lines()
            .next()
            .unwrap_or("")
            .len();
        diags.push(
            Diag::error(
                "proef::emit::invalid_artifact",
                format!(
                    "emitted artifact `{}.hurl` does not parse: {} (line {}, column {})",
                    artifact.slug, err.message, err.line, err.column
                ),
            )
            .with_source(
                format!("{}.hurl (emitted)", artifact.slug),
                std::sync::Arc::from(artifact.hurl_text.as_str()),
            )
            .with_span(Span::clamped(
                offset,
                offset + line_len.max(1),
                artifact.hurl_text.len(),
            )),
        );
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::provider::{ProviderError, SourceProvider};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// A provider backed by an in-memory map — keeps the test sans-IO.
    struct MemProvider {
        features: Vec<String>,
        packs: Vec<String>,
        fragments: Vec<String>,
        files: BTreeMap<String, Arc<str>>,
    }
    impl SourceProvider for MemProvider {
        fn discover_features(&self) -> Result<Vec<String>, ProviderError> {
            Ok(self.features.clone())
        }
        fn discover_packs(&self) -> Result<Vec<String>, ProviderError> {
            Ok(self.packs.clone())
        }
        fn discover_fragments(&self) -> Result<Vec<String>, ProviderError> {
            Ok(self.fragments.clone())
        }
        fn read(&self, name: &str) -> Result<Arc<str>, ProviderError> {
            self.files
                .get(name)
                .cloned()
                .ok_or_else(|| ProviderError(format!("no source {name}")))
        }
    }

    // Pack validation (step-kind claim, pass 8) and lowering's routing invariant
    // both require `hurl` to be a registered kind: with an empty registry a
    // `hurl:` step is `unknown_step_kind` and an unrouted lowered step. The
    // analyzer needs no *live* engine, though — a spec with no `validate` probe
    // (artifact parse-validation is skipped) plus a `hurl → hurl` route is enough
    // to bind, lower, and extract spans.
    const KINDS: &[StepKindSpec] = &[StepKindSpec {
        prefix: "hurl",
        schema: "true",
        validate: None,
        fragments: None,
        options: None,
    }];

    fn hurl_kind_map() -> &'static BTreeMap<String, String> {
        use std::sync::OnceLock;
        static M: OnceLock<BTreeMap<String, String>> = OnceLock::new();
        M.get_or_init(|| BTreeMap::from([("hurl".to_owned(), "hurl".to_owned())]))
    }

    fn empty_corpus() -> &'static pack::FragmentCorpus {
        use std::sync::OnceLock;
        static C: OnceLock<pack::FragmentCorpus> = OnceLock::new();
        C.get_or_init(pack::FragmentCorpus::empty)
    }

    fn ctx_over<'a>(
        provider: &'a dyn SourceProvider,
        empty: &'a BTreeMap<String, String>,
    ) -> AnalyzeCtx<'a> {
        AnalyzeCtx {
            provider,
            kinds: KINDS,
            kind_to_engine: hurl_kind_map(),
            env: empty,
            config_vars: empty,
            run_id: "lsp",
            fragments: empty_corpus(),
        }
    }

    #[test]
    fn analyze_surfaces_bindings_and_no_errors_on_a_clean_suite() {
        let mut files = BTreeMap::new();
        files.insert(
            "packs/p.yaml".to_owned(),
            Arc::from(
                "macros:\n  greet:\n    params: [who]\n    match: \"I greet {who}\"\n    steps:\n      - hurl: |\n          GET http://x\n",
            ),
        );
        files.insert(
            "f.feature".to_owned(),
            Arc::from("Feature: F\n  Scenario: S\n    When I greet Sam\n"),
        );
        let provider = MemProvider {
            features: vec!["f.feature".to_owned()],
            packs: vec!["packs/p.yaml".to_owned()],
            fragments: Vec::new(),
            files,
        };
        let empty = BTreeMap::new();
        let analysis = analyze_suite(&ctx_over(&provider, &empty));

        let errors: usize = analysis
            .diagnostics
            .values()
            .flatten()
            .filter(|d| d.severity == crate::diag::Severity::Error)
            .count();
        assert_eq!(
            errors, 0,
            "clean suite must have zero errors: {:?}",
            analysis.diagnostics
        );

        assert!(
            analysis
                .bindings
                .iter()
                .any(|b| b.macro_name == "greet" && b.feature == "f.feature"),
            "the greet step must be recorded as a binding"
        );
        assert!(
            analysis
                .macros
                .iter()
                .any(|m| m.name == "greet" && m.pattern.is_some())
        );
    }

    #[test]
    fn analyze_collects_unbound_without_cascade() {
        let mut files = BTreeMap::new();
        files.insert("packs/p.yaml".to_owned(), Arc::from("macros: {}\n"));
        files.insert(
            "f.feature".to_owned(),
            Arc::from("Feature: F\n  Scenario: S\n    When nothing matches this\n"),
        );
        let provider = MemProvider {
            features: vec!["f.feature".to_owned()],
            packs: vec!["packs/p.yaml".to_owned()],
            fragments: Vec::new(),
            files,
        };
        let empty = BTreeMap::new();
        let analysis = analyze_suite(&ctx_over(&provider, &empty));
        let feature_diags = analysis
            .diagnostics
            .get("f.feature")
            .expect("feature bucket");
        assert!(
            feature_diags
                .iter()
                .any(|d| d.code == "proef::bind::unbound_step")
        );
        let errors: Vec<_> = feature_diags
            .iter()
            .filter(|d| d.severity == crate::diag::Severity::Error)
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "the unbound step must be the only error-severity diagnostic, no spurious extras: {feature_diags:?}"
        );
        assert_eq!(errors[0].code, "proef::bind::unbound_step");
    }

    // §9's headline robustness property: a parse failure suppresses only its own
    // file's downstream diagnostics — it must not cascade into a sibling feature's
    // binding. Two features share one valid pack; only one of them fails to parse.
    #[test]
    fn analyze_parse_failed_feature_does_not_cascade_to_sibling() {
        let mut files = BTreeMap::new();
        files.insert(
            "packs/p.yaml".to_owned(),
            Arc::from(
                "macros:\n  greet:\n    params: [who]\n    match: \"I greet {who}\"\n    steps:\n      - hurl: |\n          GET http://x\n",
            ),
        );
        // Whitespace-only text: feature::parse's own empty-file guard
        // (`normalized.trim().is_empty()`) returns `Err` before the gherkin
        // parser even runs — a guaranteed, deliberate parse failure.
        files.insert("bad.feature".to_owned(), Arc::from("   \n"));
        files.insert(
            "good.feature".to_owned(),
            Arc::from("Feature: F\n  Scenario: S\n    When I greet Sam\n"),
        );
        let provider = MemProvider {
            features: vec!["bad.feature".to_owned(), "good.feature".to_owned()],
            packs: vec!["packs/p.yaml".to_owned()],
            fragments: Vec::new(),
            files,
        };
        let empty = BTreeMap::new();
        let analysis = analyze_suite(&ctx_over(&provider, &empty));

        // The parse-failed feature carries its own parse-error diagnostic —
        // proof the `feature::parse` `Err` branch was actually hit.
        let bad_diags = analysis
            .diagnostics
            .get("bad.feature")
            .expect("bad.feature bucket");
        assert!(
            bad_diags
                .iter()
                .any(|d| d.code == "proef::feature::empty_file"),
            "the parse-failed feature must carry its parse-error diagnostic: {bad_diags:?}"
        );

        // The sibling feature is untouched: no error diagnostics, and its
        // binding still made it through — proof there was no cascade.
        let good_diags = analysis
            .diagnostics
            .get("good.feature")
            .expect("good.feature bucket");
        assert!(
            good_diags
                .iter()
                .all(|d| d.severity != crate::diag::Severity::Error),
            "the valid sibling feature must have no error diagnostics despite the parse failure next to it: {good_diags:?}"
        );
        assert!(
            analysis
                .bindings
                .iter()
                .any(|b| b.macro_name == "greet" && b.feature == "good.feature"),
            "the valid sibling feature must still produce its binding — no cascade from the parse-failed feature"
        );
    }

    // A broken pack degrades gracefully: it reports its own error, but the good
    // pack still loads, so a feature binding against a good-pack macro survives.
    // One broken pack must never zero the whole suite's analysis (v0.5.1 fix).
    #[test]
    fn analyze_degrades_when_one_pack_is_broken() {
        let mut files = BTreeMap::new();
        files.insert(
            "packs/good.yaml".to_owned(),
            Arc::from(
                "macros:\n  greet:\n    params: [who]\n    match: \"I greet {who}\"\n    steps:\n      - hurl: |\n          GET http://x\n",
            ),
        );
        // `bogus` is not a recognized root key → deny_unknown_fields → this pack
        // fails to parse and contributes proef::pack::yaml, but must not sink the rest.
        files.insert(
            "packs/broken.yaml".to_owned(),
            Arc::from("macros: {}\nbogus: true\n"),
        );
        files.insert(
            "f.feature".to_owned(),
            Arc::from("Feature: F\n  Scenario: S\n    When I greet Sam\n"),
        );
        let provider = MemProvider {
            features: vec!["f.feature".to_owned()],
            packs: vec!["packs/good.yaml".to_owned(), "packs/broken.yaml".to_owned()],
            fragments: Vec::new(),
            files,
        };
        let empty = BTreeMap::new();
        let analysis = analyze_suite(&ctx_over(&provider, &empty));

        // The broken pack still reports its own diagnostic.
        let pack_diags = analysis
            .diagnostics
            .get("packs/broken.yaml")
            .expect("broken pack bucket");
        assert!(
            pack_diags.iter().any(|d| d.code == "proef::pack::yaml"),
            "the broken pack must carry its yaml diagnostic: {pack_diags:?}"
        );

        // The good pack still loaded: its macro is in the vocabulary...
        assert!(
            analysis.macros.iter().any(|m| m.name == "greet"),
            "the good pack's macro must survive the broken sibling"
        );
        // ...and the feature bound against it — no cascade, no zeroing.
        assert!(
            analysis
                .bindings
                .iter()
                .any(|b| b.macro_name == "greet" && b.feature == "f.feature"),
            "the feature must still bind to the good-pack macro despite the broken pack"
        );
    }

    #[test]
    fn analyze_records_use_refs_and_match_spans() {
        let mut files = BTreeMap::new();
        files.insert(
            "packs/p.yaml".to_owned(),
            Arc::from(
                "macros:\n  base:\n    match: the base\n    steps:\n      - hurl: |\n          GET http://x\n  wrapper:\n    steps:\n      - use: base\n",
            ),
        );
        let provider = MemProvider {
            features: vec![],
            packs: vec!["packs/p.yaml".to_owned()],
            fragments: Vec::new(),
            files,
        };
        let empty = BTreeMap::new();
        let analysis = analyze_suite(&ctx_over(&provider, &empty));

        // The `use: base` line is indexed, resolved to `base`.
        let u = analysis
            .use_refs
            .iter()
            .find(|u| u.target_macro == "base")
            .expect("use_ref for base");
        assert_eq!(u.pack, "packs/p.yaml");
        let src = provider_text(&provider, "packs/p.yaml");
        assert_eq!(&src[u.span.start..u.span.end], "use: base");

        // `base` carries a match_span; `wrapper` (use-only) does not.
        let base = analysis
            .macros
            .iter()
            .find(|m| m.name == "base")
            .expect("macro base");
        assert!(base.match_span.is_some());
        let wrapper = analysis
            .macros
            .iter()
            .find(|m| m.name == "wrapper")
            .expect("macro wrapper");
        assert!(wrapper.match_span.is_none());
    }

    // Guards the ordinal alignment between parsed `Use` steps and textual `use:`
    // lines: a flow-style step (`- {use: base}`) is valid YAML and parses to a
    // `Use` step, but the line scanner's `use:`-prefix match does not see it (the
    // line reads `- {use: base}`, not a `use:`-prefixed line after the dash
    // strip). Mixed with a block-style `use:` line in the same macro, the parsed
    // count (2) and textual count (1) diverge — per-ordinal pairing would silently
    // attribute the wrong textual span to a step. `index_use_refs` must skip
    // `UseRef` generation for that macro entirely rather than emit a wrong `Some`.
    #[test]
    fn analyze_skips_use_refs_when_flow_and_block_style_counts_diverge() {
        let mut files = BTreeMap::new();
        files.insert(
            "packs/p.yaml".to_owned(),
            Arc::from(
                "macros:\n  base:\n    match: the base\n    steps:\n      - hurl: |\n          GET http://x\n  wrapper:\n    steps:\n      - {use: base}\n      - use: base\n",
            ),
        );
        let provider = MemProvider {
            features: vec![],
            packs: vec!["packs/p.yaml".to_owned()],
            fragments: Vec::new(),
            files,
        };
        let empty = BTreeMap::new();
        let analysis = analyze_suite(&ctx_over(&provider, &empty));

        // The pack is valid (flow-style `use:` is legal YAML) — no error
        // diagnostics, so `analyze_suite` did not short-circuit before indexing.
        let errors: usize = analysis
            .diagnostics
            .values()
            .flatten()
            .filter(|d| d.severity == crate::diag::Severity::Error)
            .count();
        assert_eq!(
            errors, 0,
            "the mixed-style pack must be valid, zero errors: {:?}",
            analysis.diagnostics
        );

        // `base` has no `use:` steps of its own, so any `UseRef` in this suite
        // would have to come from `wrapper` — the count mismatch must suppress
        // all of them (no wrong-target/wrong-span `UseRef`, per the "never a
        // wrong `Some`" contract).
        assert!(
            analysis.use_refs.is_empty(),
            "count mismatch must skip UseRef generation for wrapper entirely, \
             not emit a misaligned pairing: {:?}",
            analysis.use_refs
        );
    }

    // Small helper to read a source back for span assertions.
    fn provider_text(p: &MemProvider, name: &str) -> String {
        p.read(name).expect("provider source").to_string()
    }

    /// The last line of defence on ADR-0010: the emitted bytes *are* the
    /// executed input, so an artifact that does not parse is a bug proef must
    /// report about itself rather than hand to the engine and let fail as a
    /// runtime error with no author-facing location.
    ///
    /// Reached with a validator that rejects — which is what a real engine's
    /// parser does when the emitter has produced something malformed, and the
    /// only way to exercise the path without first breaking the emitter.
    #[test]
    fn an_artifact_the_engine_cannot_parse_is_reported_against_the_emitter() {
        fn reject(_text: &str) -> Result<(), crate::engine::PayloadProbeError> {
            Err(crate::engine::PayloadProbeError {
                line: 1,
                column: 1,
                message: "malformed entry".into(),
            })
        }
        const REJECTING: &[StepKindSpec] = &[StepKindSpec {
            prefix: "hurl",
            schema: "true",
            validate: Some(reject),
            fragments: None,
            options: None,
        }];

        let mut files = BTreeMap::new();
        files.insert(
            "packs/p.yaml".to_owned(),
            Arc::from(
                "macros:\n  greet:\n    match: I greet Sam\n    steps:\n      - hurl: |\n          GET http://x\n          HTTP 200\n",
            ),
        );
        files.insert(
            "f.feature".to_owned(),
            Arc::from("Feature: F\n  Scenario: S\n    When I greet Sam\n"),
        );
        let provider = MemProvider {
            features: vec!["f.feature".to_owned()],
            packs: vec!["packs/p.yaml".to_owned()],
            fragments: Vec::new(),
            files,
        };
        let empty = BTreeMap::new();
        let mut ctx = ctx_over(&provider, &empty);
        ctx.kinds = REJECTING;
        let analysis = analyze_suite(&ctx);

        let codes: Vec<&str> = analysis
            .diagnostics
            .values()
            .flatten()
            .map(|d| d.code)
            .collect();
        assert!(
            codes.contains(&"proef::emit::invalid_artifact"),
            "a rejected artifact must be reported: {codes:?}"
        );
    }
}
