//! Pack validation passes 1–8 (TECH-SPEC §4.1).
//!
//! 1. `match:` guard rails (literal anchor, no adjacent captures, balanced
//!    braces, declared params) · 2. params/defaults coverage · 3. duplicate
//!    macro names across packs (in `mod.rs`, at insertion) · 4. `use:` cycle +
//!    depth ≤ [`MAX_USE_DEPTH`] · 5. unknown/missing `with:` keys ·
//!    6. finite-retry lint (typed `retry:` plus a raw-block scan for infinite
//!    hurl `retry`/`repeat` options, and the same scan's refusal of an option
//!    declared both in `[Options]` and as its typed twin) · 7.
//!    probe-instantiation parse of payload
//!    blocks via the claiming engine's [`StepKindSpec::validate`] hook ·
//!    8. every payload kind is claimed by a registered engine.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::locate;
use super::{
    ExpectItem, Macro, MacroBody, MacroStep, MacroStepKind, PackSet, PackSource, PayloadForm,
    RawMacro, RawStep,
};
use crate::diag::Diag;
use crate::engine::StepKindSpec;
use crate::matcher;
use crate::resolve::{self, Resolution, ResolveCtx, ResolveMode};
use crate::step::Retry;
use crate::world::World;

/// Maximum `use:` nesting depth (TECH-SPEC §4.1 pass 4).
pub const MAX_USE_DEPTH: usize = 32;

/// Normalize one raw macro into a [`Macro`], emitting structural
/// diagnostics (passes 1, 2, and the per-step shape rules) along the way.
/// Returns `None` only when the macro is too malformed to keep.
// One cohesive listing of the body-shape rules; splitting hides the order.
#[allow(clippy::too_many_lines)]
pub(crate) fn normalize_macro(
    name: &str,
    raw: &RawMacro,
    pack_name: &str,
    source: &PackSource,
    diags: &mut Vec<Diag>,
) -> Option<Macro> {
    let span = locate::macro_span(&source.text, name);
    let match_span = locate::match_span(&source.text, name);
    let at = |diag: Diag| {
        diag.with_source(source.name.clone(), Arc::clone(&source.text))
            .maybe_span(span)
    };

    // Pass 1: match guard rails.
    if let Some(pattern) = &raw.match_ {
        for problem in matcher::pattern_problems(pattern, &raw.params) {
            diags.push(at(Diag::error(
                problem.code(),
                format!("macro `{name}`: {problem}"),
            )));
        }
    }

    // Pass 2: defaults must name declared params.
    for default_key in raw.defaults.keys() {
        if !raw.params.contains(default_key) {
            let suggestion = matcher::closest(default_key, raw.params.iter().map(String::as_str))
                .map(|p| format!(" — did you mean `{p}`?"))
                .unwrap_or_default();
            diags.push(at(Diag::error(
                "proef::pack::default_not_param",
                format!(
                    "macro `{name}`: default `{default_key}` is not a declared param{suggestion}"
                ),
            )));
        }
    }

    // Body shape: steps XOR expect.
    let body = match (&raw.steps.is_empty(), &raw.expect) {
        (false, Some(_)) => {
            diags.push(at(Diag::error(
                "proef::pack::steps_and_expect",
                format!("macro `{name}` has both `steps:` and `expect:` — a macro is a request sequence or an assert-only macro, not both"),
            )));
            return None;
        }
        (true, None) => {
            diags.push(at(Diag::error(
                "proef::pack::empty_macro",
                format!("macro `{name}` has neither `steps:` nor `expect:`"),
            )));
            return None;
        }
        (true, Some(items)) => {
            let mut expect = Vec::new();
            // Positional pairing with `hurl:` lines in source order: only
            // items that carry the key produce one (assert-only macros have
            // no `steps:`, so every `hurl:` line in the block is an expect
            // item's), so the ordinal advances only when `item.hurl` is `Some`.
            // The line scanner only recognises block-style `key:` lines
            // (`locate::key_line_spans`), so a flow-style item (`- {hurl: …}`)
            // parses to `Some` but contributes no line — exactly the hazard
            // `analyze::index_use_refs` already guards for `use:` lines. Same
            // fix: when the counts disagree the pairing can't be trusted, so
            // every item in this macro falls back to the macro's own span
            // instead of risking an ordinal-shifted wrong line.
            let hurl_spans = locate::expect_hurl_line_spans(&source.text, name);
            let hurl_key_count = items.iter().filter(|item| item.hurl.is_some()).count();
            let spans_reliable = hurl_spans.len() == hurl_key_count;
            let mut hurl_ordinal = 0usize;
            for (index, item) in items.iter().enumerate() {
                let has_hurl_key = item.hurl.is_some();
                // A blank or whitespace-only `hurl:` block scalar carries no
                // assert lines — same as omitting the key outright (and, left
                // unrejected, lowers to a zero-line merged-asserts step that
                // underflows the sidecar span arithmetic).
                let fragment_is_blank = item
                    .hurl
                    .as_deref()
                    .is_none_or(|fragment| fragment.trim().is_empty());
                if item.status.is_none() && fragment_is_blank {
                    let fragment_span = (spans_reliable && has_hurl_key)
                        .then(|| hurl_spans.get(hurl_ordinal).copied())
                        .flatten();
                    diags.push(
                        at(Diag::error(
                            "proef::pack::empty_expect",
                            format!("macro `{name}` expect item {index} asserts nothing — give it `status:` and/or `hurl:` assert lines"),
                        ))
                        .maybe_span(fragment_span)
                        .with_help("an `expect:` item must carry at least one assert line, from `status:` and/or non-blank `hurl:` content"),
                    );
                    if has_hurl_key {
                        hurl_ordinal += 1;
                    }
                    continue;
                }
                if has_hurl_key {
                    hurl_ordinal += 1;
                }
                expect.push(ExpectItem {
                    status: item.status.clone(),
                    fragment: item.hurl.clone(),
                });
            }
            MacroBody::Expect(expect)
        }
        (false, None) => {
            let mut steps = Vec::new();
            for (index, step) in raw.steps.iter().enumerate() {
                if let Some(step) = normalize_step(name, index, step, &at, diags) {
                    steps.push(step);
                }
            }
            MacroBody::Steps(steps)
        }
    };

    Some(Macro {
        name: name.to_owned(),
        pack: pack_name.to_owned(),
        params: raw.params.clone(),
        defaults: raw.defaults.clone(),
        pattern: raw.match_.clone(),
        description: raw.description.clone(),
        tags: raw.tags.clone(),
        body,
        bind: raw.bind.clone(),
        source: Arc::clone(&source.text),
        span,
        match_span,
    })
}

/// Normalize one raw step, emitting per-step shape diagnostics.
// One cohesive listing of the step shape rules; splitting hides the order.
#[allow(clippy::too_many_lines)]
fn normalize_step(
    macro_name: &str,
    index: usize,
    raw: &RawStep,
    at: &impl Fn(Diag) -> Diag,
    diags: &mut Vec<Diag>,
) -> Option<MacroStep> {
    // saveAs targets: only `global` exists (ADR-0005).
    let mut save_as = BTreeMap::new();
    if let Some(targets) = &raw.save_as {
        for (capture, target) in targets {
            if target == "global" {
                save_as.insert(capture.clone(), target.clone());
            } else {
                diags.push(at(Diag::error(
                    "proef::pack::bad_save_target",
                    format!("macro `{macro_name}` step {index}: `saveAs: {{ {capture}: {target} }}` — the only target is `global`"),
                )));
            }
        }
    }

    // Finite-retry lint, typed half (pass 6): count 0 is pointless.
    let retry = match &raw.retry {
        Some(r) if i64::from(r.count) > MAX_COUNT => {
            diags.push(at(Diag::error(
                "proef::pack::retry_not_finite",
                format!(
                    "macro `{macro_name}` step {index}: `retry.count` {} is budget-hostile — the cap is {MAX_COUNT}",
                    r.count
                ),
            )));
            None
        }
        Some(r) if r.count == 0 => {
            diags.push(at(Diag::error(
                "proef::pack::retry_not_finite",
                format!("macro `{macro_name}` step {index}: `retry.count` must be ≥ 1"),
            )));
            None
        }
        Some(r) => Some(Retry {
            count: r.count,
            interval_ms: r.interval_ms,
        }),
        None => None,
    };

    // `bind:` supplies a *fragment's* `{{names}}`. On an inline step there is
    // nothing to supply them to — `${…}` splices at lower time — so accepting it
    // there would silently ignore what the author wrote.
    if !raw.bind.is_empty() && raw.ref_.is_none() {
        diags.push(at(Diag::error(
            "proef::pack::bind_without_ref",
            format!(
                "macro `{macro_name}` step {index}: `bind:` supplies a fragment's `{{{{…}}}}` variables, so it needs a `ref:` — an inline `hurl:` block takes `${{…}}` instead"
            ),
        )));
    }

    let kind = if let Some(target) = &raw.ref_ {
        // Body form: a step is exactly one of `hurl:`, `use:`, `ref:`.
        if !raw.payload.is_empty() || raw.use_.is_some() {
            let other = if raw.use_.is_some() {
                "use:"
            } else {
                "a payload"
            };
            diags.push(at(Diag::error(
                "proef::pack::body_form_conflict",
                format!(
                    "macro `{macro_name}` step {index}: a step is either `ref:` or {other}, not both"
                ),
            )));
            return None;
        }
        if raw.with.is_some() {
            diags.push(at(Diag::error(
                "proef::pack::with_without_use",
                format!("macro `{macro_name}` step {index}: `with:` only accompanies `use:`"),
            )));
        }
        // Unlike `use:`, a `ref:` step *does* take the step modifiers: it is one
        // request of this macro's own, not an inlining of somebody else's steps.
        MacroStepKind::Ref {
            target: target.clone(),
        }
    } else {
        match (&raw.use_, raw.payload.len()) {
            (Some(target), 0) => {
                if raw.optional || raw.when.is_some() || retry.is_some() || !save_as.is_empty() {
                    diags.push(at(Diag::error(
                    "proef::pack::use_with_modifiers",
                    format!("macro `{macro_name}` step {index}: `use:` steps take only `with:` (and `name:`) — modifiers belong on the target macro's steps"),
                )));
                }
                MacroStepKind::Use {
                    target: target.clone(),
                    with: raw.with.clone().unwrap_or_default(),
                }
            }
            (Some(_), _) => {
                diags.push(at(Diag::error(
                "proef::pack::use_with_payload",
                format!("macro `{macro_name}` step {index}: a step is either `use:` or a payload, not both"),
            )));
                return None;
            }
            (None, 0) => {
                diags.push(at(Diag::error(
                "proef::pack::empty_step",
                format!(
                    "macro `{macro_name}` step {index} has no payload (`hurl: |…`), no `ref:`, and no `use:`"
                ),
            )));
                return None;
            }
            (None, 1) => {
                if raw.with.is_some() {
                    diags.push(at(Diag::error(
                        "proef::pack::with_without_use",
                        format!(
                            "macro `{macro_name}` step {index}: `with:` only accompanies `use:`"
                        ),
                    )));
                }
                let (kind_key, value) = raw
                    .payload
                    .iter()
                    .next()
                    .map(|(k, v)| (k.clone(), v.clone()))?;
                let payload = match value {
                    serde_norway::Value::String(text) => PayloadForm::Raw(text),
                    other => PayloadForm::Structured(
                        serde_json::to_value(&other).unwrap_or(serde_json::Value::Null),
                    ),
                };
                MacroStepKind::Payload {
                    kind: kind_key,
                    payload,
                }
            }
            (None, _) => {
                let keys: Vec<&str> = raw.payload.keys().map(String::as_str).collect();
                diags.push(at(Diag::error(
                    "proef::pack::multiple_payloads",
                    format!(
                        "macro `{macro_name}` step {index} has {} payload keys ({}) — one per step",
                        keys.len(),
                        keys.join(", ")
                    ),
                )));
                return None;
            }
        }
    };

    // Delay cap (pass 6, typed half): a pause no budget can absorb is a
    // hang, not a test (ADR-0007).
    let delay_ms = match raw.delay {
        Some(ms) if ms > MAX_DELAY_MS => {
            diags.push(at(Diag::error(
                "proef::pack::delay_unbounded",
                format!(
                    "macro `{macro_name}` step {index}: `delay: {ms}` exceeds the {MAX_DELAY_MS} ms (1 hour) cap"
                ),
            )));
            None
        }
        other => other,
    };

    Some(MacroStep {
        name: raw.name.clone(),
        delay_ms,
        kind,
        optional: raw.optional,
        when: raw.when.clone(),
        retry,
        save_as,
        bind: raw.bind.clone(),
    })
}

/// Pass-6 caps (ADR-0007): counts or pauses above these cannot be absorbed
/// by any batch budget — they are hangs, not tests.
const MAX_COUNT: i64 = 10_000;
const MAX_DELAY_MS: u64 = 3_600_000;

/// Parse a raw `[Options]` duration value (`3000`, `500ms`, `3s`, `2m`) into
/// milliseconds. Templates (`{{…}}`) and anything else non-numeric return
/// `None` — the runtime budget still bounds those.
fn raw_duration_ms(value: &str) -> Option<u64> {
    let value = value.trim();
    let (number, unit_ms) = if let Some(n) = value.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = value.strip_suffix('s') {
        (n, 1000)
    } else if let Some(n) = value.strip_suffix('m') {
        (n, 60_000)
    } else {
        (value, 1)
    };
    number.trim().parse::<u64>().ok()?.checked_mul(unit_ms)
}

/// Passes over the complete macro set: `use:` graph (4, 5), payload kinds (8),
/// raw-block finite-retry scan (6), and engine probe validation (7).
pub(crate) fn run_cross_macro_passes(set: &PackSet, kinds: &[StepKindSpec], diags: &mut Vec<Diag>) {
    for macro_ in set.macros.values() {
        let at = |diag: Diag| {
            diag.with_source(macro_.pack.clone(), Arc::clone(&macro_.source))
                .maybe_span(macro_.span)
        };
        let MacroBody::Steps(steps) = &macro_.body else {
            continue;
        };

        let mut payload_ordinals: BTreeMap<&str, usize> = BTreeMap::new();
        for (index, step) in steps.iter().enumerate() {
            match &step.kind {
                MacroStepKind::Use { target, with } => {
                    use_target_passes(set, macro_, index, target, with, &at, diags);
                }
                MacroStepKind::Ref { target } => {
                    ref_target_passes(set, macro_, index, step, target, &at, diags);
                }
                MacroStepKind::Payload { kind, payload } => {
                    let ordinal = *payload_ordinals
                        .entry(kind.as_str())
                        .and_modify(|n| *n += 1)
                        .or_insert(0);
                    payload_passes(
                        macro_, index, kind, step, payload, ordinal, kinds, &at, diags,
                    );
                }
            }
        }
    }

    use_graph_passes(set, diags);
}

/// Target existence for one `ref:`, plus the retry double-declaration check
/// against the fragment's own `[Options]` — the same rule `lint_raw_options`
/// applies to an inline block, so the two body forms behave identically rather
/// than differing by where the hurl text happens to live.
fn ref_target_passes(
    set: &PackSet,
    macro_: &Macro,
    index: usize,
    step: &MacroStep,
    target: &str,
    at: &impl Fn(Diag) -> Diag,
    diags: &mut Vec<Diag>,
) {
    let Some(fragment) = set.find_fragment(target) else {
        let suggestion = matcher::closest(
            target.rsplit('#').next().unwrap_or(target),
            set.fragments.keys().map(String::as_str),
        )
        .map(|f| format!(" — did you mean `{f}`?"))
        .unwrap_or_default();
        diags.push(
            at(Diag::error(
                "proef::pack::unknown_ref",
                format!(
                    "macro `{}` step {index}: `ref: {target}` names no loaded fragment{suggestion}",
                    macro_.name
                ),
            ))
            .with_help(if set.fragments.is_empty() {
                "no fragment files were loaded — set `[run] fragments` in proef.toml to the \
                 directory holding them"
            } else {
                "a fragment is one hurl entry marked `# @proef <name>` in a scanned file"
            }),
        );
        return;
    };
    // Every option family the step can also set, not just retry: `delay:` bakes
    // into the same `[Options]` section through the same code path, so leaving
    // it unchecked reproduces exactly the silent last-wins the inline half of
    // this rule exists to refuse.
    for (family, declared_by_step) in [
        ("retry", step.retry.is_some()),
        ("delay", step.delay_ms.is_some()),
    ] {
        if declared_by_step && fragment.declared_options.iter().any(|o| o == family) {
            diags.push(at(Diag::error(
                "proef::pack::option_declared_twice",
                format!(
                    "macro `{}` step {index}: `{family}` is declared twice — in fragment `{}` (`{}` line {}) and as this step's own `{family}:`",
                    macro_.name, fragment.name, fragment.file, fragment.line
                ),
            )).with_help(
                "an entry carries one policy per option — delete whichever of the two is not authoritative",
            ));
        }
    }
}

/// Pass 4 (target existence) + pass 5 (`with:` key coverage) for one `use:`.
fn use_target_passes(
    set: &PackSet,
    macro_: &Macro,
    index: usize,
    target: &str,
    with: &BTreeMap<String, String>,
    at: &impl Fn(Diag) -> Diag,
    diags: &mut Vec<Diag>,
) {
    let Some(target_macro) = set.find_use_target(target) else {
        let suggestion = matcher::closest(
            target.rsplit('#').next().unwrap_or(target),
            set.macros.keys().map(String::as_str),
        )
        .map(|m| format!(" — did you mean `{m}`?"))
        .unwrap_or_default();
        diags.push(at(Diag::error(
            "proef::pack::unknown_use",
            format!(
                "macro `{}` step {index}: `use: {target}` names no loaded macro{suggestion}",
                macro_.name
            ),
        )));
        return;
    };

    for key in with.keys() {
        if !target_macro.params.contains(key) {
            let suggestion = matcher::closest(key, target_macro.params.iter().map(String::as_str))
                .map(|p| format!(" — did you mean `{p}`?"))
                .unwrap_or_default();
            diags.push(at(Diag::error(
                "proef::pack::unknown_with_key",
                format!(
                    "macro `{}` step {index}: `with:` key `{key}` is not a param of `{}`{suggestion}",
                    macro_.name, target_macro.name
                ),
            )));
        }
    }
    for param in &target_macro.params {
        if !with.contains_key(param) && !target_macro.defaults.contains_key(param) {
            diags.push(at(Diag::error(
                "proef::pack::missing_use_param",
                format!(
                    "macro `{}` step {index}: `use: {}` needs `with: {{ {param}: … }}` (no default exists)",
                    macro_.name, target_macro.name
                ),
            )));
        }
    }
}

/// Passes 8 (kind claimed), 6 (raw-block infinite retry/repeat), and 7
/// (engine probe validation) for one payload step.
#[allow(clippy::too_many_arguments)]
fn payload_passes(
    macro_: &Macro,
    index: usize,
    kind: &str,
    step: &MacroStep,
    payload: &PayloadForm,
    ordinal: usize,
    kinds: &[StepKindSpec],
    at: &impl Fn(Diag) -> Diag,
    diags: &mut Vec<Diag>,
) {
    // Pass 8: the kind must be claimed by a registered engine.
    let Some(spec) = kinds.iter().find(|s| s.prefix == kind) else {
        let suggestion = matcher::closest(kind, kinds.iter().map(|s| s.prefix))
            .map(|p| format!(" — did you mean `{p}:`?"))
            .unwrap_or_default();
        diags.push(at(Diag::error(
            "proef::pack::unknown_step_kind",
            format!(
                "macro `{}` step {index}: step kind `{kind}:` is not claimed by any registered engine{suggestion}",
                macro_.name
            ),
        )));
        return;
    };

    let text = match payload {
        PayloadForm::Raw(text) => text,
        PayloadForm::Structured(value) => {
            // Structured payload (ADR-0004): hand its canonical JSON text to
            // the engine's validator — the same load-time gate raw payloads
            // get. `${…}` placeholders may remain inside strings; validators
            // check shape, not values.
            if let Some(validate) = spec.validate
                && let Ok(json) = serde_json::to_string(value)
                && let Err(err) = validate(&json)
            {
                diags.push(at(Diag::error(
                    "proef::pack::payload_invalid",
                    format!(
                        "macro `{}` step {index}: `{kind}:` payload is invalid — {}",
                        macro_.name, err.message
                    ),
                )));
            }
            return;
        }
    };

    lint_raw_options(macro_, index, kind, step, ordinal, text, at, diags);

    // Pass 7: probe-instantiation parse via the engine's validator.
    let Some(validate) = spec.validate else {
        return;
    };
    match probe_lower(macro_, text) {
        Err(err) => {
            diags.push(at(Diag::error(
                "proef::pack::bad_reference",
                format!("macro `{}` step {index}: {err}", macro_.name),
            )));
        }
        Ok(candidates) => {
            let mut first_error = None;
            let mut passed = false;
            for candidate in &candidates {
                match validate(candidate) {
                    Ok(()) => {
                        passed = true;
                        break;
                    }
                    Err(err) => first_error = first_error.or(Some(err)),
                }
            }
            if !passed && let Some(err) = first_error {
                diags.push(
                        at(Diag::error(
                            "proef::pack::invalid_hurl",
                            format!(
                                "macro `{}` step {index}: payload does not parse: {} (payload line {}, column {})",
                                macro_.name, err.message, err.line, err.column
                            ),
                        ))
                        .maybe_span(locate::payload_line_span(
                            &macro_.source,
                            &macro_.name,
                            kind,
                            ordinal,
                            err.line,
                        )),
                    );
            }
        }
    }
}

/// Pass 6 (raw half): hurl allows infinite `retry`/`repeat` and unbounded
/// `delay` — parse numeric raw-option values and reject what no budget can
/// absorb (ADR-0007). Non-numeric values (`{{…}}` templates) pass: the
/// runtime batch budget still bounds those.
///
/// Also the double-declaration check: an option set *both* in the block's own
/// `[Options]` and as its YAML twin (`retry:` / `delay:`). Lowering extends an
/// author's section rather than opening a second one, and hurl resolves
/// duplicate options last-wins, so the raw value quietly beat the typed one —
/// the pack said one thing and the run did another. Only `[Options]`-section
/// lines count: a request header may legitimately be named `retry`, and this
/// is a hard error, so it must not fire on one.
// Both checks read the same one-pass scan; splitting them would walk the block
// twice and duplicate the fence and section bookkeeping.
#[allow(clippy::too_many_arguments)]
fn lint_raw_options(
    macro_: &Macro,
    index: usize,
    kind: &str,
    step: &MacroStep,
    ordinal: usize,
    text: &str,
    at: &impl Fn(Diag) -> Diag,
    diags: &mut Vec<Diag>,
) {
    let mut in_fence = false;
    let mut in_options = false;
    // One report per option family is enough to act on; a block whose every
    // entry repeats the clash would otherwise bury the step in duplicates.
    let (mut said_retry, mut said_delay) = (false, false);
    for (line_no, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue; // fenced body data — a literal `retry: -1` is payload, not an option
        }
        // A section runs until the next section header or the next entry.
        if trimmed.starts_with('[') {
            in_options = trimmed == "[Options]";
        } else if crate::lower::is_method_line(trimmed) {
            in_options = false;
        }
        let mut reject = |code: &'static str, message: String| {
            diags.push(
                at(Diag::error(
                    code,
                    format!("macro `{}` step {index}: {message}", macro_.name),
                ))
                .maybe_span(locate::payload_line_span(
                    &macro_.source,
                    &macro_.name,
                    kind,
                    ordinal,
                    line_no + 1,
                )),
            );
        };
        for option in ["retry", "repeat"] {
            if let Some(value) = trimmed.strip_prefix(&format!("{option}:")) {
                match value.trim().parse::<i64>() {
                    Ok(-1) => reject(
                        "proef::pack::retry_not_finite",
                        format!(
                            "`{option}: -1` is infinite — budgets require a finite count (ADR-0007)"
                        ),
                    ),
                    Ok(n) if n > MAX_COUNT => reject(
                        "proef::pack::retry_not_finite",
                        format!("`{option}: {n}` is budget-hostile — the cap is {MAX_COUNT}"),
                    ),
                    _ => {}
                }
            }
        }
        if let Some(value) = trimmed.strip_prefix("delay:")
            && let Some(ms) = raw_duration_ms(value)
            && ms > MAX_DELAY_MS
        {
            reject(
                "proef::pack::delay_unbounded",
                format!(
                    "`delay: {}` exceeds the {MAX_DELAY_MS} ms (1 hour) cap",
                    value.trim()
                ),
            );
        }
        if !in_options {
            continue;
        }
        let is_retry = trimmed.starts_with("retry:") || trimmed.starts_with("retry-interval:");
        let (option, twinned, said) = if is_retry {
            ("retry", step.retry.is_some(), &mut said_retry)
        } else if trimmed.starts_with("delay:") {
            ("delay", step.delay_ms.is_some(), &mut said_delay)
        } else {
            continue;
        };
        if twinned && !*said {
            *said = true;
            diags.push(
                at(Diag::error(
                    "proef::pack::option_declared_twice",
                    format!(
                        "macro `{}` step {index}: `{option}` is declared twice — here in `[Options]`, and as the step's own `{option}:`",
                        macro_.name
                    ),
                ))
                .maybe_span(locate::payload_line_span(
                    &macro_.source,
                    &macro_.name,
                    kind,
                    ordinal,
                    line_no + 1,
                ))
                .with_help(
                    "an entry carries one policy per option — delete whichever of the two is not authoritative",
                ),
            );
        }
    }
}

/// Probe-lower a payload: substitute placeholder params and resolve in
/// [`ResolveMode::Probe`]. Engine payload grammar is positional (URLs need a
/// scheme or template, statuses need digits), so two placeholder shapes are
/// tried — template-form first, numeric second; a block failing both is
/// genuinely malformed. The authoritative check is M2's parse of the *real*
/// emitted artifact; this pass is early feedback at pack-authoring time.
fn probe_lower(macro_: &Macro, text: &str) -> Result<Vec<String>, resolve::ResolveError> {
    let world = World::default();
    let empty = BTreeMap::new();
    let mut candidates = Vec::new();
    for placeholder in ["{{probe}}", "1"] {
        let args: BTreeMap<String, String> = macro_
            .params
            .iter()
            .map(|p| (p.clone(), placeholder.to_owned()))
            .collect();
        let ctx = ResolveCtx {
            args: &args,
            defaults: &macro_.defaults,
            env: &empty,
            config_vars: &empty,
            run_id: "probe-run",
            world: &world,
            mode: ResolveMode::Probe,
        };
        // A fresh probe, not a scenario — the occurrence counter starts at 0
        // for each placeholder candidate; this pass only checks grammar.
        let mut fakes = 0;
        let Resolution { text, .. } = resolve::resolve(text, &ctx, &mut fakes)?;
        candidates.push(text);
    }
    Ok(candidates)
}

/// Pass 4: `use:` reference cycles and depth over the whole macro graph.
/// Three-color DFS with memoized chain depths — node-linear where a per-root
/// path enumeration goes exponential on shared (multi-edge) `use:` targets.
fn use_graph_passes(set: &PackSet, diags: &mut Vec<Diag>) {
    let mut colors: BTreeMap<&str, Color> = BTreeMap::new();
    let mut chains: BTreeMap<&str, usize> = BTreeMap::new();
    for macro_ in set.macros.values() {
        visit_uses(set, macro_, &mut colors, &mut chains, diags);
    }
    for macro_ in set.macros.values() {
        if chains.get(macro_.name.as_str()).copied().unwrap_or(1) <= MAX_USE_DEPTH {
            continue;
        }
        let path = longest_use_path(set, macro_, &chains);
        // The first macro past the limit carries the diagnostic — the same
        // attribution the depth-33 stack frame had under the walking scheme.
        let Some(deep) = path.get(MAX_USE_DEPTH).copied() else {
            continue; // depth reached only through a cycle — already reported
        };
        diags.push(
            Diag::error(
                "proef::pack::use_too_deep",
                format!(
                    "`use:` nesting exceeds depth {MAX_USE_DEPTH} (via `{}`)",
                    path[..=MAX_USE_DEPTH]
                        .iter()
                        .map(|m| m.name.as_str())
                        .collect::<Vec<_>>()
                        .join("` → `")
                ),
            )
            .with_source(deep.pack.clone(), Arc::clone(&deep.source))
            .maybe_span(deep.span),
        );
    }
}

#[derive(Clone, Copy)]
enum Color {
    Gray,
    Black,
}

enum Frame<'a> {
    Enter(&'a Macro),
    Exit(&'a Macro),
}

/// Resolvable `use:` targets of a macro, in step order.
fn use_targets<'a>(set: &'a PackSet, macro_: &'a Macro) -> Vec<&'a Macro> {
    let MacroBody::Steps(steps) = &macro_.body else {
        return Vec::new();
    };
    steps
        .iter()
        .filter_map(|step| {
            let MacroStepKind::Use { target, .. } = &step.kind else {
                return None;
            };
            set.find_use_target(target) // unresolved: reported by use_target_passes
        })
        .collect()
}

/// Iterative DFS from one root (explicit frames — the walk must stay
/// stack-safe on arbitrarily long chains): gray while on the path, black once
/// the longest downstream chain is memoized in `chains`. A `use:` edge to a
/// gray macro closes a cycle.
fn visit_uses<'a>(
    set: &'a PackSet,
    root: &'a Macro,
    colors: &mut BTreeMap<&'a str, Color>,
    chains: &mut BTreeMap<&'a str, usize>,
    diags: &mut Vec<Diag>,
) {
    if colors.contains_key(root.name.as_str()) {
        return;
    }
    let mut work = vec![Frame::Enter(root)];
    let mut path: Vec<&'a Macro> = Vec::new();
    while let Some(frame) = work.pop() {
        match frame {
            Frame::Enter(m) => {
                if colors.contains_key(m.name.as_str()) {
                    continue; // finished via an earlier multi-edge
                }
                colors.insert(m.name.as_str(), Color::Gray);
                path.push(m);
                work.push(Frame::Exit(m));
                for next in use_targets(set, m).into_iter().rev() {
                    match colors.get(next.name.as_str()).copied() {
                        Some(Color::Gray) => report_use_cycle(&path, next, diags),
                        Some(Color::Black) => {} // chain read at Exit
                        None => work.push(Frame::Enter(next)),
                    }
                }
            }
            Frame::Exit(m) => {
                path.pop();
                let chain = 1 + use_targets(set, m)
                    .into_iter()
                    .filter_map(|next| chains.get(next.name.as_str()).copied())
                    .max()
                    .unwrap_or(0);
                colors.insert(m.name.as_str(), Color::Black);
                chains.insert(m.name.as_str(), chain);
            }
        }
    }
}

/// Render one cycle: rotate the gray-path ring to start at its
/// lexicographically-first member; the member whose `use:` closes back to it
/// carries the diagnostic.
fn report_use_cycle(path: &[&Macro], next: &Macro, diags: &mut Vec<Diag>) {
    let pos = path.iter().position(|m| m.name == next.name).unwrap_or(0);
    let ring = &path[pos..];
    let min_ix = ring
        .iter()
        .enumerate()
        .min_by_key(|(_, m)| m.name.as_str())
        .map_or(0, |(i, _)| i);
    let rotated: Vec<&Macro> = ring[min_ix..]
        .iter()
        .chain(&ring[..min_ix])
        .copied()
        .collect();
    let Some(closer) = rotated.last().copied() else {
        return;
    };
    let names: Vec<&str> = rotated.iter().map(|m| m.name.as_str()).collect();
    diags.push(
        Diag::error(
            "proef::pack::use_cycle",
            format!("`use:` cycle: `{}` → `{}`", names.join("` → `"), names[0]),
        )
        .with_source(closer.pack.clone(), Arc::clone(&closer.source))
        .maybe_span(closer.span),
    );
}

/// Follow max-chain children from `from` — in a cycle-free graph this is a
/// longest `use:` chain, matching the memoized depth that triggered the
/// diagnostic.
fn longest_use_path<'a>(
    set: &'a PackSet,
    from: &'a Macro,
    chains: &BTreeMap<&'a str, usize>,
) -> Vec<&'a Macro> {
    let mut path = vec![from];
    while path.len() <= MAX_USE_DEPTH {
        let cur = path[path.len() - 1];
        let next = use_targets(set, cur)
            .into_iter()
            .filter(|cand| !path.iter().any(|m| m.name == cand.name))
            .max_by_key(|cand| chains.get(cand.name.as_str()).copied().unwrap_or(1));
        match next {
            Some(next) => path.push(next),
            None => break,
        }
    }
    path
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use crate::diag::FrontError;
    use crate::engine::{PayloadProbeError, StepKindSpec};
    use crate::pack::{self, PackSource};

    fn deny(_json: &str) -> Result<(), PayloadProbeError> {
        Err(PayloadProbeError {
            line: 1,
            column: 1,
            message: "unknown alt verb".into(),
        })
    }

    const KINDS: &[StepKindSpec] = &[StepKindSpec {
        prefix: "alt",
        schema: "true",
        validate: Some(deny),
        fragments: None,
    }];

    /// A whitespace-only `hurl:` fragment with no `status:` carries no assert
    /// line — lowering it would produce a zero-line merged-asserts step, which
    /// underflows the sidecar's `start + lines - 1` span arithmetic. Pack
    /// validation rejects it before that can happen, spanning the `hurl:`
    /// line itself rather than the whole macro.
    #[test]
    fn whitespace_only_expect_fragment_is_rejected() {
        let source = PackSource {
            name: "expect.yaml".into(),
            text: Arc::from(
                "macros:\n  empty:\n    match: nothing binds this\n    expect:\n      - hurl: |\n\n",
            ),
        };
        let err = pack::load(&[source], &[], KINDS).unwrap_err();
        let FrontError::Diagnostics(diags) = err else {
            panic!("diagnostics expected");
        };
        let diag = diags
            .iter()
            .find(|d| d.code == "proef::pack::empty_expect")
            .unwrap_or_else(|| panic!("expected proef::pack::empty_expect in {diags:?}"));
        assert!(diag.help.is_some(), "a remediation hint is expected");
        let text = diag.source_text.as_ref().unwrap();
        let span = diag
            .span
            .unwrap_or_else(|| panic!("expected a span: {diag:?}"));
        assert_eq!(
            &text[span.start..span.end],
            "hurl: |",
            "span should land on the empty fragment's `hurl:` line, not the whole macro"
        );
    }

    /// A flow-style item (`- {status: …, hurl: …}`) parses `item.hurl` to
    /// `Some`, but the block-style line scanner behind
    /// `locate::expect_hurl_line_spans` cannot see it and contributes no
    /// span — the same hazard `analyze::index_use_refs` already guards for
    /// `use:` lines. Pin that the guard here falls back to the macro's own
    /// span for the whole macro, rather than pairing a later blank item onto
    /// a wrong, ordinal-shifted line.
    #[test]
    fn flow_style_hurl_key_falls_back_to_the_macro_span() {
        let text: Arc<str> = Arc::from(concat!(
            "macros:\n",
            "  mixed:\n",
            "    match: nothing binds this\n",
            "    expect:\n",
            "      - {status: \"200\", hurl: 'jsonpath \"$.a\" exists'}\n",
            "      - hurl: |\n",
            "\n",
            "      - hurl: |\n",
            "          jsonpath \"$.b\" exists\n",
        ));
        let source = PackSource {
            name: "mixed.yaml".into(),
            text: Arc::clone(&text),
        };
        let err = pack::load(&[source], &[], KINDS).unwrap_err();
        let FrontError::Diagnostics(diags) = err else {
            panic!("diagnostics expected");
        };
        let empty_expect: Vec<_> = diags
            .iter()
            .filter(|d| d.code == "proef::pack::empty_expect")
            .collect();
        assert_eq!(
            empty_expect.len(),
            1,
            "only the blank second item should be flagged: {diags:?}"
        );
        let diag = empty_expect[0];
        assert!(
            diag.message.contains("expect item 1"),
            "the blank item is index 1: {diag:?}"
        );
        let macro_span =
            crate::pack::locate::macro_span(&text, "mixed").unwrap_or_else(|| panic!("macro span"));
        assert_eq!(
            diag.span,
            Some(macro_span),
            "an unreliable line-scan pairing must anchor on the macro, not a later item's line"
        );
    }

    /// Structured payloads reach the engine's validator at load time —
    /// the same gate raw payloads have always had.
    #[test]
    fn structured_payloads_run_the_engine_validator() {
        let source = PackSource {
            name: "alt.yaml".into(),
            text: Arc::from(
                "macros:\n  probe:\n    match: the alternate step runs\n    steps:\n      - alt:\n          bogus: 1\n",
            ),
        };
        let err = pack::load(&[source], &[], KINDS).unwrap_err();
        let FrontError::Diagnostics(diags) = err else {
            panic!("diagnostics expected");
        };
        assert!(
            diags
                .iter()
                .any(|d| d.code == "proef::pack::payload_invalid"
                    && d.message.contains("unknown alt verb")),
            "{diags:?}"
        );
    }

    /// A doubled-edge `use:` chain that path enumeration would walk ~2^30
    /// times loads instantly under the node-linear graph passes, and a chain
    /// of exactly [`MAX_USE_DEPTH`] macros raises no diagnostics.
    #[test]
    fn use_graph_walk_is_linear_on_multi_edge_dags() {
        const PLAIN: &[StepKindSpec] = &[StepKindSpec {
            prefix: "alt",
            schema: "true",
            validate: None,
            fragments: None,
        }];
        use std::fmt::Write as _;
        let mut yaml = String::from("macros:\n");
        for i in 0..31 {
            writeln!(yaml, "  m{i:02}:").unwrap();
            if i == 0 {
                yaml.push_str("    match: the chain runs\n");
            }
            writeln!(
                yaml,
                "    steps:\n      - use: m{next:02}\n      - use: m{next:02}",
                next = i + 1
            )
            .unwrap();
        }
        yaml.push_str("  m31:\n    steps:\n      - alt:\n          probe: 1\n");
        let packs = pack::load(
            &[PackSource {
                name: "chain.yaml".into(),
                text: Arc::from(yaml.as_str()),
            }],
            &[],
            PLAIN,
        )
        .unwrap();
        assert_eq!(packs.macros.len(), 32);
    }

    /// No validator, so nothing but the raw-option lint speaks.
    const RAW: &[StepKindSpec] = &[StepKindSpec {
        prefix: "alt",
        schema: "true",
        validate: None,
        fragments: None,
    }];

    /// Setting an option in both the block's `[Options]` and its YAML twin
    /// used to run silently: lowering extends the author's own section rather
    /// than opening a second one, and hurl resolves a duplicated option
    /// last-wins, so the raw value won and the pack's `retry:` was a lie. The
    /// span lands on the raw line — the one that used to take effect.
    #[test]
    fn an_option_set_in_both_places_is_rejected() {
        let source = PackSource {
            name: "twice.yaml".into(),
            text: Arc::from(concat!(
                "macros:\n",
                "  twiceOver:\n",
                "    match: I set the retry in both places\n",
                "    steps:\n",
                "      - retry: { count: 3, interval_ms: 200 }\n",
                "        alt: |\n",
                "          GET http://x\n",
                "          [Options]\n",
                "          retry: 5\n",
                "          HTTP 200\n",
            )),
        };
        let FrontError::Diagnostics(diags) = pack::load(&[source], &[], RAW).unwrap_err() else {
            panic!("diagnostics expected");
        };
        let diag = diags
            .iter()
            .find(|d| d.code == "proef::pack::option_declared_twice")
            .unwrap_or_else(|| panic!("expected the clash in {diags:?}"));
        assert!(diag.help.is_some(), "a remediation hint is expected");
        let text = diag.source_text.as_ref().unwrap();
        let span = diag
            .span
            .unwrap_or_else(|| panic!("expected a span: {diag:?}"));
        assert_eq!(
            &text[span.start..span.end],
            "retry: 5",
            "span should land on the raw option line, not the whole macro"
        );
        assert_eq!(
            diags
                .iter()
                .filter(|d| d.code == "proef::pack::option_declared_twice")
                .count(),
            1,
            "one report per option family, however many entries repeat it"
        );
    }

    /// The scan is `[Options]`-scoped on purpose. `retry` is a legal *request
    /// header* name, and a header line is `name: value` like an option line —
    /// so a line-shaped match alone would turn an ordinary header into a hard
    /// error the moment the step also carried a typed `retry:`.
    #[test]
    fn a_request_header_named_retry_is_not_a_clash() {
        let source = PackSource {
            name: "header.yaml".into(),
            text: Arc::from(concat!(
                "macros:\n",
                "  headerRetry:\n",
                "    match: the request header is named retry\n",
                "    steps:\n",
                "      - retry: { count: 3, interval_ms: 200 }\n",
                "        alt: |\n",
                "          GET http://x\n",
                "          retry: 5\n",
                "          HTTP 200\n",
            )),
        };
        pack::load(&[source], &[], RAW)
            .unwrap_or_else(|err| panic!("a header named `retry` must not clash: {err:?}"));
    }

    // -----------------------------------------------------------------------
    // Fragments (ADR-0018)
    // -----------------------------------------------------------------------

    /// A stand-in for a real engine's scanner. `proef-core` cannot depend on
    /// `proef-engine-hurl`, so these tests drive the loader through the seam
    /// exactly as a future engine would — which is also the point: nothing in
    /// the loading rules knows what hurl is.
    ///
    /// `@name` opens a fragment, `retry` marks the one above as declaring a
    /// retry policy, `?var` a read, `!var` a capture, and `@!boom` fails.
    fn fake_scan(
        text: &str,
    ) -> Result<Vec<crate::engine::ScannedFragment>, crate::engine::FragmentScanError> {
        let mut out: Vec<crate::engine::ScannedFragment> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line == "@!boom" {
                return Err(crate::engine::FragmentScanError {
                    line: index + 1,
                    column: 1,
                    message: "unreadable entry".to_owned(),
                });
            }
            if let Some(name) = line.strip_prefix('@') {
                out.push(crate::engine::ScannedFragment {
                    name: Some(name.to_owned()),
                    text: format!("GET http://x/{name}\n"),
                    line: index + 1,
                    placeholders: Vec::new(),
                    declared_options: Vec::new(),
                });
            } else if let Some(last) = out.last_mut() {
                if line == "retry" {
                    last.declared_options.push("retry".to_owned());
                } else if let Some(read) = line.strip_prefix('?') {
                    last.placeholders.push(read.to_owned());
                }
            }
        }
        Ok(out)
    }

    const SCANNING: &[StepKindSpec] = &[StepKindSpec {
        prefix: "alt",
        schema: "true",
        validate: None,
        fragments: Some(crate::engine::FragmentSupport {
            ext: "frag",
            scan: fake_scan,
        }),
    }];

    fn source(name: &str, text: &str) -> PackSource {
        PackSource {
            name: name.to_owned(),
            text: Arc::from(text),
        }
    }

    fn diags_of(packs: &[PackSource], fragments: &[PackSource]) -> Vec<crate::diag::Diag> {
        match pack::load(packs, fragments, SCANNING) {
            Ok(_) => Vec::new(),
            Err(FrontError::Diagnostics(diags)) => diags,
            Err(other) => panic!("diagnostics expected, got {other:?}"),
        }
    }

    fn has(diags: &[crate::diag::Diag], code: &str) -> bool {
        diags.iter().any(|d| d.code == code)
    }

    #[test]
    fn a_ref_names_a_loaded_fragment() {
        let packs = pack::load(
            &[source(
                "p.yaml",
                "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: admin.search\n",
            )],
            &[source("api.frag", "@admin.search\n")],
            SCANNING,
        )
        .unwrap_or_else(|err| panic!("should load: {err:?}"));
        assert_eq!(packs.fragments.len(), 1);
        assert!(packs.find_fragment("admin.search").is_some());
        // Qualified and bare spellings resolve the same fragment, as `use:` does.
        assert!(packs.find_fragment("api.frag#admin.search").is_some());
        assert!(packs.find_fragment("other.frag#admin.search").is_none());
    }

    #[test]
    fn a_ref_to_an_unknown_fragment_is_rejected_with_a_suggestion() {
        let diags = diags_of(
            &[source(
                "p.yaml",
                "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: admin.serch\n",
            )],
            &[source("api.frag", "@admin.search\n")],
        );
        let diag = diags
            .iter()
            .find(|d| d.code == "proef::pack::unknown_ref")
            .unwrap_or_else(|| panic!("expected unknown_ref in {diags:?}"));
        assert!(diag.message.contains("did you mean `admin.search`?"));
    }

    /// With no fragment files loaded at all, the help has to say so — otherwise
    /// the author reads "names no loaded fragment" as a typo in their own name.
    #[test]
    fn an_unknown_ref_with_no_fragments_loaded_points_at_the_config() {
        let diags = diags_of(
            &[source(
                "p.yaml",
                "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: admin.search\n",
            )],
            &[],
        );
        let diag = diags
            .iter()
            .find(|d| d.code == "proef::pack::unknown_ref")
            .unwrap_or_else(|| panic!("expected unknown_ref in {diags:?}"));
        assert!(
            diag.help
                .as_deref()
                .unwrap_or_default()
                .contains("fragments"),
            "{:?}",
            diag.help
        );
    }

    #[test]
    fn a_step_is_one_body_form_only() {
        let diags = diags_of(
            &[source(
                "p.yaml",
                "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: f\n        alt: |\n          GET http://x\n",
            )],
            &[source("api.frag", "@f\n")],
        );
        assert!(has(&diags, "proef::pack::body_form_conflict"), "{diags:?}");
    }

    /// `bind:` feeds a fragment's variables. On an inline step there is nothing
    /// to feed, so accepting it would silently ignore what the author wrote.
    #[test]
    fn bind_without_a_ref_is_rejected() {
        let diags = diags_of(
            &[source(
                "p.yaml",
                "macros:\n  m:\n    match: it runs\n    steps:\n      - bind: { a: b }\n        alt: |\n          GET http://x\n",
            )],
            &[],
        );
        assert!(has(&diags, "proef::pack::bind_without_ref"), "{diags:?}");
    }

    #[test]
    fn fragment_names_are_global() {
        let diags = diags_of(
            &[source(
                "p.yaml",
                "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: dup\n",
            )],
            &[source("a.frag", "@dup\n"), source("b.frag", "@dup\n")],
        );
        let diag = diags
            .iter()
            .find(|d| d.code == "proef::pack::duplicate_fragment")
            .unwrap_or_else(|| panic!("expected duplicate_fragment in {diags:?}"));
        assert!(diag.message.contains("a.frag") && diag.message.contains("b.frag"));
    }

    /// A fragment file the engine cannot read reports its own diagnostic and is
    /// skipped — the same "never sinks its siblings" rule packs get.
    #[test]
    fn an_unreadable_fragment_file_does_not_sink_the_others() {
        let diags = diags_of(
            &[source(
                "p.yaml",
                "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: ok\n",
            )],
            &[source("bad.frag", "@!boom\n"), source("good.frag", "@ok\n")],
        );
        assert!(has(&diags, "proef::pack::bad_annotation"), "{diags:?}");
        assert!(
            !has(&diags, "proef::pack::unknown_ref"),
            "the readable file still loaded: {diags:?}"
        );
    }

    /// The same rule an inline block gets: one authority per option, whichever
    /// body form the hurl text lives in.
    #[test]
    fn retry_declared_by_both_fragment_and_step_is_rejected() {
        let diags = diags_of(
            &[source(
                "p.yaml",
                "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: poll\n        retry: { count: 3, interval_ms: 200 }\n",
            )],
            &[source("api.frag", "@poll\nretry\n")],
        );
        let diag = diags
            .iter()
            .find(|d| d.code == "proef::pack::option_declared_twice")
            .unwrap_or_else(|| panic!("expected option_declared_twice in {diags:?}"));
        assert!(diag.message.contains("fragment `poll`"), "{}", diag.message);
    }

    #[test]
    fn bind_scopes_survive_loading() {
        let packs = pack::load(
            &[source(
                "p.yaml",
                "bind:\n  base: ${url:base}\nmacros:\n  m:\n    match: it runs\n    bind:\n      q: ${q}\n    steps:\n      - ref: f\n        bind:\n          id: \"{{recordId}}\"\n",
            )],
            &[source("api.frag", "@f\n")],
            SCANNING,
        )
        .unwrap_or_else(|err| panic!("should load: {err:?}"));
        assert_eq!(packs.bind["p.yaml"]["base"], "${url:base}");
        let macro_ = &packs.macros["m"];
        assert_eq!(macro_.bind["q"], "${q}");
        let crate::pack::MacroBody::Steps(steps) = &macro_.body else {
            panic!("steps expected");
        };
        assert_eq!(steps[0].bind["id"], "{{recordId}}");
    }
}
