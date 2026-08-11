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
    /// The feature the scenario came from (source for diags).
    pub feature: &'a FeatureFile,
    /// Loaded macros.
    pub packs: &'a PackSet,
    /// Step kind prefix → engine id (from the CLI's registry assembly).
    pub kind_to_engine: &'a BTreeMap<String, String>,
    /// Injected environment snapshot.
    pub env: &'a BTreeMap<String, String>,
    /// Injected `proef.toml` config scope (`${url:key}` / `${vars:key}`), keyed
    /// `"<namespace>:<key>"` — the active `[env.<name>]` already deep-merged in.
    pub config_vars: &'a BTreeMap<String, String>,
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
    /// `${fake:*}` occurrence counter, shared across every step in the
    /// scenario (not reset per `resolve()` call) so two steps each asking
    /// for a fresh `${fake:email}` get distinct values. Scoped to one
    /// `lower()` call — i.e. one scenario — which keeps it a pure function
    /// of `(run_id, the scenario's own step order)`: `lower()` runs
    /// single-threaded per scenario (`runner.rs`: scenario-per-OS-thread),
    /// so no cross-scenario or cross-thread ordering ever reaches it.
    fakes: usize,
}

/// Lower one bound scenario into engine batches.
pub fn lower(scenario: &BoundScenario, ctx: &LowerCtx<'_>) -> Result<LoweredScenario, Vec<Diag>> {
    let mut diags: Vec<Diag> = Vec::new();
    let mut warnings: Vec<Diag> = Vec::new();
    let mut refs = Refs::default();
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
            ctx,
            0,
            &mut lowered,
            &mut refs,
            &mut warnings,
            &mut diags,
            &at,
        );
    }

    // Routing invariant (ADR-0002): every non-glued step's kind must map to a
    // registered engine. Pack validation and the CLI registry keep this true,
    // so a miss is drift between them — surfaced as an explicit internal
    // fault instead of a fabricated engine id that fails later and further
    // from the cause.
    for step in &lowered {
        if matches!(step.payload, StepPayload::MergedAsserts { .. }) {
            continue; // glued to its host batch — no engine of its own
        }
        if !ctx.kind_to_engine.contains_key(step.kind.as_str()) {
            diags.push(
                Diag::error(
                    "proef::lower::kind_unrouted",
                    format!(
                        "internal: step kind `{}` is not claimed by any registered engine \
                         (registry/pack-validation drift)",
                        step.kind.as_str()
                    ),
                )
                .with_source(ctx.feature.path.clone(), Arc::clone(&ctx.feature.source)),
            );
        }
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

/// Expand one macro invocation into lowered steps (recursing through `use:`).
#[allow(clippy::too_many_arguments)]
fn expand_macro(
    macro_: &Macro,
    args: &BTreeMap<String, String>,
    step_ref: &StepRef,
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
            env: ctx.env,
            config_vars: ctx.config_vars,
            run_id: ctx.run_id,
            world: ctx.world,
            mode: ctx.mode,
        };
        match resolve::resolve(text, &resolve_ctx, &mut refs.fakes) {
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
            // The label annotates *this* step for humans (artifact comments,
            // events) — it must report the fake values the step actually
            // used, not mint fresh ones of its own. Remember where the
            // occurrence counter stood before the functional resolves
            // (payload, then guard) so the label can replay from the same
            // base afterward.
            let label_fakes_start = refs.fakes;
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
            // But a label is a *replay*, not a new use: it shares whatever
            // `${…}` text the payload/guard already resolved (typically the
            // same captured arg, e.g. `name: search ${term}` next to
            // `q: ${term}`), so it resolves from a scratch copy of the
            // counter rewound to `label_fakes_start` — reproducing the
            // payload's own fake values instead of consuming fresh
            // occurrences that would then shift every later step's fakes by
            // one. Restoring to a *fixed* end (wherever payload/guard left
            // off) is only correct when the label is an exact mirror: a
            // label with a `${fake:…}` the payload never had still consumes
            // real occurrences during the replay, and blindly rewinding past
            // them would hand those numbers back out to a later step —
            // colliding with a value this label already displayed. So the
            // real counter is restored to the *high-water mark* of the two —
            // wherever payload/guard left off, or wherever the label's own
            // replay reached, whichever is further — never below either.
            // Mirrored references replay with no trace (unaffected, the
            // common case); any extra reference the label consumes stays
            // consumed and can never be reissued.
            let functional_fakes_end = refs.fakes;
            let label = match &macro_step.name {
                Some(name) => {
                    refs.fakes = label_fakes_start;
                    let resolved = resolve_in(name, refs, warnings, diags);
                    refs.fakes = functional_fakes_end.max(refs.fakes);
                    match resolved {
                        Some(resolved) => Some(resolved),
                        None => return,
                    }
                }
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
    // Pre-scan: entries whose author already wrote an `[Options]` section only
    // get that section extended — auto-injecting a fresh one as well would
    // leave two `[Options]` sections in one entry, which hurl rejects. Slot 0
    // is the preamble before the first method line.
    let mut author_options = vec![false];
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if is_method_line(trimmed) {
            author_options.push(false);
        } else if trimmed == "[Options]"
            && let Some(last) = author_options.last_mut()
        {
            *last = true;
        }
    }
    let has_author_options = |entry: usize| author_options.get(entry).copied().unwrap_or(false);
    let mut out: Vec<String> = Vec::new();
    let mut in_entry_head = false; // between a method line and its first section/body
    let mut injected_current = false;
    let mut in_fence = false; // inside a ```…``` body — no entry surgery there
    let mut entry = 0usize;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            // A fence opening directly after the entry head is the body — the
            // options section belongs immediately before it.
            if !in_fence && in_entry_head && !injected_current && !has_author_options(entry) {
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
        if is_method_line(trimmed) {
            in_entry_head = true;
            injected_current = false;
            entry += 1;
            out.push(line.to_owned());
            continue;
        }
        if trimmed == "[Options]" {
            // Extend the author's own section (once — a second author section
            // is the pack's own parse error, not ours to widen).
            out.push(line.to_owned());
            if !injected_current {
                out.push(retry_lines.clone());
                injected_current = true;
            }
            in_entry_head = false;
            continue;
        }
        let is_header = in_entry_head && is_header_line(trimmed);
        if in_entry_head && !is_header && !injected_current && !has_author_options(entry) {
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
    // Ensure the *last* entry has a response section, then an [Asserts]
    // section, then append the asserts. (The emitter parse-validates the
    // result.) The scan is scoped to the last entry with fences skipped — an
    // earlier entry's response, or a fenced body line starting with `HTTP`,
    // must not satisfy it (ADR-0004: merge into the previous entry, singular).
    let (tail_has_http, tail_has_asserts) = last_entry_scan(text);
    if !tail_has_http {
        push_line(text, "HTTP *");
    }
    if !tail_has_asserts {
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

/// Is this trimmed line an entry-opening method line (`GET http://…`)? Custom
/// methods are any ≥ 3-char run of ASCII uppercase / `-` — except `HTTP`,
/// which opens a response.
///
/// Shared with the emitter's capture scan (`emit::capture_names`) — one
/// canonical method recogniser, not a duplicate.
pub(crate) fn is_method_line(trimmed: &str) -> bool {
    trimmed.split_whitespace().next().is_some_and(|word| {
        word.len() >= 3
            && word.chars().all(|c| c.is_ascii_uppercase() || c == '-')
            && word != "HTTP"
    }) && trimmed.split_whitespace().count() >= 2
}

/// Does the *last* entry of `text` have a response (`HTTP …`) line, and an
/// `[Asserts]` section? Flags reset at every entry-opening method line, and
/// fenced (```…```) bodies are skipped, so the end state describes the last
/// entry alone.
fn last_entry_scan(text: &str) -> (bool, bool) {
    let mut in_fence = false;
    let (mut has_http, mut has_asserts) = (false, false);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if is_method_line(trimmed) {
            (has_http, has_asserts) = (false, false);
            continue;
        }
        has_http = has_http || trimmed.starts_with("HTTP");
        has_asserts = has_asserts || trimmed == "[Asserts]";
    }
    (has_http, has_asserts)
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
        // Unreachable behind lower()'s kind-routing guard; kept total (the
        // kind doubles as the engine id) so core stays panic-free if a future
        // caller skips the guard.
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
        file_ext: None,
        scan_fragments: None,
    }];

    const PACK: &str = r#"macros:
  auth:
    params: [token]
    steps:
      - name: authenticate
        hurl: |
          POST ${url:base}/auth
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
          GET ${url:base}/search?q=${term}
          HTTP 200
          [Captures]
          recordId: jsonpath "$[0].id"
  checkHealth:
    match: the service is healthy
    steps:
      - optional: true
        hurl: |
          GET ${url:base}/health
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
            "Feature: F\n  Scenario: S\n    Given the service is healthy\n    When I search for \"Jansen\"\n    Then the response status is 200\n",
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
        config_vars: &'a BTreeMap<String, String>,
        world: &'a World,
    ) -> LowerCtx<'a> {
        LowerCtx {
            feature,
            packs,
            kind_to_engine,
            env,
            config_vars,
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
        let config_vars =
            BTreeMap::from([("url:base".to_owned(), "http://fixture.local".to_owned())]);
        let world = World::default();
        let lowered = lower(
            &scenario,
            &ctx(
                &feature,
                &packs,
                &kind_to_engine,
                &env,
                &config_vars,
                &world,
            ),
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
        assert_eq!(lowered.batches[1].steps[1].step.line, 4);
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
        let config_vars = BTreeMap::new();
        let world = World::default();
        let errs = lower(
            &scenario,
            &ctx(
                &feature,
                &packs,
                &kind_to_engine,
                &env,
                &config_vars,
                &world,
            ),
        )
        .unwrap_err();
        assert_eq!(errs[0].code, "proef::lower::then_before_when");
    }

    /// Pack validation only sees the *unresolved* pack text (`item.hurl`),
    /// which is non-blank here (`"${vars:blank}"`) — the emptiness only
    /// appears once `${vars:key}` resolves against a `proef.toml` value that
    /// happens to be `""` (a legitimate, env-conditional authoring pattern:
    /// extra asserts present in some environments, none in others). That
    /// still lowers to a zero-line `MergedAsserts` step, and the sidecar
    /// emitter must never turn a zero-line step into an inverted
    /// `.map.json` span (ADR-0010: emitted artifacts are the normative
    /// contract).
    #[test]
    fn an_expect_fragment_that_resolves_empty_does_not_invert_the_merged_span() {
        const PACK: &str = r#"macros:
  ping:
    match: the service is pinged
    steps:
      - hurl: |
          GET ${url:base}/ping
          HTTP 200
  expectBlank:
    match: nothing extra is asserted
    expect:
      - hurl: "${vars:blank}"
"#;
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
            "Feature: F\n  Scenario: S\n    Given the service is pinged\n    Then nothing extra is asserted\n",
        )
        .unwrap();
        let scenario = crate::bind::bind(&feature, &packs).unwrap().remove(0);
        let kind_to_engine: BTreeMap<String, String> =
            [("hurl".to_owned(), "hurl".to_owned())].into();
        let env = BTreeMap::new();
        let config_vars = BTreeMap::from([
            ("url:base".to_owned(), "http://fixture.local".to_owned()),
            ("vars:blank".to_owned(), String::new()),
        ]);
        let world = World::default();
        let lowered = lower(
            &scenario,
            &ctx(
                &feature,
                &packs,
                &kind_to_engine,
                &env,
                &config_vars,
                &world,
            ),
        )
        .unwrap();

        assert_eq!(lowered.batches[0].steps.len(), 2);
        let StepPayload::MergedAsserts { lines } = lowered.batches[0].steps[1].payload else {
            panic!("expected a merged-asserts step for the Then line");
        };
        assert_eq!(lines, 0, "the fragment resolved to nothing");

        let artifact = crate::emit::emit(&lowered, "t", &world).unwrap();
        for entry in &artifact.map.entries {
            let [start, end] = entry.hurl_lines;
            assert!(
                start <= end,
                "inverted span for a zero-line merge: {start}..{end}"
            );
        }
    }

    /// `${…}` resolves inside structured payload string values,
    /// recursively — keys stay literal (they are schema, not data).
    #[test]
    fn structured_payloads_resolve_placeholders_recursively() {
        const ALT_KINDS: &[StepKindSpec] = &[StepKindSpec {
            prefix: "alt",
            schema: "true",
            validate: None,
            file_ext: None,
            scan_fragments: None,
        }];
        let packs = pack::load(
            &[PackSource {
                name: "alt.yaml".into(),
                text: Arc::from(
                    "macros:\n  probe:\n    match: the alternate step runs\n    steps:\n      - name: probe\n        alt:\n          target: \"${url:base}/item\"\n          checks: [\"${url:base}\", 7]\n",
                ),
            }],
            ALT_KINDS,
        )
        .unwrap();
        let feature = crate::feature::parse(
            "t.feature",
            "Feature: F\n  Scenario: S\n    When the alternate step runs\n",
        )
        .unwrap();
        let scenario = crate::bind::bind(&feature, &packs).unwrap().remove(0);
        let kind_to_engine: BTreeMap<String, String> =
            [("alt".to_owned(), "alt".to_owned())].into();
        let env = BTreeMap::new();
        let config_vars =
            BTreeMap::from([("url:base".to_owned(), "http://fixture.local".to_owned())]);
        let world = World::default();
        let lowered = lower(
            &scenario,
            &ctx(
                &feature,
                &packs,
                &kind_to_engine,
                &env,
                &config_vars,
                &world,
            ),
        )
        .unwrap();
        let StepPayload::Structured(value) = &lowered.batches[0].steps[0].payload else {
            panic!("structured payload expected");
        };
        assert_eq!(value["target"], "http://fixture.local/item");
        assert_eq!(value["checks"][0], "http://fixture.local");
        assert_eq!(value["checks"][1], 7);
    }

    /// The Then-merge scans only the *last* entry: an earlier entry's
    /// `HTTP`/`[Asserts]` must not satisfy the check (ADR-0004 — merge into
    /// the previous entry, singular).
    #[test]
    fn expect_merge_scopes_to_the_last_entry() {
        let packs = pack::load(
            &[PackSource {
                name: "multi.yaml".into(),
                text: Arc::from(
                    "macros:\n  pair:\n    match: both calls run\n    steps:\n      - hurl: |\n          GET http://x/a\n          HTTP 200\n          [Asserts]\n          status == 200\n          GET http://x/b\n  expectStatus:\n    params: [status]\n    match: \"the response status is {status}\"\n    expect:\n      - status: \"${status}\"\n",
                ),
            }],
            KINDS,
        )
        .unwrap();
        let feature = crate::feature::parse(
            "t.feature",
            "Feature: F\n  Scenario: S\n    When both calls run\n    Then the response status is 201\n",
        )
        .unwrap();
        let scenario = crate::bind::bind(&feature, &packs).unwrap().remove(0);
        let kind_to_engine: BTreeMap<String, String> =
            [("hurl".to_owned(), "hurl".to_owned())].into();
        let env = BTreeMap::new();
        let config_vars = BTreeMap::new();
        let world = World::default();
        let lowered = lower(
            &scenario,
            &ctx(
                &feature,
                &packs,
                &kind_to_engine,
                &env,
                &config_vars,
                &world,
            ),
        )
        .unwrap();
        let StepPayload::HurlEntries(text) = &lowered.batches[0].steps[0].payload else {
            panic!("expected hurl entries");
        };
        // The second entry had no response: the merge must open its own
        // `HTTP *` + `[Asserts]` after `GET http://x/b` instead of riding on
        // the first entry's.
        let tail = text.split("GET http://x/b").nth(1).unwrap();
        assert!(tail.contains("HTTP *"), "{text}");
        assert!(tail.contains("[Asserts]"), "{text}");
        assert!(tail.contains("status == 201"), "{text}");
    }

    /// An author `[Options]` section placed after another section is extended
    /// in place — never paired with a second, injected `[Options]`, which
    /// hurl rejects as a duplicate section.
    #[test]
    fn baked_options_extend_a_late_author_options_section() {
        let retry = Some(crate::step::Retry {
            count: 2,
            interval_ms: 100,
        });
        let body =
            "GET http://x/a\n[QueryStringParams]\nq: 1\n[Options]\nverbose: true\nHTTP 200\n";
        let baked = bake_entry_options(body, retry, None);
        assert_eq!(baked.matches("[Options]").count(), 1, "{baked}");
        assert!(
            baked.contains("[Options]\nretry: 2\nretry-interval: 100ms\nverbose: true"),
            "{baked}"
        );
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
        let steps: Vec<LoweredStep> = ["hurl", "hurl", "alt", "hurl"]
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
            ("alt".to_owned(), "alt".to_owned()),
        ]
        .into();
        let batches = segment(steps, &mapping);
        let sizes: Vec<usize> = batches.iter().map(|b| b.steps.len()).collect();
        assert_eq!(sizes, vec![2, 1, 1]);
        assert_eq!(batches[1].engine.as_str(), "alt");
        // Scenario-wide ordinals — the sidecar `batch` key engines filter by.
        let indexes: Vec<usize> = batches.iter().map(|b| b.index).collect();
        assert_eq!(indexes, vec![0, 1, 2]);
    }

    /// A step's label replays the payload's `${fake:…}` values (the same
    /// occurrence) instead of minting a fresh one — so the artifact comment
    /// never names data the request didn't actually send — and that replay
    /// must leave no trace on the running counter: a later step's own fake
    /// still lands on the very next occurrence, not one the label
    /// incidentally borrowed.
    #[test]
    fn label_mirrors_the_payloads_fake_values_without_shifting_later_steps() {
        const FAKE_PACK: &str = r#"macros:
  searchFor:
    params: [term]
    match: "the operator searches for {term}"
    steps:
      - name: "search for ${term}"
        hurl: |
          GET ${url:base}/search
          [Query]
          q: ${term}
          HTTP 200
  pingFake:
    match: a fresh fake is requested
    steps:
      - hurl: |
          GET ${url:base}/ping
          [Query]
          v: ${fake:lastName}
          HTTP 200
"#;
        let packs = pack::load(
            &[PackSource {
                name: "fakes.yaml".into(),
                text: Arc::from(FAKE_PACK),
            }],
            KINDS,
        )
        .unwrap();
        let feature = crate::feature::parse(
            "t.feature",
            "Feature: F\n  Scenario: S\n    When the operator searches for ${fake:lastName}\n    Then a fresh fake is requested\n",
        )
        .unwrap();
        let scenario = crate::bind::bind(&feature, &packs).unwrap().remove(0);
        let kind_to_engine: BTreeMap<String, String> =
            [("hurl".to_owned(), "hurl".to_owned())].into();
        let env = BTreeMap::new();
        let config_vars =
            BTreeMap::from([("url:base".to_owned(), "http://fixture.local".to_owned())]);
        let world = World::default();
        let lowered = lower(
            &scenario,
            &ctx(
                &feature,
                &packs,
                &kind_to_engine,
                &env,
                &config_vars,
                &world,
            ),
        )
        .unwrap();

        // Both steps share one hurl batch (no optional/engine boundary).
        assert_eq!(lowered.batches[0].steps.len(), 2);
        let StepPayload::HurlEntries(search) = &lowered.batches[0].steps[0].payload else {
            panic!("expected hurl entries");
        };
        let label = lowered.batches[0].steps[0].label.as_deref().unwrap();

        // The payload's request resolves the scenario's first ${fake:…} —
        // occurrence 0 — and the label mirrors that exact value.
        let occurrence_0 = crate::fake::generate("run-0001", 0, "lastName").unwrap();
        assert!(
            search.contains(&format!("q: {occurrence_0}")),
            "payload: {search}"
        );
        assert!(label.contains(&occurrence_0), "label: {label}");

        // The second step's own fake continues from occurrence 1 — proof
        // the label's replay above did not silently consume it.
        let StepPayload::HurlEntries(ping) = &lowered.batches[0].steps[1].payload else {
            panic!("expected hurl entries");
        };
        let occurrence_1 = crate::fake::generate("run-0001", 1, "lastName").unwrap();
        assert!(ping.contains(&format!("v: {occurrence_1}")), "ping: {ping}");
    }

    /// The mirrored-replay above only covers a label that is an *exact*
    /// mirror of its payload. A label with *more* `${fake:…}` references
    /// than its payload still consumes real occurrences during its replay —
    /// blindly rewinding the counter back to wherever the payload/guard left
    /// off would hand those consumed occurrences back out to a later step,
    /// which would then display a value the label already showed.
    #[test]
    fn label_with_more_fakes_than_its_payload_does_not_leak_occurrences_to_later_steps() {
        const FAKE_PACK: &str = r#"macros:
  unmirroredLabel:
    match: a label mentions more fakes than its payload
    steps:
      - name: "${fake:lastName} vs ${fake:lastName}"
        hurl: |
          GET ${url:base}/probe
          [Query]
          q: ${fake:lastName}
          HTTP 200
  pingFake:
    match: a fresh fake is requested
    steps:
      - hurl: |
          GET ${url:base}/ping
          [Query]
          v: ${fake:lastName}
          HTTP 200
"#;
        let packs = pack::load(
            &[PackSource {
                name: "unmirrored.yaml".into(),
                text: Arc::from(FAKE_PACK),
            }],
            KINDS,
        )
        .unwrap();
        let feature = crate::feature::parse(
            "t.feature",
            "Feature: F\n  Scenario: S\n    When a label mentions more fakes than its payload\n    Then a fresh fake is requested\n",
        )
        .unwrap();
        let scenario = crate::bind::bind(&feature, &packs).unwrap().remove(0);
        let kind_to_engine: BTreeMap<String, String> =
            [("hurl".to_owned(), "hurl".to_owned())].into();
        let env = BTreeMap::new();
        let config_vars =
            BTreeMap::from([("url:base".to_owned(), "http://fixture.local".to_owned())]);
        let world = World::default();
        let lowered = lower(
            &scenario,
            &ctx(
                &feature,
                &packs,
                &kind_to_engine,
                &env,
                &config_vars,
                &world,
            ),
        )
        .unwrap();

        // Both steps share one hurl batch.
        assert_eq!(lowered.batches[0].steps.len(), 2);
        let label = lowered.batches[0].steps[0].label.as_deref().unwrap();
        let StepPayload::HurlEntries(ping) = &lowered.batches[0].steps[1].payload else {
            panic!("expected hurl entries");
        };

        // The label's payload uses occurrence 0; the label itself has a
        // *second* `${fake:lastName}` the payload never had, which must
        // consume occurrence 1 for real.
        let occurrence_0 = crate::fake::generate("run-0001", 0, "lastName").unwrap();
        let occurrence_1 = crate::fake::generate("run-0001", 1, "lastName").unwrap();
        assert!(label.contains(&occurrence_0), "label: {label}");
        assert!(label.contains(&occurrence_1), "label: {label}");

        // The next step's own, independent fake must continue *past* what
        // the label already consumed — occurrence 2 — never occurrence 1,
        // which the label already displayed.
        let occurrence_2 = crate::fake::generate("run-0001", 2, "lastName").unwrap();
        assert!(
            !ping.contains(&format!("v: {occurrence_1}")),
            "the next step's fake reused an occurrence the label already \
             displayed: {ping}"
        );
        assert!(ping.contains(&format!("v: {occurrence_2}")), "ping: {ping}");
    }
}
