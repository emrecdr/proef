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
/// binding and macro relations editors need. Broken packs short-circuit feature
/// binding (no cascade); a parse-failed feature is skipped, not fatal.
pub fn analyze_suite(ctx: &AnalyzeCtx<'_>) -> SuiteAnalysis {
    let mut out = SuiteAnalysis::default();

    // Packs first: a broken pack blocks binding, so on pack failure we publish
    // pack diagnostics and skip feature binding (no cascade).
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

    let packs: Arc<PackSet> = match pack::load(&sources, ctx.kinds) {
        Ok(set) => Arc::new(set),
        Err(err) => {
            for d in front_error_diags(err) {
                let name = d.source_name.clone().unwrap_or_default();
                out.push_diags(&name, [d]);
            }
            return out; // packs broken → do not cascade into feature binding
        }
    };

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
                    let stem = feature_stem(name);
                    if let Some(artifact) = emit::emit(&lowered, &stem, &world) {
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

/// Index every `use:` reference → its resolved target, for go-to-def from a
/// `use:` line. The per-macro ordinal matches `pack::locate::use_span`'s counting —
/// guarded per macro: if the parsed `Use` step count and the textual `use:`-line
/// count diverge (e.g. a flow-style `- {use: base}` step, which parses fine but
/// isn't seen by the line scanner), the ordinal pairing is unreliable, so that
/// macro's `use:` lines are skipped entirely rather than risk a wrong `Some`.
fn index_use_refs(packs: &PackSet) -> Vec<UseRef> {
    let mut use_refs = Vec::new();
    for m in packs.macros.values() {
        let MacroBody::Steps(steps) = &m.body else {
            continue;
        };
        let parsed_use_count = steps
            .iter()
            .filter(|step| matches!(step.kind, MacroStepKind::Use { .. }))
            .count();
        let text_use_count = crate::pack::locate::count_use_lines(&m.source, &m.name);
        if parsed_use_count != text_use_count {
            continue; // counts diverge → per-ordinal pairing unreliable, skip
        }
        let mut ordinal = 0usize;
        for step in steps {
            if let MacroStepKind::Use { target, .. } = &step.kind {
                if let Some(span) = crate::pack::locate::use_span(&m.source, &m.name, ordinal)
                    && let Some(target_macro) = packs.find_use_target(target)
                {
                    use_refs.push(UseRef {
                        pack: m.pack.clone(),
                        span,
                        target_macro: target_macro.name.clone(),
                    });
                }
                ordinal += 1;
            }
        }
    }
    use_refs
}

fn feature_stem(name: &str) -> String {
    std::path::Path::new(name).file_stem().map_or_else(
        || "feature".to_owned(),
        |s| s.to_string_lossy().into_owned(),
    )
}

fn read_error_diag(name: &str, msg: &str) -> Diag {
    Diag::error(
        "proef::source::unreadable",
        format!("cannot read {name}: {msg}"),
    )
    .with_source(name.to_owned(), Arc::from(""))
}

fn front_error_diags(err: crate::diag::FrontError) -> Vec<Diag> {
    match err {
        crate::diag::FrontError::Diagnostics(list) => list,
        crate::diag::FrontError::Core(core) => {
            vec![Diag::error("proef::pack::load", core.to_string())]
        }
    }
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
        files: BTreeMap<String, Arc<str>>,
    }
    impl SourceProvider for MemProvider {
        fn discover_features(&self) -> Result<Vec<String>, ProviderError> {
            Ok(self.features.clone())
        }
        fn discover_packs(&self) -> Result<Vec<String>, ProviderError> {
            Ok(self.packs.clone())
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
    }];

    fn hurl_kind_map() -> &'static BTreeMap<String, String> {
        use std::sync::OnceLock;
        static M: OnceLock<BTreeMap<String, String>> = OnceLock::new();
        M.get_or_init(|| BTreeMap::from([("hurl".to_owned(), "hurl".to_owned())]))
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

    // §9's other half: a broken pack short-circuits before feature binding even
    // starts, so a perfectly normal feature must not be falsely reported.
    #[test]
    fn analyze_broken_pack_short_circuits_before_feature_binding() {
        let mut files = BTreeMap::new();
        // `deny_unknown_fields` on the pack schema's root: `bogus` is not a
        // recognized key, so `serde_norway::from_str::<RawPack>` returns `Err`
        // and `pack::load` returns `Err(FrontError::Diagnostics(..))` carrying
        // `proef::pack::yaml` (mirrors `tests/errors/pack__yaml`).
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
            packs: vec!["packs/broken.yaml".to_owned()],
            files,
        };
        let empty = BTreeMap::new();
        let analysis = analyze_suite(&ctx_over(&provider, &empty));

        // The broken pack carries its yaml diagnostic — proof `pack::load`'s
        // `Err` branch was actually hit.
        let pack_diags = analysis
            .diagnostics
            .get("packs/broken.yaml")
            .expect("pack bucket");
        assert!(
            pack_diags.iter().any(|d| d.code == "proef::pack::yaml"),
            "the broken pack must carry its yaml diagnostic: {pack_diags:?}"
        );

        // Feature binding never ran: no bindings recorded anywhere, and the
        // feature was not even visited (no bucket, so certainly no
        // `proef::bind::*` diagnostics) — proof `analyze_suite` returned early.
        assert!(
            analysis.bindings.is_empty(),
            "no bindings should be produced when the pack fails to load: {:?}",
            analysis.bindings
        );
        let feature_diags = analysis.diagnostics.get("f.feature");
        assert!(
            feature_diags
                .is_none_or(|diags| !diags.iter().any(|d| d.code.starts_with("proef::bind::"))),
            "the feature must not be falsely reported when the pack short-circuited binding: {feature_diags:?}"
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
}
