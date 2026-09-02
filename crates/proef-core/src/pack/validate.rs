//! Pack validation passes 1–13 (TECH-SPEC §4.1).
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
//!
//! Fragments (ADR-0018) add five more: 9. a `ref:` names a loaded fragment ·
//! 10. no two fragment files declare the same name (in `mod.rs`, at insertion) ·
//! 11. a step is `ref:` **xor** a payload/`use:` · 12. an option family is not
//! declared both in the fragment's own `[Options]` and as the step's key, and no
//! variable is both supplied by the fragment's `[Options] variable:` and given by
//! a `bind:` — pass 6's rule applied across the file boundary, family-to-family
//! for options and name-to-name for variables · 13. a fragment file the
//! engine's scanner could not read, or an annotation it could not attach (in
//! `mod.rs`, at scan time).
//!
//! Fragments deliberately **skip pass 7**: they parse as authored, so there is
//! no probe instantiation to guess at. Their `bind:` tables are checked here
//! for reachability; whether every `{{…}}` is actually supplied is a lower-time
//! question (`proef::lower::unbound_placeholder`), because only lowering knows
//! what earlier steps captured.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::locate;
use super::{
    ExpectItem, Macro, MacroBody, MacroStep, MacroStepKind, PackSet, PackSource, PayloadForm,
    RawMacro, RawStep,
};
use crate::diag::Diag;
use crate::engine::{OptionRecogniser, RawOption, RawOptionValue, StepKindSpec};
use crate::lower::macro_has_ref;
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
    index: &locate::MacroIndex<'_>,
    diags: &mut Vec<Diag>,
) -> Option<Macro> {
    let span = index.macro_span(name);
    let match_span = index.match_span(name);
    let at = |diag: Diag| {
        diag.with_source(source.name.clone(), Arc::clone(&source.text))
            .maybe_span(span)
    };
    // Pattern and defaults problems live on the `match:` line, and the caret
    // should too. `match_span` was computed here since the pass was written
    // and never reached a diagnostic — thirteen of the nineteen seeded pack
    // snapshots underlined the macro *name* while the defect sat on a line
    // outside the excerpt (one excerpted the *previous* macro). miette's
    // source rendering is this tool's main authoring surface; aim it.
    let at_match = |diag: Diag| {
        diag.with_source(source.name.clone(), Arc::clone(&source.text))
            .maybe_span(match_span.or(span))
    };

    // Pass 1: match guard rails.
    if let Some(pattern) = &raw.match_ {
        for problem in matcher::pattern_problems(pattern, &raw.params) {
            diags.push(at_match(Diag::error(
                problem.code(),
                format!("macro `{name}`: {problem}"),
            )));
        }
    }

    // Pass 2: defaults must name declared params. The caret sits on the
    // `match:` line too — the params it declares are what a default must
    // name, and the macro-name span said nothing about either.
    for default_key in raw.defaults.keys() {
        if !raw.params.contains(default_key) {
            let suggestion = matcher::suggest_or_enumerate(
                default_key,
                raw.params.iter().map(String::as_str),
                None,
            );
            diags.push(at_match(Diag::error(
                "proef::pack::default_not_param",
                format!(
                    "macro `{name}`: default `{default_key}` is not a declared param{suggestion}"
                ),
            )));
        }
    }

    // Pass 2b: a macro-scope `bind:` needs something in this macro to bind.
    // Same rule as the step-scope check, at the scope above it: a `bind:` no
    // `ref:` can read is silently dropped at lower time, and a setting quietly
    // ignored is the bug both halves refuse to ship. The predicate is *this
    // macro's own* steps because a `use:` target resolves its own scopes — a
    // parent's macro-scope table never reaches the child (ADR-0018).
    if !raw.bind.is_empty() && !raw.steps.iter().any(|step| step.ref_.is_some()) {
        diags.push(at(Diag::error(
            "proef::pack::bind_without_ref",
            format!(
                "macro `{name}`: `bind:` supplies a fragment's `{{{{…}}}}` variables, but no step here has a `ref:` — a `use:` target resolves its own bindings, so this table would go unread"
            ),
        )));
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
            let hurl_spans = index.expect_hurl_line_spans(name);
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
            // Dropped from the body: it is not a runnable step in either form,
            // and keeping it would invite the target-existence pass to report a
            // *second* thing wrong with a step whose real problem is stated
            // above. What the drop must not do is let a later pass conclude
            // anything from the gap — see `pack_scope_bind_pass`.
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
    // The suffix table mirrors `hurl_core::types::DurationUnit` exactly
    // (ms/s/m/h). Falling behind hurl's grammar is how `delay: 5h` — five
    // times the cap — validated clean while `delay: 90m` was refused.
    let (number, unit_ms) = if let Some(n) = value.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = value.strip_suffix('s') {
        (n, 1000)
    } else if let Some(n) = value.strip_suffix('m') {
        (n, 60_000)
    } else if let Some(n) = value.strip_suffix('h') {
        (n, 3_600_000)
    } else {
        (value, 1)
    };
    number.trim().parse::<u64>().ok()?.checked_mul(unit_ms)
}

/// Passes over the complete macro set: `use:` graph (4, 5), payload kinds (8),
/// raw-block finite-retry scan (6), and engine probe validation (7).
pub(crate) fn run_cross_macro_passes(set: &PackSet, kinds: &[StepKindSpec], diags: &mut Vec<Diag>) {
    // One index per pack *file*, not per macro: indexing is a whole-file pass
    // and a file holds many macros. Locating a macro used to rescan the text
    // per lookup, which made validation quadratic in the macro count.
    let mut anchors: BTreeMap<&str, locate::MacroIndex<'_>> = BTreeMap::new();
    for macro_ in set.macros.values() {
        anchors
            .entry(macro_.pack.as_str())
            .or_insert_with(|| locate::MacroIndex::new(&macro_.source));
    }

    for macro_ in set.macros.values() {
        let anchors = &anchors[macro_.pack.as_str()];
        let at = |diag: Diag| {
            diag.with_source(macro_.pack.clone(), Arc::clone(&macro_.source))
                .maybe_span(macro_.span)
        };
        let MacroBody::Steps(steps) = &macro_.body else {
            continue;
        };

        // Macro scope: the union over this macro's own `ref:` targets. Skipped
        // when the macro has none — `bind_without_ref` already owns that case
        // and says something more useful about it.
        if !macro_.bind.is_empty() && macro_has_ref(macro_) {
            let (readable, complete) = scope_placeholders(set, macro_);
            if complete {
                unread_bind_pass(
                    "this macro refs",
                    &format!("macro `{}`", macro_.name),
                    macro_.bind.keys(),
                    &readable,
                    &at,
                    diags,
                );
            }
        }

        let mut payload_ordinals: BTreeMap<&str, usize> = BTreeMap::new();
        for (index, step) in steps.iter().enumerate() {
            match &step.kind {
                MacroStepKind::Use { target, with } => {
                    use_target_passes(set, macro_, index, target, with, &at, diags);
                }
                MacroStepKind::Ref { .. } => {
                    ref_target_passes(set, kinds, macro_, index, step, &at, diags);
                }
                MacroStepKind::Payload { kind, payload } => {
                    let ordinal = *payload_ordinals
                        .entry(kind.as_str())
                        .and_modify(|n| *n += 1)
                        .or_insert(0);
                    payload_passes(
                        macro_, anchors, index, kind, step, payload, ordinal, kinds, &at, diags,
                    );
                }
            }
        }
    }

    pack_scope_bind_pass(set, diags);
    use_graph_passes(set, diags);
}

/// Target existence for one `ref:`, plus the double-declaration checks against
/// the fragment's own `[Options]` — the same rule `lint_raw_options` applies to
/// an inline block, so the two body forms behave identically rather than
/// differing by where the hurl text happens to live.
///
/// Two comparisons, because the two halves of an `[Options]` section clash on
/// different keys: option *families* (`retry:`, `delay:`) family-to-family, and
/// supplied *variables* name-to-name.
fn ref_target_passes(
    set: &PackSet,
    kinds: &[StepKindSpec],
    macro_: &Macro,
    index: usize,
    step: &MacroStep,
    at: &impl Fn(Diag) -> Diag,
    diags: &mut Vec<Diag>,
) {
    // Taken from the step rather than passed beside it: the caller matched this
    // variant to dispatch here, so a second `target` parameter was one more
    // argument that could only ever hold the same value.
    let MacroStepKind::Ref { target } = &step.kind else {
        return;
    };
    let Some(fragment) = set.find_fragment(target) else {
        let suggestion = matcher::suggest_or_enumerate(
            target.rsplit('#').next().unwrap_or(target),
            set.fragments.keys().map(String::as_str),
            Some("`proef fragments` lists them"),
        );
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
            })
            .with_fix_replacing(target, suggestion.nearest.as_deref()),
        );
        return;
    };
    // The ADR-0007 value caps, against the fragment's own text. The same scan
    // the inline form gets: a fragment reaches the runner through the same
    // `[Options]` section, so `retry: -1` behind a `ref:` abandons an
    // uncancellable thread exactly as it would inline. Anchored on the `ref:`
    // line, since that is what the pack author can edit, and the message names
    // the fragment file and line so the other half is findable.
    for violation in recogniser(kinds, &fragment.kind)
        .map(|recognise| scan_option_values(&fragment.text, recognise))
        .unwrap_or_default()
    {
        diags.push(
            at(Diag::error(
                violation.code,
                format!(
                    "macro `{}` step {index}: in fragment `{}` (`{}` line {}), {}",
                    macro_.name,
                    fragment.name,
                    fragment.file,
                    fragment.line + violation.line - 1,
                    violation.detail
                ),
            ))
            .with_help(
                "the cap applies to the executed request, whichever file it was written in — \
                 edit the fragment, or point this step at one that stays within budget",
            ),
        );
    }
    // Every option family the step sets, not just retry: `delay:` bakes into the
    // same `[Options]` section through the same code path, so leaving it
    // unchecked reproduces exactly the silent last-wins the inline half of this
    // rule exists to refuse. The families come from the step itself, so this and
    // `lint_raw_options` cannot disagree about what a step declared.
    for family in step.declared_options() {
        if fragment.declared_options.iter().any(|o| o == family) {
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
    // Step scope: this step binds for exactly one fragment, so its own reads are
    // the whole scope.
    unread_bind_pass(
        "this step refs",
        &format!("macro `{}` step {index}", macro_.name),
        step.bind.keys(),
        &fragment.placeholders.iter().map(String::as_str).collect(),
        at,
        diags,
    );
    // The same rule one level down. A `bind:` reaches hurl as `[Options]
    // variable:`, so a fragment supplying that name is the identical silent
    // last-wins — except here the *fragment's* line lands last and wins,
    // discarding the value the pack author wrote. Worse than the retry case:
    // hurl assigns `variable:` into the run-level set rather than scoping it,
    // so the discarded value stays discarded for every later entry too.
    for name in &fragment.supplied_variables {
        let Some(scope) = binding_scope(set, macro_, step, name) else {
            continue;
        };
        diags.push(
            at(Diag::error(
                "proef::pack::option_declared_twice",
                format!(
                    "macro `{}` step {index}: `{name}` is supplied twice — by fragment `{}` (`{}` line {}) and by the {scope} `bind:`",
                    macro_.name, fragment.name, fragment.file, fragment.line
                ),
            ))
            .with_help(format!(
                "delete whichever is not authoritative — both reach the entry as \
                 `variable: {name}=`, where the fragment's own line lands last and the bound \
                 value would never reach the request",
            )),
        );
    }
}

/// Refuse `bind:` keys that nothing in their scope reads.
///
/// The finer half of `bind_without_ref`, which only catches a table with no
/// `ref:` at all. A *key* nobody reads is the same bug one level down and the
/// commoner one — a typo binds `toekn` beside `token` and the run stays green,
/// because a fragment reads what it reads and an extra name simply never
/// arrives. It was the only authoring mistake in the fragment path that produced
/// no signal whatsoever.
///
/// `readable` is a **union over the scope**, never one fragment's placeholders:
/// a pack-scope table is "the plumbing every macro in the file needs", so a key
/// serving one macro and not its siblings is correct usage, and per-fragment
/// checking would reject exactly the thing pack scope is for. Dead means no
/// fragment reachable from this scope reads it.
fn unread_bind_pass<'a>(
    scope: &str,
    where_: &str,
    bind: impl Iterator<Item = &'a String>,
    readable: &BTreeSet<&str>,
    at: &impl Fn(Diag) -> Diag,
    diags: &mut Vec<Diag>,
) {
    for key in bind {
        if readable.contains(key.as_str()) {
            continue;
        }
        let suggestion = matcher::suggest_or_enumerate(key, readable.iter().copied(), None);
        diags.push(
            at(Diag::error(
                "proef::pack::unread_bind_key",
                format!("{where_}: `bind:` supplies `{key}`, which no fragment {scope} reads{suggestion}"),
            ))
            .with_help(if readable.is_empty() {
                "no fragment in scope reads any variable — delete the table".to_owned()
            } else {
                format!(
                    "the fragments in scope read: `{}`",
                    readable.iter().copied().collect::<Vec<_>>().join("`, `")
                )
            })
            .with_fix_replacing(key, suggestion.nearest.as_deref()),
        );
    }
}

/// Which `bind:` scope supplies `name`, most specific first — the half of a
/// double-supply diagnostic that says where to look for the other declaration.
fn binding_scope(
    set: &PackSet,
    macro_: &Macro,
    step: &MacroStep,
    name: &str,
) -> Option<&'static str> {
    if step.bind.contains_key(name) {
        Some("step's")
    } else if macro_.bind.contains_key(name) {
        Some("macro's")
    } else if set
        .bind
        .get(&macro_.pack)
        .is_some_and(|table| table.contains_key(name))
    {
        Some("pack's")
    } else {
        None
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
        let suggestion = matcher::suggest_or_enumerate(
            target.rsplit('#').next().unwrap_or(target),
            set.macros.keys().map(String::as_str),
            Some("`proef macros` lists them"),
        );
        diags.push(
            at(Diag::error(
                "proef::pack::unknown_use",
                format!(
                    "macro `{}` step {index}: `use: {target}` names no loaded macro{suggestion}",
                    macro_.name
                ),
            ))
            .with_fix_replacing(target, suggestion.nearest.as_deref()),
        );
        return;
    };

    for key in with.keys() {
        if !target_macro.params.contains(key) {
            let suggestion = matcher::suggest_or_enumerate(
                key,
                target_macro.params.iter().map(String::as_str),
                None,
            );
            diags.push(
                at(Diag::error(
                    "proef::pack::unknown_with_key",
                    format!(
                        "macro `{}` step {index}: `with:` key `{key}` is not a param of `{}`{suggestion}",
                        macro_.name, target_macro.name
                    ),
                ))
                .with_fix_replacing(key, suggestion.nearest.as_deref()),
            );
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
    anchors: &locate::MacroIndex<'_>,
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
        // The valid set has exactly one member today (`hurl`), and the old
        // silent-below-threshold tail never named it — the message said "not
        // claimed by any registered engine" about a registry of one.
        let suggestion = matcher::suggest_or_enumerate(kind, kinds.iter().map(|s| s.prefix), None);
        diags.push(
            at(Diag::error(
                "proef::pack::unknown_step_kind",
                format!(
                    "macro `{}` step {index}: step kind `{kind}:` is not claimed by any registered engine{suggestion}",
                    macro_.name
                ),
            ))
            .with_fix_replacing(kind, suggestion.nearest.as_deref()),
        );
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

    lint_raw_options(
        macro_,
        anchors,
        index,
        kind,
        step,
        ordinal,
        text,
        spec.options,
        at,
        diags,
    );

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
                        .maybe_span(anchors.payload_line_span(
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

/// The option recogniser contributed by the kind named `kind`, if it has one.
///
/// Routed by kind rather than by "the first engine that has one": a step's kind
/// names its engine (ADR-0002), so a second engine's `[Options]` vocabulary
/// applies to its own steps and to nothing else.
fn recogniser(kinds: &[StepKindSpec], kind: &str) -> Option<OptionRecogniser> {
    kinds
        .iter()
        .find(|spec| spec.prefix == kind)
        .and_then(|spec| spec.options)
}

/// One ADR-0007 value-cap violation in hurl text: which line, which code, and
/// the sentence describing it — but not where it lives, because that differs
/// between the two body forms.
struct OptionViolation {
    /// 1-based line within the scanned text.
    line: usize,
    code: &'static str,
    detail: String,
}

/// The ADR-0007 value caps over **any** hurl text: a finite `retry:`/`repeat:`
/// and a `delay:` under the ceiling.
///
/// Split out of [`lint_raw_options`] because the caps must hold wherever the
/// text came from. Both body forms reach the same runner through the same
/// `[Options]` section, and hurl has no cancellation — an infinite retry makes
/// the batch budget unestimatable and leaves the watchdog abandoning a thread it
/// cannot stop, which is the failure mode ADR-0007 exists to prevent. While this
/// scan lived inside the inline-only linter, a fragment could set `retry: -1`
/// and validate clean: byte-identical text was rejected inline and accepted
/// behind a `ref:`.
///
/// Returns findings rather than diagnostics so each caller can anchor them in
/// its own source — the one thing the two genuinely disagree about.
/// One recognised option line of an engine payload.
struct OptionLine<'a> {
    /// 1-based line within the scanned text.
    line: usize,
    /// The option key, trimmed — the text left of the `:`.
    key: &'a str,
    /// The raw value, untrimmed.
    value: &'a str,
    /// What the claiming engine says this key means.
    option: RawOption,
    /// Whether the line sits inside an `[Options]` section.
    in_options: bool,
}

/// Every option line an engine payload declares, fence-aware and section-aware.
///
/// One walk feeding both halves of pass 6. Splitting the value caps out of the
/// twin-declaration check is what let the caps reach fragments, but doing it as
/// two loops duplicated the fence rule and the section bookkeeping — the exact
/// cost the old single-pass comment predicted. The *walk* is the shared part;
/// the two checks differ only in what they filter for.
fn option_lines(text: &str, recognise: OptionRecogniser) -> Vec<OptionLine<'_>> {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut in_options = false;
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
        // `is_section_header`, not whole-line equality: hurl permits a
        // trailing comment on the header line, and `[Options] # tuning`
        // followed by `retry: -1` validated clean under the equality test —
        // the exact infinite-retry hole this pass exists to refuse.
        if trimmed.starts_with('[') {
            in_options = crate::lower::is_section_header(trimmed, "Options");
        } else if crate::lower::is_method_line(trimmed) {
            in_options = false;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        // The engine names its own options; the core only decides what a value
        // may be. A key this engine does not claim is somebody else's line.
        let Some(option) = recognise(key.trim()) else {
            continue;
        };
        out.push(OptionLine {
            line: line_no + 1,
            key: key.trim(),
            value,
            option,
            in_options,
        });
    }
    out
}

fn scan_option_values(text: &str, recognise: OptionRecogniser) -> Vec<OptionViolation> {
    let mut found = Vec::new();
    for OptionLine {
        line,
        key,
        value,
        option,
        in_options,
    } in option_lines(text, recognise)
    {
        // Only `[Options]` lines are options. Without this gate a `[Query]`
        // parameter, form field or lowercase header named `retry` is a hard
        // error — and once the caps reached fragments that meant proef
        // rejecting a corpus it does not own for a line hurl treats as data,
        // against ADR-0018's promise that pointing at someone else's files
        // costs nothing. The inline half of pass 6 has always gated on this;
        // the value scan inherited a laxer rule that was invisible while it
        // only ever read proef's own YAML.
        if !in_options {
            continue;
        }
        let mut push = |code: &'static str, detail: String| {
            found.push(OptionViolation { line, code, detail });
        };
        match option.value {
            Some(RawOptionValue::Count) => match value.trim().parse::<i64>() {
                Ok(-1) => push(
                    "proef::pack::retry_not_finite",
                    format!(
                        "`{key}: -1` is infinite — hurl cannot be interrupted mid-call, so an \
                         unbounded retry is a hang the watchdog must abandon; give it a finite \
                         count (`{key}: 5`, with `retry-interval:` for pacing)"
                    ),
                ),
                Ok(n) if n > MAX_COUNT => push(
                    "proef::pack::retry_not_finite",
                    format!("`{key}: {n}` is budget-hostile — the cap is {MAX_COUNT}"),
                ),
                _ => {}
            },
            Some(RawOptionValue::Duration) => {
                if let Some(ms) = raw_duration_ms(value)
                    && ms > MAX_DELAY_MS
                {
                    push(
                        "proef::pack::delay_unbounded",
                        format!(
                            "`{key}: {}` exceeds the {MAX_DELAY_MS} ms (1 hour) cap",
                            value.trim()
                        ),
                    );
                }
            }
            None => {}
        }
    }
    found
}

/// Pass 6 (raw half), for an inline block: the ADR-0007 value caps
/// ([`scan_option_values`]) anchored in the pack, plus the double-declaration
/// check.
///
/// The double-declaration half: an option set *both* in the block's own
/// `[Options]` and as its YAML twin (`retry:` / `delay:`). Lowering extends an
/// author's section rather than opening a second one, and hurl resolves
/// duplicate options last-wins, so the raw value quietly beat the typed one —
/// the pack said one thing and the run did another. Only `[Options]`-section
/// lines count: a request header may legitimately be named `retry`, and this
/// is a hard error, so it must not fire on one.
///
/// Only this half is inline-only, because only it compares against the *step's*
/// YAML keys. The caps read the text alone, which is why they now also run
/// against a fragment's — see [`scan_option_values`].
#[allow(clippy::too_many_arguments)]
fn lint_raw_options(
    macro_: &Macro,
    anchors: &locate::MacroIndex<'_>,
    index: usize,
    kind: &str,
    step: &MacroStep,
    ordinal: usize,
    text: &str,
    recognise: Option<OptionRecogniser>,
    at: &impl Fn(Diag) -> Diag,
    diags: &mut Vec<Diag>,
) {
    // The caller already resolved this kind's spec to get here; re-finding it
    // would be a second lookup for the same answer.
    let Some(recognise) = recognise else {
        return;
    };
    for violation in scan_option_values(text, recognise) {
        diags.push(
            at(Diag::error(
                violation.code,
                format!("macro `{}` step {index}: {}", macro_.name, violation.detail),
            ))
            .maybe_span(anchors.payload_line_span(
                &macro_.name,
                kind,
                ordinal,
                violation.line,
            )),
        );
    }

    // One report per option family is enough to act on; a block whose every
    // entry repeats the clash would otherwise bury the step in duplicates.
    // A list rather than a flag per family, so a new family needs no latch.
    let mut said: Vec<&'static str> = Vec::new();
    for entry in option_lines(text, recognise) {
        // Only `[Options]`-section lines count here: a request header may
        // legitimately be named `retry`, and this is a hard error.
        //
        // The engine maps its own spellings onto the families a pack knows
        // (`engine::OPTION_FAMILIES`) — several keys may share one policy, which
        // is the engine's business, not the core's.
        let Some(option) = entry.option.family.filter(|_| entry.in_options) else {
            continue;
        };
        let line_no = entry.line - 1;
        if step.declared_options().any(|f| f == option) && !said.contains(&option) {
            said.push(option);
            diags.push(
                at(Diag::error(
                    "proef::pack::option_declared_twice",
                    format!(
                        "macro `{}` step {index}: `{option}` is declared twice — here in `[Options]`, and as the step's own `{option}:`",
                        macro_.name
                    ),
                ))
                .maybe_span(anchors.payload_line_span(&macro_.name, kind, ordinal, line_no + 1))
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
/// A pack-scope `bind:` needs something in that pack to bind.
///
/// Every placeholder read by the fragments a macro's own `ref:` steps name —
/// the readable set for that macro's scope — and whether that set is
/// **complete**. A `use:` target resolves its own scopes (ADR-0018), so its
/// reads deliberately do not count here.
///
/// Incomplete when some `ref:` names nothing loaded: a typo, an unparseable
/// corpus file, or `[run] fragments` unset. The set is then a lower bound, and
/// "no fragment in scope reads this key" would be a conclusion drawn from a
/// scope known to be missing pieces — which is how one typo'd `ref:` used to
/// produce a correct `unknown_ref` and then two false `unread_bind_key`
/// telling the author to delete a `bind:` that was never wrong.
///
/// A set, because both callers compare membership; returning a `Vec` only to
/// have each of them collect it was two spellings of one answer.
fn scope_placeholders<'a>(set: &'a PackSet, macro_: &Macro) -> (BTreeSet<&'a str>, bool) {
    let MacroBody::Steps(steps) = &macro_.body else {
        return (BTreeSet::new(), true);
    };
    let mut readable = BTreeSet::new();
    let mut complete = true;
    for step in steps {
        let MacroStepKind::Ref { target } = &step.kind else {
            continue;
        };
        match set.find_fragment(target) {
            Some(fragment) => readable.extend(fragment.placeholders.iter().map(String::as_str)),
            None => complete = false,
        }
    }
    (readable, complete)
}

/// The same rule the macro and step scopes already carry, at the scope above
/// them — `AUTHORING.md` said it applied "at every scope" while the third one
/// silently dropped its table, which is the setting-ignored-in-silence bug the
/// other two exist to refuse. Attributed to a macro from the pack, since a
/// `PackSet` keeps macros rather than the pack's own source text.
fn pack_scope_bind_pass(set: &PackSet, diags: &mut Vec<Diag>) {
    for (pack, table) in &set.bind {
        if table.is_empty() {
            continue;
        }
        // A step that declared both `ref:` and a payload is reported and then
        // dropped, so this pack's loaded bodies no longer show every `ref:` its
        // author wrote. "No macro here has a `ref:`" is then a claim about the
        // loaded set, not about the pack — and a pack whose only `ref:` was the
        // conflicted step got told its `bind:` would go unread, which is false
        // and points nowhere useful. Infer nothing from a body known to be
        // incomplete; the conflict itself already fails the run.
        if diags.iter().any(|d| {
            d.code == "proef::pack::body_form_conflict"
                && d.source_name.as_deref() == Some(pack.as_str())
        }) {
            continue;
        }
        let from_pack = || set.macros.values().filter(|m| &m.pack == pack);
        if from_pack().any(macro_has_ref) {
            // Reachable, so the table is read — but each *key* still has to be.
            // Union over every fragment any macro in this pack refs.
            let mut readable: BTreeSet<&str> = BTreeSet::new();
            let mut complete = true;
            for macro_ in from_pack() {
                let (reads, whole) = scope_placeholders(set, macro_);
                readable.extend(reads);
                complete &= whole;
            }
            if let Some(anchor) = from_pack().next().filter(|_| complete) {
                let at = |d: Diag| {
                    d.with_source(anchor.pack.clone(), Arc::clone(&anchor.source))
                        .maybe_span(anchor.span)
                };
                unread_bind_pass(
                    "in this pack",
                    &format!("pack `{pack}`"),
                    table.keys(),
                    &readable,
                    &at,
                    diags,
                );
            }
            continue;
        }
        // Nothing to anchor on if the pack contributed no macros at all; that
        // is already its own diagnostic.
        let Some(anchor) = from_pack().next() else {
            continue;
        };
        diags.push(
            Diag::error(
                "proef::pack::bind_without_ref",
                format!(
                    "pack `{pack}`: `bind:` supplies a fragment's `{{{{…}}}}` variables, but no macro in this pack has a `ref:` step — the table would go unread"
                ),
            )
            .with_source(anchor.pack.clone(), Arc::clone(&anchor.source))
            .maybe_span(anchor.span)
            .with_help(
                "delete the table, or give the step that needs it a `ref: <fragment>` body",
            ),
        );
    }
}

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
        .with_help(
            "`use:` composition must be a tree: pull the shared steps into a \
             third macro both sides `use:`, instead of pointing at each other",
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
        options: None,
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
        let err = pack::load(&[source], &crate::pack::FragmentCorpus::empty(), KINDS).unwrap_err();
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
        let err = pack::load(&[source], &crate::pack::FragmentCorpus::empty(), KINDS).unwrap_err();
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
        let macro_span = crate::pack::locate::MacroIndex::new(&text)
            .macro_span("mixed")
            .unwrap_or_else(|| panic!("macro span"));
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
        let err = pack::load(&[source], &crate::pack::FragmentCorpus::empty(), KINDS).unwrap_err();
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
            options: None,
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
            &crate::pack::FragmentCorpus::empty(),
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
        // A kind with raw `[Options]`, so the budget rules have a vocabulary to
        // apply. A kind contributing no recogniser is deliberately not linted —
        // the core has no way to know what its option keys mean — so a fixture
        // without one would test that silence rather than the rule.
        options: Some(fake_recognise),
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
        let FrontError::Diagnostics(diags) =
            pack::load(&[source], &crate::pack::FragmentCorpus::empty(), RAW).unwrap_err()
        else {
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

    /// hurl's `section_name` parser leaves the rest of the header line to the
    /// ordinary comment terminator, so `[Options] # tuning` is a real section
    /// to hurl — and under whole-line equality it was *not* one to this scan,
    /// which turned every pass-6 check off behind a comment: a `retry: -1`
    /// dry-ran clean, reopening exactly the abandoned-thread hole ADR-0007
    /// exists to refuse.
    #[test]
    fn a_commented_options_header_still_opens_the_section() {
        let src = source(
            "commented.yaml",
            concat!(
                "macros:\n",
                "  tuned:\n",
                "    match: the header carries a comment\n",
                "    steps:\n",
                "      - alt: |\n",
                "          GET http://x\n",
                "          [Options] # tuning\n",
                "          retry: -1\n",
                "          HTTP 200\n",
            ),
        );
        let FrontError::Diagnostics(diags) =
            pack::load(&[src], &crate::pack::FragmentCorpus::empty(), RAW).unwrap_err()
        else {
            panic!("diagnostics expected");
        };
        assert!(
            diags
                .iter()
                .any(|d| d.code == "proef::pack::retry_not_finite"),
            "the commented header must not disable the finite-retry refusal: {diags:?}"
        );
    }

    /// The duration table mirrors hurl's `DurationUnit` (ms/s/m/h) in full.
    /// With `h` missing, `delay: 90m` was refused while the strictly larger
    /// `delay: 5h` fell through the suffix parse and validated clean.
    #[test]
    fn an_hour_suffixed_delay_is_capped_like_the_minute_spelling() {
        let src = source(
            "hours.yaml",
            concat!(
                "macros:\n",
                "  patient:\n",
                "    match: the delay is written in hours\n",
                "    steps:\n",
                "      - alt: |\n",
                "          GET http://x\n",
                "          [Options]\n",
                "          delay: 5h\n",
                "          HTTP 200\n",
            ),
        );
        let FrontError::Diagnostics(diags) =
            pack::load(&[src], &crate::pack::FragmentCorpus::empty(), RAW).unwrap_err()
        else {
            panic!("diagnostics expected");
        };
        assert!(
            diags
                .iter()
                .any(|d| d.code == "proef::pack::delay_unbounded"),
            "an hour-suffixed delay over the cap must be refused: {diags:?}"
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
        pack::load(&[source], &crate::pack::FragmentCorpus::empty(), RAW)
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
    ) -> Result<crate::engine::ScannedFile, crate::engine::FragmentScanError> {
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
                    name: name.to_owned(),
                    text: format!("GET http://x/{name}\n"),
                    line: index + 1,
                    placeholders: Vec::new(),
                    declared_options: Vec::new(),
                    supplied_variables: Vec::new(),
                });
            } else if let Some(last) = out.last_mut() {
                if line == "retry" {
                    last.declared_options.push("retry".to_owned());
                } else if let Some(read) = line.strip_prefix('?') {
                    last.placeholders.push(read.to_owned());
                } else if let Some(supplied) = line.strip_prefix('=') {
                    use std::fmt::Write as _;
                    let _ = write!(last.text, "[Options]\nvariable: {supplied}=from-fragment\n");
                    last.supplied_variables.push(supplied.to_owned());
                } else if let Some(raw) = line.strip_prefix('+') {
                    // A raw line appended to the fragment's own text, so a test
                    // can put an `[Options]` value in the file rather than in
                    // the scanner's structured output — which is the whole point
                    // of the value caps: they read the text, not the summary.
                    use std::fmt::Write as _;
                    let _ = write!(last.text, "[Options]\n{raw}\n");
                }
            }
        }
        Ok(crate::engine::ScannedFile {
            fragments: out,
            unannotated: Vec::new(),
        })
    }

    const SCANNING: &[StepKindSpec] = &[StepKindSpec {
        prefix: "alt",
        schema: "true",
        validate: None,
        fragments: Some(crate::engine::FragmentSupport {
            ext: "frag",
            scan: fake_scan,
            template_reads: |_| Vec::new(),
        }),
        options: Some(fake_recognise),
    }];

    /// The stub engine's option vocabulary. Deliberately spelled like hurl's, so
    /// these tests exercise the same shapes the real engine contributes — the
    /// point of the seam is that the core never learns them, not that the core
    /// stops enforcing anything.
    fn fake_recognise(key: &str) -> Option<crate::engine::RawOption> {
        use crate::engine::{RawOption, RawOptionValue};
        let (family, value) = match key {
            "retry" => (Some("retry"), Some(RawOptionValue::Count)),
            "repeat" => (None, Some(RawOptionValue::Count)),
            "delay" => (Some("delay"), Some(RawOptionValue::Duration)),
            "retry-interval" => (Some("retry"), None),
            _ => return None,
        };
        Some(RawOption { family, value })
    }

    fn source(name: &str, text: &str) -> PackSource {
        PackSource {
            name: name.to_owned(),
            text: Arc::from(text),
        }
    }

    fn diags_of(packs: &[PackSource], fragments: &[PackSource]) -> Vec<crate::diag::Diag> {
        let corpus = pack::FragmentCorpus::new(fragments.to_vec(), SCANNING);
        match pack::load(packs, &corpus, SCANNING) {
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
            &pack::FragmentCorpus::new(vec![source("api.frag", "@admin.search\n")], SCANNING),
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

    /// ADR-0007's caps are about the request that runs, so they cannot depend on
    /// which file the author wrote it in. They used to: the scan lived inside
    /// the inline-only linter, so byte-identical `[Options]` was rejected in a
    /// `hurl:` block and accepted in a fragment — and an infinite retry is
    /// precisely what the watchdog cannot rescue, hurl having no cancellation.
    #[test]
    fn a_fragments_option_values_are_capped_like_an_inline_blocks() {
        for (line, code) in [
            ("retry: -1", "proef::pack::retry_not_finite"),
            ("repeat: -1", "proef::pack::retry_not_finite"),
            ("delay: 99999999", "proef::pack::delay_unbounded"),
        ] {
            let diags = diags_of(
                &[source(
                    "p.yaml",
                    "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: f\n",
                )],
                &[source("api.frag", &format!("@f\n+{line}\n"))],
            );
            assert!(has(&diags, code), "`{line}` in a fragment: {diags:?}");
        }
    }

    /// A step carrying both `ref:` and a payload is an error — but it is still a
    /// step the author put a `ref:` on. While the conflicted step was dropped
    /// from the normalized body, "does this macro reference a fragment?" had two
    /// answers (the raw shape at macro scope, the normalized body at pack
    /// scope), and a pack whose only `ref:` also carried a payload was told, on
    /// top of the real error, that no macro in it had a `ref:` at all.
    #[test]
    fn a_conflicted_body_form_is_still_a_ref_at_every_scope() {
        let diags = diags_of(
            &[source(
                "p.yaml",
                "bind:\n  a: b\nmacros:\n  m:\n    match: it runs\n    steps:\n      - ref: f\n        alt: |\n          GET http://x\n",
            )],
            &[source("api.frag", "@f\n")],
        );
        assert!(has(&diags, "proef::pack::body_form_conflict"), "{diags:?}");
        assert!(
            !has(&diags, "proef::pack::bind_without_ref"),
            "the pack does have a `ref:` — {diags:?}"
        );
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

    /// The same rule one scope up. A macro-scope `bind:` on a macro with no
    /// `ref:` step is unreadable — and the tempting reading, that a `use:`
    /// target will pick it up, is wrong: the child resolves its own scopes.
    /// Left unchecked this is the *silent* half of the same mistake, and it is
    /// the one authors hit, because factoring plumbing upward is the habit.
    #[test]
    fn a_macro_scope_bind_with_no_ref_step_is_rejected() {
        let diags = diags_of(
            &[source(
                "p.yaml",
                "macros:\n  target:\n    match: the target\n    steps:\n      - ref: f\n  m:\n    match: it runs\n    bind:\n      a: b\n    steps:\n      - use: target\n",
            )],
            &[source("api.frag", "@f\n")],
        );
        assert!(has(&diags, "proef::pack::bind_without_ref"), "{diags:?}");
    }

    /// …and it must not fire on a macro that does have one, or the rule would
    /// refuse the feature it exists to protect.
    #[test]
    fn a_macro_scope_bind_beside_a_ref_step_is_accepted() {
        let diags = diags_of(
            &[source(
                "p.yaml",
                "macros:\n  m:\n    match: it runs\n    bind:\n      a: b\n    steps:\n      - ref: f\n",
            )],
            &[source("api.frag", "@f\n")],
        );
        assert!(!has(&diags, "proef::pack::bind_without_ref"), "{diags:?}");
    }

    /// `bind:` reaches hurl as `[Options] variable:`, so a fragment supplying
    /// the same name is the same silent last-wins `option_declared_twice` was
    /// built for — and it lands the wrong way round: the fragment's literal
    /// wins and the bound value never reaches the request. Checked at each
    /// scope, because the diagnostic has to say where the other half lives.
    #[test]
    fn a_variable_the_fragment_supplies_and_the_pack_binds_is_refused() {
        for (scope, pack) in [
            (
                "step's",
                "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: f\n        bind:\n          token: v\n",
            ),
            (
                "macro's",
                "macros:\n  m:\n    match: it runs\n    bind:\n      token: v\n    steps:\n      - ref: f\n",
            ),
            (
                "pack's",
                "bind:\n  token: v\nmacros:\n  m:\n    match: it runs\n    steps:\n      - ref: f\n",
            ),
        ] {
            let diags = diags_of(
                &[source("p.yaml", pack)],
                &[source("api.frag", "@f\n=token\n")],
            );
            let diag = diags
                .iter()
                .find(|d| d.code == "proef::pack::option_declared_twice")
                .unwrap_or_else(|| panic!("expected {scope} clash in {diags:?}"));
            assert!(
                diag.message.contains("token") && diag.message.contains(scope),
                "{scope}: {}",
                diag.message
            );
        }
    }

    /// The other half of the same rule: a fragment may supply a variable no one
    /// binds. That is how the file stays runnable under stock `hurl` with no
    /// variables file, so refusing it would break ADR-0018's premise.
    #[test]
    fn a_variable_only_the_fragment_supplies_is_accepted() {
        let diags = diags_of(
            &[source(
                "p.yaml",
                "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: f\n",
            )],
            &[source("api.frag", "@f\n=token\n")],
        );
        assert!(
            !has(&diags, "proef::pack::option_declared_twice"),
            "{diags:?}"
        );
    }

    /// `AUTHORING.md` said `bind_without_ref` applies "at every scope" while the
    /// pack scope silently dropped its table — the setting-ignored-in-silence
    /// bug the other two scopes exist to refuse.
    #[test]
    fn a_pack_scope_bind_with_no_ref_anywhere_is_rejected() {
        let diags = diags_of(
            &[source(
                "p.yaml",
                "bind:\n  unused: v\nmacros:\n  m:\n    match: it runs\n    steps:\n      - hurl: |\n          GET http://x\n          HTTP 200\n",
            )],
            &[source("api.frag", "@f\n")],
        );
        let diag = diags
            .iter()
            .find(|d| d.code == "proef::pack::bind_without_ref")
            .unwrap_or_else(|| panic!("expected bind_without_ref in {diags:?}"));
        assert!(
            diag.message.contains("no macro in this pack"),
            "{}",
            diag.message
        );
    }

    /// …and a pack whose macros do `ref:` keeps its table.
    #[test]
    fn a_pack_scope_bind_beside_a_ref_is_accepted() {
        let diags = diags_of(
            &[source(
                "p.yaml",
                "bind:\n  used: v\nmacros:\n  m:\n    match: it runs\n    steps:\n      - ref: f\n",
            )],
            &[source("api.frag", "@f\n")],
        );
        assert!(!has(&diags, "proef::pack::bind_without_ref"), "{diags:?}");
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
            // The fragment reads all three, one per scope: a `bind:` key nothing
            // in scope reads is now its own error (`unread_bind_key`), so a
            // fixture binding into a fragment that reads nothing would be
            // testing an unloadable pack.
            &pack::FragmentCorpus::new(
                vec![source("api.frag", "@f\n?base\n?q\n?id\n")],
                SCANNING,
            ),
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

    /// A corpus is scanned **at most once**, however many times packs are loaded
    /// against it.
    ///
    /// One `proef test` loads packs up to four times — the suite, then
    /// `[run] setup`/`teardown`, each validated and then run — always against
    /// the same corpus. Rescanning per load measured ~75% of a 200-file run's
    /// total work, so this is a performance property, and performance nothing
    /// asserts is performance that quietly comes back.
    #[test]
    fn one_corpus_is_scanned_once_however_many_loads_read_it() {
        let packs = [source(
            "p.yaml",
            "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: admin.search\n",
        )];
        let corpus =
            pack::FragmentCorpus::new(vec![source("api.frag", "@admin.search\n")], SCANNING);

        let (first, _) = pack::load_collecting(&packs, &corpus, SCANNING);
        let (second, _) = pack::load_collecting(&packs, &corpus, SCANNING);

        assert!(first.find_fragment("admin.search").is_some());
        assert!(
            Arc::ptr_eq(&first.fragments, &second.fragments),
            "a second load must reuse the first scan, not repeat it"
        );
    }

    /// A corpus nothing `ref:`s is never scanned — CONFIG.md's promise that
    /// pointing proef at a corpus you did not write costs nothing.
    ///
    /// Proven by making the scan *observable*: the file would fail to parse, so
    /// a `bad_annotation` diagnostic appears exactly when the scan ran. Sharing
    /// one corpus across loads must not have turned the scan eager.
    #[test]
    fn a_corpus_no_pack_refs_is_never_scanned() {
        let unreadable = vec![source("api.frag", "@!boom\n")];

        let no_ref = [source(
            "p.yaml",
            "macros:\n  m:\n    match: it runs\n    steps:\n      - alt: GET /x\n",
        )];
        let corpus = pack::FragmentCorpus::new(unreadable.clone(), SCANNING);
        let (_, diags) = pack::load_collecting(&no_ref, &corpus, SCANNING);
        assert!(
            !diags
                .iter()
                .any(|d| d.code == "proef::pack::bad_annotation"),
            "no `ref:` anywhere, so the corpus must never be read: {diags:?}"
        );

        // Control: the same corpus, with a `ref:` present, *is* scanned — so the
        // assertion above is about laziness, not about a scanner that never runs.
        let with_ref = [source(
            "p.yaml",
            "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: whatever\n",
        )];
        let corpus = pack::FragmentCorpus::new(unreadable, SCANNING);
        let (_, diags) = pack::load_collecting(&with_ref, &corpus, SCANNING);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "proef::pack::bad_annotation"),
            "a pack with a `ref:` must reach the scanner: {diags:?}"
        );
    }
}

/// Every pack-validation code this module can reach, exercised by name.
///
/// `DIAGNOSTICS.md` calls a code "a contract: they never change meaning". Ten of
/// this module's codes had nothing holding them to that — reachable in
/// production, documented, exercised by nothing — and a diagnostic nobody
/// triggers is a diagnostic nobody has read. The table is deliberately one test
/// per code rather than one big assertion, so a failure names the contract that
/// broke instead of "some code went missing".
#[cfg(test)]
mod diagnostic_code_coverage {
    #![allow(clippy::unwrap_used)]

    use std::fmt::Write as _;
    use std::sync::Arc;

    use crate::diag::FrontError;
    use crate::engine::{PayloadProbeError, StepKindSpec};
    use crate::pack::{self, FragmentCorpus, PackSource};

    /// Accepts anything: these tests are about *pack shape*, and a rejecting
    /// validator would add a `payload_invalid` to every case and make each
    /// assertion read as "the code is in there somewhere" for the wrong reason.
    // The signature is dictated by `StepKindSpec::validate`, so the `Result`
    // is not this function's choice to make.
    #[allow(clippy::unnecessary_wraps)]
    fn accept(_text: &str) -> Result<(), PayloadProbeError> {
        Ok(())
    }

    const KINDS: &[StepKindSpec] = &[StepKindSpec {
        prefix: "alt",
        schema: "true",
        validate: Some(accept),
        fragments: None,
        options: None,
    }];

    /// The codes a pack raises when loaded. Errors and warnings alike — a
    /// warning is as much a contract as an error, and `docstring_unused` is one.
    fn codes(pack_text: &str) -> Vec<(String, String)> {
        let source = PackSource {
            name: "p.yaml".into(),
            text: Arc::from(pack_text),
        };
        match pack::load(&[source], &FragmentCorpus::empty(), KINDS) {
            Err(FrontError::Diagnostics(diags)) => diags
                .iter()
                .map(|d| (d.code.to_string(), d.message.clone()))
                .collect(),
            Err(other) => panic!("expected diagnostics, got {other:?}"),
            Ok(_) => Vec::new(),
        }
    }

    /// Failure prints the messages, not just the codes: a fixture that stopped
    /// reaching the code it targets usually stopped being valid at all, and the
    /// message is what says which.
    #[track_caller]
    fn assert_raises(code: &str, pack_text: &str) {
        let raised = codes(pack_text);
        assert!(
            raised.iter().any(|(c, _)| c == code),
            "expected {code}, got:\n{}",
            raised
                .iter()
                .map(|(c, m)| format!("  {c}: {m}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn a_macro_with_both_steps_and_expect_is_refused() {
        assert_raises(
            "proef::pack::steps_and_expect",
            // `expect:` items take `status`/`hurl` by schema, not an arbitrary
            // payload kind — so this fixture uses `status:` rather than `alt:`.
            "macros:\n  m:\n    match: a sentence\n    steps:\n      - alt: |\n          x\n    expect:\n      - status: 200\n",
        );
    }

    #[test]
    fn a_macro_with_neither_steps_nor_expect_is_refused() {
        assert_raises(
            "proef::pack::empty_macro",
            "macros:\n  m:\n    match: a sentence\n",
        );
    }

    #[test]
    fn a_step_with_no_body_of_any_kind_is_refused() {
        assert_raises(
            "proef::pack::empty_step",
            "macros:\n  m:\n    match: a sentence\n    steps:\n      - name: nothing here\n",
        );
    }

    #[test]
    fn a_step_with_two_payload_keys_is_refused() {
        assert_raises(
            "proef::pack::multiple_payloads",
            "macros:\n  m:\n    match: a sentence\n    steps:\n      - alt: |\n          x\n        \
             other: |\n          y\n",
        );
    }

    /// `global` is the only promotion target (ADR-0005). A typo here would
    /// otherwise read as a capture that silently never promotes.
    #[test]
    fn a_save_as_target_other_than_global_is_refused() {
        assert_raises(
            "proef::pack::bad_save_target",
            "macros:\n  m:\n    match: a sentence\n    steps:\n      - alt: |\n          x\n        \
             saveAs:\n          token: session\n",
        );
    }

    #[test]
    fn a_use_step_carrying_step_modifiers_is_refused() {
        assert_raises(
            "proef::pack::use_with_modifiers",
            "macros:\n  target:\n    match: the target sentence\n    steps:\n      - alt: |\n          x\n  m:\n    match: a sentence\n    steps:\n      - use: target\n        optional: true\n",
        );
    }

    #[test]
    fn a_use_step_that_also_carries_a_payload_is_refused() {
        assert_raises(
            "proef::pack::use_with_payload",
            "macros:\n  target:\n    match: the target sentence\n    steps:\n      - alt: |\n          x\n  m:\n    match: a sentence\n    steps:\n      - use: target\n        alt: |\n          y\n",
        );
    }

    /// `with:` supplies a `use:` target's params. On a `ref:` step it reads as
    /// if it would bind the fragment — which is `bind:`'s job — so the silent
    /// version of this mistake is values that simply never arrive.
    #[test]
    fn a_with_block_without_a_use_is_refused() {
        assert_raises(
            "proef::pack::with_without_use",
            "macros:\n  m:\n    match: a sentence\n    steps:\n      - ref: some.fragment\n        \
             with:\n          k: v\n",
        );
    }

    /// The depth limit is a real ceiling, not a stack-overflow guess: the walk
    /// is memoized, so the diagnostic has to be raised deliberately.
    #[test]
    fn a_use_chain_past_the_depth_limit_is_refused() {
        let depth = super::MAX_USE_DEPTH + 2;
        let mut pack = String::from("macros:\n");
        for level in 0..=depth {
            let _ = write!(
                pack,
                "  m{level}:\n    match: sentence number {level}\n    steps:\n"
            );
            if level == depth {
                pack.push_str("      - alt: |\n          x\n");
            } else {
                let _ = writeln!(pack, "      - use: m{}", level + 1);
            }
        }
        assert_raises("proef::pack::use_too_deep", &pack);
    }

    /// Pass 7 probe-lowers each payload before the engine ever sees it. Probe
    /// mode tolerates anything that *might* resolve later — unknown vars, env,
    /// globals — and still refuses what can never resolve: an unknown
    /// namespace is a typo at authoring time, not a value that arrives at run
    /// time, so the pack must not load.
    #[test]
    fn a_payload_reference_that_can_never_resolve_is_refused() {
        assert_raises(
            "proef::pack::bad_reference",
            "macros:\n  m:\n    match: a sentence\n    steps:\n      - alt: |\n          GET /x/${nosuchns:key}\n",
        );
    }
}

/// The published complexity claim, kept honest.
///
/// #138 made pack validation linear in the macro count and the changelog states
/// the result as a *shape*: "the curve changed shape — 4× per doubling before,
/// ~2× after". That number lived only in prose, which is where this project has
/// now watched four separate claims decay. Nothing prevented a future span
/// locator from scanning the whole file again and quietly restoring the
/// quadratic behaviour, because a regression there costs seconds rather than
/// correctness and no gate measures seconds.
///
/// **Why a ratio and not a benchmark.** The claim *is* a ratio, so asserting one
/// tests the thing that was promised rather than a proxy for it. A ratio is also
/// the only timing assertion that survives a shared CI runner: load inflates
/// both measurements together and cancels, where an absolute threshold would
/// have to be set so loose it stopped meaning anything. `TESTING-STRATEGY.md`
/// §5 permits wall time "only as generous upper bounds", and this is one — the
/// observed value is ~2.05 against a bound of 3.0, while quadratic is 4.0.
///
/// Criterion, divan and iai-callgrind were all considered and rejected.
/// iai-callgrind is the right tool for gating in CI — instruction counts are
/// immune to runner noise — but it needs valgrind, so it would be a gate the
/// maintainer cannot reproduce on macOS. The other two measure wall time, which
/// puts them in the same noise regime as this test while also adding a
/// dependency tree to a workspace that audits every edge.
#[cfg(test)]
mod complexity {
    #![allow(clippy::unwrap_used)]

    use std::fmt::Write as _;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::engine::{PayloadProbeError, StepKindSpec};
    use crate::pack::{self, FragmentCorpus, PackSource};

    #[allow(clippy::unnecessary_wraps)] // signature fixed by StepKindSpec::validate
    fn accept(_text: &str) -> Result<(), PayloadProbeError> {
        Ok(())
    }

    const KINDS: &[StepKindSpec] = &[StepKindSpec {
        prefix: "alt",
        schema: "true",
        validate: Some(accept),
        fragments: None,
        options: None,
    }];

    /// A pack of `n` independent macros — no `use:` edges, so the graph passes
    /// are trivial and what is being measured is the per-macro span locating
    /// that #138 changed.
    fn pack_of(n: usize) -> String {
        let mut text = String::from("macros:\n");
        for i in 0..n {
            let _ = write!(
                text,
                "  m{i}:\n    match: sentence number {i}\n    steps:\n      - alt: |\n          GET /x\n"
            );
        }
        text
    }

    /// Best of five. The minimum is the right statistic for a timing sample:
    /// scheduler noise only ever *adds*, so the fastest observation is the one
    /// closest to the work actually done.
    fn best_load(text: &str) -> Duration {
        let mut best = Duration::MAX;
        for _ in 0..5 {
            let source = PackSource {
                name: "p.yaml".into(),
                text: Arc::from(text),
            };
            let start = Instant::now();
            let loaded = pack::load(&[source], &FragmentCorpus::empty(), KINDS);
            best = best.min(start.elapsed());
            assert!(loaded.is_ok(), "the generated pack must be valid");
        }
        best
    }

    #[test]
    fn validation_cost_stays_linear_in_the_macro_count() {
        let single = best_load(&pack_of(1_000));
        let double = best_load(&pack_of(2_000));

        // Guard against a degenerate measurement making the ratio meaningless:
        // if the smaller load is too fast to time, the assertion below would
        // pass on anything.
        assert!(
            single >= Duration::from_millis(2),
            "1000 macros loaded in {single:?} — too fast to compare; raise the sizes"
        );

        let ratio = double.as_secs_f64() / single.as_secs_f64();
        assert!(
            ratio < 3.0,
            "doubling the macro count cost {ratio:.2}× ({single:?} → {double:?}). \
             Linear is ~2× and quadratic is ~4×, so this is the span-locating \
             regression #138 fixed: every locator scanning the whole pack file \
             to find its own macro's block. Check `locate::MacroIndex` is still \
             built once and read, not rebuilt or bypassed."
        );
    }
}
