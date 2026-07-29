//! Lowering (TECH-SPEC §4.4): bound scenarios → engine batches.
//!
//! Macro expansion (`use:`/`with:`, cycle-safe, depth ≤ 32) · recursive `${…}`
//! resolution (ADR-0005, via [`crate::resolve`]) · the Then-merge rule
//! (`expect:` macros fold their asserts into the *previous* request entry;
//! Then-before-When is an error) · batch segmentation (**maximal**: split only
//! at engine changes and `optional:` boundaries — each optional step is a
//! singleton batch so its failure can warn without poisoning neighbors).
//!
//! Pure: environment snapshot, run id, and World are injected.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::bind::BoundScenario;
use crate::diag::{Diag, Severity};
use crate::feature::FeatureFile;
use crate::pack::{Macro, MacroBody, MacroStep, MacroStepKind, PackSet, PayloadForm};
use crate::resolve::{self, ResolveCtx, ResolveMode};
use crate::step::{Guard, LoweredStep, StepBatch, StepKindId, StepPayload, StepRef};
use crate::world::World;

/// Everything lowering needs, all injected (core purity).
#[derive(Debug, Clone, Copy)]
pub struct LowerCtx<'a> {
    /// The feature the scenario came from (directives + source for diags).
    pub feature: &'a FeatureFile,
    /// Loaded macros.
    pub packs: &'a PackSet,
    /// Step kind prefix → engine id (from the CLI's registry assembly).
    pub kind_to_engine: &'a BTreeMap<String, String>,
    /// Injected environment snapshot.
    pub env: &'a BTreeMap<String, String>,
    /// Injected run identifier.
    pub run_id: &'a str,
    /// World (globals read at lower time).
    pub world: &'a World,
    /// Strict (execution) or dry-run resolution.
    pub mode: ResolveMode,
}

/// A fully lowered scenario: ordered engine batches plus what lowering learned.
#[derive(Debug)]
pub struct LoweredScenario {
    /// Scenario name (post-expansion).
    pub name: String,
    /// Accumulated tags.
    pub tags: Vec<String>,
    /// 1-based header line.
    pub line: usize,
    /// Contiguous same-engine batches, in authored order.
    pub batches: Vec<StepBatch>,
    /// Secret names referenced anywhere in the scenario (values never appear).
    pub secrets: BTreeSet<String>,
    /// Global keys read anywhere in the scenario (drives `.vars`, ADR-0010).
    pub globals: BTreeSet<String>,
    /// Soft findings (dry-run globals, …) as warning diagnostics.
    pub warnings: Vec<Diag>,
}

/// Runtime backstop for `use:` recursion (statically checked at pack load).
const MAX_EXPANSION_DEPTH: usize = 32;

/// What resolution referenced while lowering one scenario.
#[derive(Debug, Default)]
struct Refs {
    secrets: BTreeSet<String>,
    globals: BTreeSet<String>,
}

/// Lower one bound scenario into engine batches.
pub fn lower(scenario: &BoundScenario, ctx: &LowerCtx<'_>) -> Result<LoweredScenario, Vec<Diag>> {
    let mut diags: Vec<Diag> = Vec::new();
    let mut warnings: Vec<Diag> = Vec::new();
    let mut refs = Refs::default();

    // Directives resolve first (env/run only — no step scope).
    let Some(directives) = resolve_directives(ctx, &mut refs, &mut warnings, &mut diags) else {
        return Err(diags);
    };

    let mut lowered: Vec<LoweredStep> = Vec::new();
    for step in &scenario.steps {
        let step_ref = StepRef {
            file: Arc::from(ctx.feature.path.as_str()),
            line: step.defn.line,
            text: Arc::from(step.defn.text.as_str()),
        };
        let at = |diag: Diag| {
            diag.with_source(ctx.feature.path.clone(), Arc::clone(&ctx.feature.source))
                .with_span(step.defn.span)
        };
        let Some(macro_) = ctx.packs.macros.get(&step.macro_name) else {
            continue; // binder guarantees existence
        };
        expand_macro(
            macro_,
            &step.args,
            &step_ref,
            &directives,
            ctx,
            0,
            &mut lowered,
            &mut refs,
            &mut warnings,
            &mut diags,
            &at,
        );
    }

    if diags.iter().any(|d| d.severity == Severity::Error) {
        return Err(diags);
    }

    Ok(LoweredScenario {
        name: scenario.name.clone(),
        tags: scenario.tags.clone(),
        line: scenario.line,
        batches: segment(lowered, ctx.kind_to_engine),
        secrets: refs.secrets,
        globals: refs.globals,
        warnings,
    })
}

/// Resolve `# key: value` directive values (they may reference env/run).
/// Failures land in `diags`; `None` means at least one directive is broken.
fn resolve_directives(
    ctx: &LowerCtx<'_>,
    refs: &mut Refs,
    warnings: &mut Vec<Diag>,
    diags: &mut Vec<Diag>,
) -> Option<BTreeMap<String, String>> {
    let empty = BTreeMap::new();
    let mut resolved = BTreeMap::new();
    for (key, value) in &ctx.feature.directives {
        let resolve_ctx = ResolveCtx {
            args: &empty,
            defaults: &empty,
            directives: &resolved, // earlier directives are visible to later ones
            env: ctx.env,
            run_id: ctx.run_id,
            world: ctx.world,
            mode: ctx.mode,
        };
        match resolve::resolve(value, &resolve_ctx) {
            Ok(resolution) => {
                refs.secrets.extend(resolution.secrets);
                refs.globals.extend(resolution.globals);
                push_warnings(warnings, &resolution.warnings, ctx, key);
                resolved.insert(key.clone(), resolution.text);
            }
            Err(err) => {
                diags.push(
                    Diag::error(
                        err.code(),
                        format!("directive `# {key}:` does not resolve: {err}"),
                    )
                    .with_source(ctx.feature.path.clone(), Arc::clone(&ctx.feature.source)),
                );
                return None;
            }
        }
    }
    Some(resolved)
}

/// Expand one macro invocation into lowered steps (recursing through `use:`).
#[allow(clippy::too_many_arguments)]
fn expand_macro(
    macro_: &Macro,
    args: &BTreeMap<String, String>,
    step_ref: &StepRef,
    directives: &BTreeMap<String, String>,
    ctx: &LowerCtx<'_>,
    depth: usize,
    out: &mut Vec<LoweredStep>,
    refs: &mut Refs,
    warnings: &mut Vec<Diag>,
    diags: &mut Vec<Diag>,
    at: &impl Fn(Diag) -> Diag,
) {
    if depth > MAX_EXPANSION_DEPTH {
        diags.push(at(Diag::error(
            "proef::lower::expansion_too_deep",
            format!(
                "macro expansion exceeded depth {MAX_EXPANSION_DEPTH} at `{}`",
                macro_.name
            ),
        )));
        return;
    }

    let resolve_in = |text: &str,
                      refs: &mut Refs,
                      warnings: &mut Vec<Diag>,
                      diags: &mut Vec<Diag>|
     -> Option<String> {
        let resolve_ctx = ResolveCtx {
            args,
            defaults: &macro_.defaults,
            directives,
            env: ctx.env,
            run_id: ctx.run_id,
            world: ctx.world,
            mode: ctx.mode,
        };
        match resolve::resolve(text, &resolve_ctx) {
            Ok(resolution) => {
                refs.secrets.extend(resolution.secrets);
                refs.globals.extend(resolution.globals);
                push_warnings(warnings, &resolution.warnings, ctx, &macro_.name);
                Some(resolution.text)
            }
            Err(err) => {
                diags.push(at(Diag::error(
                    err.code(),
                    format!("in macro `{}`: {err}", macro_.name),
                )));
                None
            }
        }
    };

    match &macro_.body {
        MacroBody::Expect(items) => {
            let mut merged: Option<(StepKindId, bool, usize)> = None;
            for item in items {
                let status = match &item.status {
                    Some(status) => match resolve_in(status, refs, warnings, diags) {
                        Some(status) => Some(status),
                        None => continue,
                    },
                    None => None,
                };
                let fragment = match &item.fragment {
                    Some(fragment) => match resolve_in(fragment, refs, warnings, diags) {
                        Some(fragment) => Some(fragment),
                        None => continue,
                    },
                    None => None,
                };
                if let Some((kind, optional, lines)) =
                    merge_expect(status.as_deref(), fragment.as_deref(), out, diags, at)
                {
                    let entry = merged.get_or_insert((kind, optional, 0));
                    entry.2 += lines;
                }
            }
            // The authored `Then` surfaces as its own step (§2.7): zero bytes
            // of its own, anchored on the assert lines it appended to the
            // host entry. It shares the host's fate (`optional` inherited).
            if let Some((kind, optional, lines)) = merged {
                out.push(LoweredStep {
                    step: step_ref.clone(),
                    kind,
                    payload: StepPayload::MergedAsserts { lines },
                    optional,
                    when: None,
                    label: None,
                    save_as: std::collections::BTreeMap::new(),
                });
            }
        }
        MacroBody::Steps(steps) => {
            for macro_step in steps {
                expand_step(
                    macro_step,
                    step_ref,
                    directives,
                    ctx,
                    depth,
                    out,
                    refs,
                    warnings,
                    diags,
                    at,
                    &resolve_in,
                );
            }
        }
    }
}

/// Expand one pack step (payload or `use:` composition).
#[allow(clippy::too_many_arguments)]
fn expand_step(
    macro_step: &MacroStep,
    step_ref: &StepRef,
    directives: &BTreeMap<String, String>,
    ctx: &LowerCtx<'_>,
    depth: usize,
    out: &mut Vec<LoweredStep>,
    refs: &mut Refs,
    warnings: &mut Vec<Diag>,
    diags: &mut Vec<Diag>,
    at: &impl Fn(Diag) -> Diag,
    resolve_in: &impl Fn(&str, &mut Refs, &mut Vec<Diag>, &mut Vec<Diag>) -> Option<String>,
) {
    match &macro_step.kind {
        MacroStepKind::Use { target, with } => {
            let Some(target_macro) = ctx.packs.find_use_target(target) else {
                return; // pack validation reported it
            };
            // `with:` values resolve in the *parent* scope, then become the
            // child's args (child defaults fill the rest).
            let mut child_args = BTreeMap::new();
            for (key, value) in with {
                if let Some(resolved) = resolve_in(value, refs, warnings, diags) {
                    child_args.insert(key.clone(), resolved);
                }
            }
            expand_macro(
                target_macro,
                &child_args,
                step_ref,
                directives,
                ctx,
                depth + 1,
                out,
                refs,
                warnings,
                diags,
                at,
            );
        }
        MacroStepKind::Payload { kind, payload } => {
            let payload = match payload {
                PayloadForm::Raw(text) => {
                    let Some(resolved) = resolve_in(text, refs, warnings, diags) else {
                        return;
                    };
                    // Bake `retry:`/`delay:` into hurl `[Options]` so artifacts
                    // replay with identical semantics under the stock CLI
                    // (ADR-0010); per-entry [Options] override batch defaults.
                    let resolved = if macro_step.retry.is_some() || macro_step.delay_ms.is_some() {
                        bake_entry_options(&resolved, macro_step.retry, macro_step.delay_ms)
                    } else {
                        resolved
                    };
                    StepPayload::HurlEntries(resolved)
                }
                PayloadForm::Structured(value) => {
                    // `${…}` resolves inside structured payloads exactly as in
                    // raw ones (ADR-0005): every string value, recursively.
                    // Keys are schema, not data — they stay literal.
                    let mut resolve = |text: &str| {
                        // No `$` ⇒ no placeholders and no `$${` escapes: skip
                        // the resolver's copy passes for the common case.
                        if !text.contains('$') {
                            return Some(text.to_owned());
                        }
                        resolve_in(text, refs, warnings, diags)
                    };
                    match resolve_structured(value, &mut resolve) {
                        Some(resolved) => StepPayload::Structured(resolved),
                        None => return,
                    }
                }
            };
            let when = match &macro_step.when {
                Some(guard) => match resolve_in(guard, refs, warnings, diags) {
                    Some(resolved) => Some(Guard(resolved)),
                    None => return,
                },
                None => None,
            };
            // Labels resolve like payloads (same scope, same strictness) —
            // otherwise raw `${…}` leaks into artifact comments and events.
            let label = match &macro_step.name {
                Some(name) => match resolve_in(name, refs, warnings, diags) {
                    Some(resolved) => Some(resolved),
                    None => return,
                },
                None => None,
            };
            out.push(LoweredStep {
                step: step_ref.clone(),
                kind: StepKindId::from(kind.as_str()),
                payload,
                optional: macro_step.optional,
                when,
                label,
                save_as: macro_step.save_as.clone(),
            });
        }
    }
}

/// Every string *value* in a structured payload resolved through `resolve`
/// (`None` propagates a resolution failure — the caller already has diags).
fn resolve_structured(
    value: &serde_json::Value,
    resolve: &mut dyn FnMut(&str) -> Option<String>,
) -> Option<serde_json::Value> {
    use serde_json::Value as J;
    Some(match value {
        J::String(text) => J::String(resolve(text)?),
        J::Array(items) => J::Array(
            items
                .iter()
                .map(|item| resolve_structured(item, resolve))
                .collect::<Option<_>>()?,
        ),
        J::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, item) in map {
                out.insert(key.clone(), resolve_structured(item, resolve)?);
            }
            J::Object(out)
        }
        other => other.clone(),
    })
}

/// Inject `[Options] retry/retry-interval` after each entry's header block.
///
/// Textual by necessity (the core owns no hurl parser), safe by construction:
/// the emitted artifact is parse-validated with the real parser, so a bad
/// injection cannot survive to execution. An existing `[Options]` section is
/// extended instead of duplicated (hurl rejects duplicate sections).
fn bake_entry_options(
    text: &str,
    retry: Option<crate::step::Retry>,
    delay_ms: Option<u64>,
) -> String {
    let mut option_lines: Vec<String> = Vec::new();
    if let Some(retry) = retry {
        option_lines.push(format!("retry: {}", retry.count));
        option_lines.push(format!("retry-interval: {}ms", retry.interval_ms));
    }
    if let Some(delay_ms) = delay_ms {
        option_lines.push(format!("delay: {delay_ms}ms"));
    }
    let retry_lines = option_lines.join("\n");
    let mut out: Vec<String> = Vec::new();
    let mut in_entry_head = false; // between a method line and its first section/body
    let mut injected_current = false;
    let mut in_fence = false; // inside a ```…``` body — no entry surgery there
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            // A fence opening directly after the entry head is the body — the
            // options section belongs immediately before it.
            if !in_fence && in_entry_head && !injected_current {
                out.push("[Options]".to_owned());
                out.push(retry_lines.clone());
                injected_current = true;
            }
            in_fence = !in_fence;
            in_entry_head = false;
            out.push(line.to_owned());
            continue;
        }
        if in_fence {
            out.push(line.to_owned());
            continue;
        }
        let is_method_line = trimmed.split_whitespace().next().is_some_and(|word| {
            word.len() >= 3
                && word.chars().all(|c| c.is_ascii_uppercase() || c == '-')
                && word != "HTTP"
        }) && trimmed.split_whitespace().count() >= 2;
        if is_method_line {
            in_entry_head = true;
            injected_current = false;
            out.push(line.to_owned());
            continue;
        }
        if trimmed == "[Options]" {
            // Extend the author's own section.
            out.push(line.to_owned());
            out.push(retry_lines.clone());
            injected_current = true;
            in_entry_head = false;
            continue;
        }
        let is_header = in_entry_head && is_header_line(trimmed);
        if in_entry_head && !is_header && !injected_current {
            out.push("[Options]".to_owned());
            out.push(retry_lines.clone());
            injected_current = true;
            in_entry_head = false;
        }
        out.push(line.to_owned());
    }
    if in_entry_head && !injected_current {
        out.push("[Options]".to_owned());
        out.push(retry_lines.clone());
    }
    let mut result = out.join("\n");
    if text.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// The Then-merge rule (ADR-0004): fold a status assert and/or raw assert
/// fragment into the previous request entry — error when no request precedes
/// (Then-before-When).
/// Merge one `expect:` item into the previous request entry. Returns the
/// host's `(kind, optional)` plus how many assert lines were appended —
/// the caller anchors an authored-step row on them (§2.7 visibility).
fn merge_expect(
    status: Option<&str>,
    fragment: Option<&str>,
    out: &mut [LoweredStep],
    diags: &mut Vec<Diag>,
    at: &impl Fn(Diag) -> Diag,
) -> Option<(StepKindId, bool, usize)> {
    let Some(previous) = out
        .iter_mut()
        .rev()
        .find(|s| matches!(s.payload, StepPayload::HurlEntries(_)))
    else {
        diags.push(
            at(Diag::error(
                "proef::lower::then_before_when",
                "this assert-only step has no previous request entry to attach to",
            ))
            .with_help("a Then step asserts on the request made by an earlier When step"),
        );
        return None;
    };

    if let Some(status) = status
        && (!status.chars().all(|c| c.is_ascii_digit()) || status.is_empty())
    {
        diags.push(at(Diag::error(
            "proef::lower::bad_status",
            format!("expected an HTTP status number, got `{status}`"),
        )));
        return None;
    }

    let host_kind = previous.kind.clone();
    let host_optional = previous.optional;
    let StepPayload::HurlEntries(text) = &mut previous.payload else {
        return None;
    };
    // Ensure the last entry has a response section, then an [Asserts] section,
    // then append the asserts. (The emitter parse-validates the result.)
    if !text.lines().any(|l| l.trim_start().starts_with("HTTP")) {
        push_line(text, "HTTP *");
    }
    if !text.lines().any(|l| l.trim() == "[Asserts]") {
        push_line(text, "[Asserts]");
    }
    let mut appended = 0usize;
    if let Some(status) = status {
        push_line(text, &format!("status == {status}"));
        appended += 1;
    }
    if let Some(fragment) = fragment {
        for line in fragment.lines().filter(|l| !l.trim().is_empty()) {
            push_line(text, line.trim_end());
            appended += 1;
        }
    }
    Some((host_kind, host_optional, appended))
}

/// Is this a `Name: value` HTTP header line per hurl's grammar? The name must
/// be a non-empty run of token characters before the colon — an XML/JSON/text
/// body line (`<root xmlns:x=…`, `{"a": 1}`, prose) never qualifies, so the
/// `[Options]` injection can never land inside a body.
fn is_header_line(trimmed: &str) -> bool {
    let Some((name, _)) = trimmed.split_once(':') else {
        return false;
    };
    !name.is_empty()
        && name != "HTTP"
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c))
}

fn push_line(text: &mut String, line: &str) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
}

/// Maximal segmentation: contiguous same-engine steps share a batch; a batch
/// breaks at engine changes and around `optional:` steps (each optional step
/// is a singleton batch so its failure warns without poisoning neighbors).
fn segment(steps: Vec<LoweredStep>, kind_to_engine: &BTreeMap<String, String>) -> Vec<StepBatch> {
    let mut batches: Vec<StepBatch> = Vec::new();
    for step in steps {
        let engine = kind_to_engine
            .get(step.kind.as_str())
            .map_or_else(|| step.kind.as_str().to_owned(), Clone::clone);
        // A merged-asserts step never opens a batch: its asserts live inside
        // the previous step's entry, so it must ride in the same dispatch.
        let glued = matches!(step.payload, StepPayload::MergedAsserts { .. });
        let start_new = match batches.last() {
            None => true,
            Some(last) => {
                !glued
                    && (last.engine.as_str() != engine
                        || step.optional
                        || last.steps.last().is_some_and(|s| s.optional))
            }
        };
        if start_new {
            batches.push(StepBatch {
                index: batches.len(),
                engine: crate::engine::EngineId::from(engine.as_str()),
                steps: vec![step],
            });
        } else if let Some(last) = batches.last_mut() {
            last.steps.push(step);
        }
    }
    batches
}

fn push_warnings(warnings: &mut Vec<Diag>, texts: &[String], ctx: &LowerCtx<'_>, where_: &str) {
    for text in texts {
        warnings.push(
            Diag::warning("proef::lower::dry_run_unknown", format!("{where_}: {text}"))
                .with_source(ctx.feature.path.clone(), Arc::clone(&ctx.feature.source)),
        );
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::engine::StepKindSpec;
    use crate::pack::{self, PackSource};
    use crate::step::StepPayload;

    const KINDS: &[StepKindSpec] = &[StepKindSpec {
        prefix: "hurl",
        schema: "true",
        validate: None,
    }];

    const PACK: &str = r#"templates:
  auth:
    params: [token]
    steps:
      - name: authenticate
        hurl: |
          POST ${baseURL}/auth
          Authorization: Bearer ${token}
          HTTP 200
  search:
    params: [term]
    match: "I search for {term}"
    steps:
      - use: auth
        with: { token: "${secret:apiToken}" }
      - name: run the search
        hurl: |
          GET ${baseURL}/search?q=${term}
          HTTP 200
          [Captures]
          clientId: jsonpath "$[0].id"
  checkHealth:
    match: the service is healthy
    steps:
      - optional: true
        hurl: |
          GET ${baseURL}/health
  expectStatus:
    params: [status]
    match: "the response status is {status}"
    expect:
      - status: "${status}"
"#;

    fn fixture() -> (
        crate::feature::FeatureFile,
        crate::bind::BoundScenario,
        PackSet,
    ) {
        let packs = pack::load(
            &[PackSource {
                name: "test.yaml".into(),
                text: Arc::from(PACK),
            }],
            KINDS,
        )
        .unwrap();
        let feature = crate::feature::parse(
            "t.feature",
            "# baseURL: http://fixture.local\nFeature: F\n  Scenario: S\n    Given the service is healthy\n    When I search for \"Jansen\"\n    Then the response status is 200\n",
        )
        .unwrap();
        let scenario = crate::bind::bind(&feature, &packs).unwrap().remove(0);
        (feature, scenario, packs)
    }

    fn ctx<'a>(
        feature: &'a crate::feature::FeatureFile,
        packs: &'a PackSet,
        kind_to_engine: &'a BTreeMap<String, String>,
        env: &'a BTreeMap<String, String>,
        world: &'a World,
    ) -> LowerCtx<'a> {
        LowerCtx {
            feature,
            packs,
            kind_to_engine,
            env,
            run_id: "run-0001",
            world,
            mode: ResolveMode::DryRun,
        }
    }

    #[test]
    fn expansion_resolution_merge_and_segmentation_work_together() {
        let (feature, scenario, packs) = fixture();
        let kind_to_engine: BTreeMap<String, String> =
            [("hurl".to_owned(), "hurl".to_owned())].into();
        let env = BTreeMap::new();
        let world = World::default();
        let lowered = lower(
            &scenario,
            &ctx(&feature, &packs, &kind_to_engine, &env, &world),
        )
        .unwrap();

        // Optional health check is a singleton batch; auth + search batch
        // together, and the authored `Then` rides along as a visible
        // merged-asserts step (§2.7) glued to its host.
        assert_eq!(lowered.batches.len(), 2);
        assert_eq!(lowered.batches[0].steps.len(), 1);
        assert!(lowered.batches[0].steps[0].optional);
        assert_eq!(lowered.batches[1].steps.len(), 3);
        let StepPayload::MergedAsserts { lines } = lowered.batches[1].steps[2].payload else {
            panic!("expected a merged-asserts step for the Then line");
        };
        assert_eq!(lines, 1, "the expect appended exactly `status == 200`");

        // use:/with: expansion resolved the parent's secret reference.
        let StepPayload::HurlEntries(auth) = &lowered.batches[1].steps[0].payload else {
            panic!("expected hurl entries");
        };
        assert!(auth.contains("POST http://fixture.local/auth"), "{auth}");
        assert!(
            auth.contains("Bearer {{apiToken}}"),
            "secret placeholder: {auth}"
        );
        assert!(lowered.secrets.contains("apiToken"));

        // The expect macro merged `status == 200` into the *search* entry.
        let StepPayload::HurlEntries(search) = &lowered.batches[1].steps[1].payload else {
            panic!("expected hurl entries");
        };
        assert!(
            search.contains("GET http://fixture.local/search?q=Jansen"),
            "{search}"
        );
        assert!(search.contains("[Asserts]"), "{search}");
        assert!(search.trim_end().ends_with("status == 200"), "{search}");

        // Anchors point at the feature lines.
        assert_eq!(lowered.batches[1].steps[1].step.line, 5);
        assert_eq!(
            lowered.batches[1].steps[0].label.as_deref(),
            Some("authenticate")
        );
    }

    #[test]
    fn then_before_when_is_an_error() {
        let (_, _, packs) = fixture();
        let feature = crate::feature::parse(
            "t.feature",
            "Feature: F\n  Scenario: S\n    Then the response status is 200\n",
        )
        .unwrap();
        let scenario = crate::bind::bind(&feature, &packs).unwrap().remove(0);
        let kind_to_engine = BTreeMap::new();
        let env = BTreeMap::new();
        let world = World::default();
        let errs = lower(
            &scenario,
            &ctx(&feature, &packs, &kind_to_engine, &env, &world),
        )
        .unwrap_err();
        assert_eq!(errs[0].code, "proef::lower::then_before_when");
    }

    /// `${…}` resolves inside structured payload string values,
    /// recursively — keys stay literal (they are schema, not data).
    #[test]
    fn structured_payloads_resolve_placeholders_recursively() {
        const WEB_KINDS: &[StepKindSpec] = &[StepKindSpec {
            prefix: "web",
            schema: "true",
            validate: None,
        }];
        let packs = pack::load(
            &[PackSource {
                name: "web.yaml".into(),
                text: Arc::from(
                    "templates:\n  open:\n    match: the page is opened\n    steps:\n      - name: open\n        web:\n          goto: \"${baseURL}/page\"\n          checks: [\"${baseURL}\", 7]\n",
                ),
            }],
            WEB_KINDS,
        )
        .unwrap();
        let feature = crate::feature::parse(
            "t.feature",
            "# baseURL: http://fixture.local\nFeature: F\n  Scenario: S\n    When the page is opened\n",
        )
        .unwrap();
        let scenario = crate::bind::bind(&feature, &packs).unwrap().remove(0);
        let kind_to_engine: BTreeMap<String, String> =
            [("web".to_owned(), "web".to_owned())].into();
        let env = BTreeMap::new();
        let world = World::default();
        let lowered = lower(
            &scenario,
            &ctx(&feature, &packs, &kind_to_engine, &env, &world),
        )
        .unwrap();
        let StepPayload::Structured(value) = &lowered.batches[0].steps[0].payload else {
            panic!("structured payload expected");
        };
        assert_eq!(value["goto"], "http://fixture.local/page");
        assert_eq!(value["checks"][0], "http://fixture.local");
        assert_eq!(value["checks"][1], 7);
    }

    /// The `[Options]` injection must never land inside a body: fenced text,
    /// XML (colon-bearing first line), and JSON bodies all stay untouched.
    #[test]
    fn baked_options_never_enter_bodies() {
        let retry = Some(crate::step::Retry {
            count: 2,
            interval_ms: 100,
        });
        for body in [
            "POST http://x/a\n```\nNOTE FOR REVIEW\nsecond line\n```\nHTTP 200\n",
            "POST http://x/a\n<root xmlns:x=\"urn:example\">\n  <child>hi</child>\n</root>\nHTTP 200\n",
            "POST http://x/a\n{\"note\": \"FOR REVIEW\"}\nHTTP 200\n",
        ] {
            let baked = bake_entry_options(body, retry, None);
            assert_eq!(
                baked.matches("[Options]").count(),
                1,
                "exactly one options block in:\n{baked}"
            );
            let options_at = baked.find("[Options]").unwrap_or(usize::MAX);
            let body_at = baked
                .find("```")
                .or_else(|| baked.find('<'))
                .or_else(|| baked.find('{'))
                .unwrap_or(0);
            assert!(options_at < body_at, "options precede the body:\n{baked}");
        }
    }

    #[test]
    fn engine_change_splits_batches() {
        let steps: Vec<LoweredStep> = ["hurl", "hurl", "web", "hurl"]
            .iter()
            .map(|kind| LoweredStep {
                step: StepRef {
                    file: Arc::from("f"),
                    line: 1,
                    text: Arc::from("t"),
                },
                kind: StepKindId::from(*kind),
                payload: StepPayload::HurlEntries(String::new()),
                optional: false,
                when: None,
                label: None,
                save_as: BTreeMap::new(),
            })
            .collect();
        let mapping: BTreeMap<String, String> = [
            ("hurl".to_owned(), "hurl".to_owned()),
            ("web".to_owned(), "web".to_owned()),
        ]
        .into();
        let batches = segment(steps, &mapping);
        let sizes: Vec<usize> = batches.iter().map(|b| b.steps.len()).collect();
        assert_eq!(sizes, vec![2, 1, 1]);
        assert_eq!(batches[1].engine.as_str(), "web");
        // Scenario-wide ordinals — the sidecar `batch` key engines filter by.
        let indexes: Vec<usize> = batches.iter().map(|b| b.index).collect();
        assert_eq!(indexes, vec![0, 1, 2]);
    }
}
