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
    /// Secrets referenced anywhere in the scenario, as **hurl variable name →
    /// secret name** (values never appear). The two differ only when a
    /// fragment binding renames one — `bind: { auth_token: ${secret:apiToken} }`
    /// — which is what lets a corpus proef did not write keep its own variable
    /// names (ADR-0018). Inline `${secret:X}` maps `X` to itself.
    pub secrets: BTreeMap<String, String>,
    /// Global keys read anywhere in the scenario (drives `.vars`, ADR-0010).
    pub globals: BTreeSet<String>,
    /// Soft findings (dry-run globals, …) as warning diagnostics.
    pub warnings: Vec<Diag>,
}

/// Runtime backstop for `use:` recursion (statically checked at pack load).
const MAX_EXPANSION_DEPTH: usize = 32;

/// Resolved fragment bindings in scope, by hurl variable name (ADR-0018).
type Bindings = BTreeMap<String, Bound>;

/// One resolved binding, split by how its value reaches hurl.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Bound {
    /// A literal, injected as a per-entry `[Options] variable:` — so it lands
    /// in the artifact and replays identically under the stock CLI.
    Value(String),
    /// A secret, named here and injected by the engine at run time. It must
    /// **not** take the `[Options]` path: that would write the value into the
    /// artifact, which ADR-0005 forbids outright.
    Secret(String),
}

/// `${secret:NAME}` as an *entire* binding value — the only shape a secret may
/// take. Anything composite (`Bearer ${secret:token}`) would have to be
/// materialized to be injected, putting the value in the artifact; the fragment
/// should spell the literal part itself and bind the secret alone.
///
/// Both this and [`mentions_secret`] read the value through the resolver's own
/// [`crate::resolve::first_reference`], so there is exactly one thing that knows
/// what a `${…}` reference is — and `$${` stays an escape here too, rather than
/// a second scanner mistaking an escaped literal for a live secret.
fn whole_secret(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    let (name, start, end) = crate::resolve::first_reference(trimmed)?;
    if start != 0 || end != trimmed.len() {
        return None; // something surrounds it — composite
    }
    let inner = name.strip_prefix("secret:")?.trim();
    (!inner.is_empty() && !inner.contains(['{', '$'])).then_some(inner)
}

/// Does any *live* reference in `value` name the secret namespace? Used to
/// refuse a composite once [`whole_secret`] has ruled out the sole legal shape.
fn mentions_secret(value: &str) -> bool {
    let mut rest = value;
    while let Some((name, _, end)) = crate::resolve::first_reference(rest) {
        if name.starts_with("secret:") {
            return true;
        }
        rest = &rest[end..];
    }
    false
}

/// Escape a bound value for a quoted hurl option value. `\` and `"` are the
/// two characters that would end or re-open the literal; `{{…}}` is left alone
/// on purpose, since hurl expanding it is the point.
fn quote_option(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Does this macro reference any fragment? Bindings resolve only when they can
/// be used — otherwise a pack-scope `${fake:…}` would advance for macros that
/// never bind anything, and an unresolvable one would fail a macro it does not
/// concern.
fn macro_has_ref(macro_: &Macro) -> bool {
    match &macro_.body {
        MacroBody::Steps(steps) => steps
            .iter()
            .any(|step| matches!(step.kind, MacroStepKind::Ref { .. })),
        MacroBody::Expect(_) => false,
    }
}

/// The bindings a macro's `ref:` steps see before their own: pack scope,
/// resolved once per scenario and cached, then macro scope, resolved once per
/// invocation — which is what makes "one binding, one value" mean something
/// different at each level (ADR-0018). Empty for a macro with no `ref:` step,
/// so an unused table never advances the `${fake:…}` counter.
fn scope_bindings(
    macro_: &Macro,
    ctx: &LowerCtx<'_>,
    refs: &mut Refs,
    sinks: &mut Sinks,
    resolve_in: &impl Fn(&str, &mut Refs, &mut Sinks) -> Option<String>,
    at: &impl Fn(Diag) -> Diag,
) -> Bindings {
    let mut scoped = Bindings::new();
    if !macro_has_ref(macro_) {
        return scoped;
    }
    if let Some(table) = ctx.packs.bind.get(&macro_.pack) {
        if !refs.pack_bindings.contains_key(&macro_.pack) {
            let resolved = resolve_bindings(table, refs, sinks, resolve_in, at);
            refs.pack_bindings.insert(macro_.pack.clone(), resolved);
        }
        if let Some(cached) = refs.pack_bindings.get(&macro_.pack) {
            scoped.extend(cached.clone());
        }
    }
    scoped.extend(resolve_bindings(&macro_.bind, refs, sinks, resolve_in, at));
    scoped
}

/// Resolve one `bind:` table in the caller's scope.
fn resolve_bindings(
    table: &BTreeMap<String, String>,
    refs: &mut Refs,
    sinks: &mut Sinks,
    resolve_in: &impl Fn(&str, &mut Refs, &mut Sinks) -> Option<String>,
    at: &impl Fn(Diag) -> Diag,
) -> Bindings {
    let mut out = Bindings::new();
    for (name, value) in table {
        if let Some(secret) = whole_secret(value) {
            out.insert(name.clone(), Bound::Secret(secret.to_owned()));
            continue;
        }
        if mentions_secret(value) {
            sinks.errors.push(
                at(Diag::error(
                    "proef::lower::secret_in_composite_bind",
                    format!(
                        "binding `{name}` mixes a secret into a larger value — the result would have to be written into the artifact to be injected"
                    ),
                ))
                .with_help(
                    "bind the secret on its own and put the surrounding text in the fragment \
                     (`Authorization: Bearer {{token}}` with `bind: { token: ${secret:…} }`)",
                ),
            );
            continue;
        }
        if let Some(resolved) = resolve_in(value, refs, sinks) {
            out.insert(name.clone(), Bound::Value(resolved));
        }
    }
    out
}

/// Every capture name produced by the steps lowered so far — what a fragment
/// may read without binding it.
fn captures_before(out: &[LoweredStep]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for step in out {
        if let StepPayload::HurlEntries(text) = &step.payload {
            let lines: Vec<&str> = text.lines().collect();
            names.extend(crate::emit::capture_names(&lines));
        }
    }
    names
}

/// The two diagnostic sinks a lowering pass fills.
///
/// Bundled rather than passed as two parameters, because they are the same type
/// and were adjacent: transposing them at any of a dozen call sites compiled
/// cleanly and routed every error into `warnings`, so a scenario that should
/// have failed lowered "successfully" and the run exited 0. Named fields make
/// that mistake unspellable.
#[derive(Debug, Default)]
struct Sinks {
    /// Non-fatal notes surfaced beside a scenario that still runs.
    warnings: Vec<Diag>,
    /// Failures — any one of these aborts the scenario.
    errors: Vec<Diag>,
}

/// What resolution referenced while lowering one scenario.
#[derive(Debug, Default)]
struct Refs {
    /// hurl variable name → secret name (see [`LoweredScenario::secrets`]).
    secrets: BTreeMap<String, String>,
    globals: BTreeSet<String>,
    /// Pack-scope `bind:` tables resolved once per scenario, by pack name.
    /// Scope decides *when* a binding resolves: one binding is one value, so a
    /// pack-scope `${fake:email}` is one identity for the whole scenario, a
    /// macro-scope one is per invocation, a step-scope one is per step.
    pack_bindings: BTreeMap<String, Bindings>,
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
    let mut sinks = Sinks::default();
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
            &mut sinks,
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
            sinks.errors.push(
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

    if sinks.errors.iter().any(|d| d.severity == Severity::Error) {
        return Err(sinks.errors);
    }

    Ok(LoweredScenario {
        name: scenario.name.clone(),
        tags: scenario.tags.clone(),
        line: scenario.line,
        batches: segment(lowered, ctx.kind_to_engine),
        secrets: refs.secrets,
        globals: refs.globals,
        warnings: sinks.warnings,
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
    sinks: &mut Sinks,
    at: &impl Fn(Diag) -> Diag,
) {
    if depth > MAX_EXPANSION_DEPTH {
        sinks.errors.push(at(Diag::error(
            "proef::lower::expansion_too_deep",
            format!(
                "macro expansion exceeded depth {MAX_EXPANSION_DEPTH} at `{}`",
                macro_.name
            ),
        )));
        return;
    }

    let resolve_in = |text: &str, refs: &mut Refs, sinks: &mut Sinks| -> Option<String> {
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
                // Inline `${secret:X}` lowers to the literal `{{X}}`, so the
                // hurl variable and the secret share a name; only a fragment
                // binding can make them differ.
                refs.secrets
                    .extend(resolution.secrets.into_iter().map(|s| (s.clone(), s)));
                refs.globals.extend(resolution.globals);
                push_warnings(sinks, &resolution.warnings, ctx, &macro_.name);
                Some(resolution.text)
            }
            Err(err) => {
                sinks.errors.push(at(Diag::error(
                    err.code(),
                    format!("in macro `{}`: {err}", macro_.name),
                )));
                None
            }
        }
    };

    // Bindings in scope for this macro's `ref:` steps: pack scope (resolved
    // once per scenario, cached) then macro scope (once per invocation, which
    // is here). Step scope is applied per step in `expand_step`.
    let scoped = scope_bindings(macro_, ctx, refs, sinks, &resolve_in, at);

    match &macro_.body {
        MacroBody::Expect(items) => {
            let mut merged: Option<(StepKindId, bool, usize)> = None;
            for item in items {
                let status = match &item.status {
                    Some(status) => match resolve_in(status, refs, sinks) {
                        Some(status) => Some(status),
                        None => continue,
                    },
                    None => None,
                };
                let fragment = match &item.fragment {
                    Some(fragment) => match resolve_in(fragment, refs, sinks) {
                        Some(fragment) => Some(fragment),
                        None => continue,
                    },
                    None => None,
                };
                if let Some((kind, optional, lines)) =
                    merge_expect(status.as_deref(), fragment.as_deref(), out, sinks, at)
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
                    // Not the `fragment` bound a few lines up: that one is
                    // `ExpectItem::fragment`, raw assert *text* (YAML key
                    // `hurl:`), and predates ADR-0018's named fragments. An
                    // `expect:` step executes no request, so it refs nothing.
                    fragment: None,
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
                    sinks,
                    at,
                    &resolve_in,
                    &scoped,
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
    sinks: &mut Sinks,
    at: &impl Fn(Diag) -> Diag,
    resolve_in: &impl Fn(&str, &mut Refs, &mut Sinks) -> Option<String>,
    scoped: &Bindings,
) {
    match &macro_step.kind {
        MacroStepKind::Ref { target } => expand_ref_step(
            target, macro_step, step_ref, ctx, out, refs, sinks, at, resolve_in, scoped,
        ),
        MacroStepKind::Use { target, with } => {
            let Some(target_macro) = ctx.packs.find_use_target(target) else {
                return; // pack validation reported it
            };
            // `with:` values resolve in the *parent* scope, then become the
            // child's args (child defaults fill the rest).
            let mut child_args = BTreeMap::new();
            for (key, value) in with {
                if let Some(resolved) = resolve_in(value, refs, sinks) {
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
                sinks,
                at,
            );
        }
        MacroStepKind::Payload { kind, payload } => expand_payload_step(
            macro_step, kind, payload, step_ref, out, refs, sinks, resolve_in,
        ),
    }
}

/// One `ref:` step: bind, check every read is supplied, then emit the
/// fragment's own text with the bindings baked in (ADR-0018).
#[allow(clippy::too_many_arguments)]
fn expand_ref_step(
    target: &str,
    macro_step: &MacroStep,
    step_ref: &StepRef,
    ctx: &LowerCtx<'_>,
    out: &mut Vec<LoweredStep>,
    refs: &mut Refs,
    sinks: &mut Sinks,
    at: &impl Fn(Diag) -> Diag,
    resolve_in: &impl Fn(&str, &mut Refs, &mut Sinks) -> Option<String>,
    scoped: &Bindings,
) {
    let Some(fragment) = ctx.packs.find_fragment(target) else {
        return; // pack validation reported it
    };
    // A `ref:` step's functional resolves are its step-scope bindings — those
    // are what the request is built from — so the label replays from here
    // (see `finish_step`). Pack- and macro-scope bindings resolved earlier and
    // are shared, so they are deliberately not replayed.
    let label_fakes_start = refs.fakes;
    // Step scope is the most specific, so it lands last and wins.
    let mut bindings = scoped.clone();
    bindings.extend(resolve_bindings(
        &macro_step.bind,
        refs,
        sinks,
        resolve_in,
        at,
    ));

    // Every variable the fragment reads must be bound here or captured
    // by a step before it. hurl's `[Options] variable:` *assigns* into
    // one shared set (it does not scope), so an unbound name would
    // silently inherit whatever an earlier entry happened to leave —
    // running green against the wrong value.
    let unbound: Vec<&str> = fragment
        .placeholders
        .iter()
        .filter(|name| {
            !bindings.contains_key(name.as_str())
                && !refs.secrets.contains_key(name.as_str())
                // A fragment that answers its own question is answered. This is
                // what lets the file run standalone under the engine's own
                // binary with no variables file — ADR-0018's premise — so
                // refusing it here would reject valid input that the engine
                // itself accepts. Scoped to *this* fragment: a name another
                // entry happened to leave in hurl's shared set is exactly the
                // implicit inheritance this whole check exists to refuse, so
                // only `[Captures]` carries a value forward.
                && !fragment.supplied_variables.contains(name)
        })
        .map(String::as_str)
        .collect();
    // Captures are the expensive half — the scan re-reads every step lowered so
    // far — so it only runs for names a binding did not already cover, which in
    // a pack that binds what its fragment reads is none of them.
    let missing: Vec<&str> = if unbound.is_empty() {
        Vec::new()
    } else {
        let available = captures_before(out);
        unbound
            .into_iter()
            .filter(|name| !available.contains(*name))
            .collect()
    };
    if !missing.is_empty() {
        // Anchored on the fragment, not on the pack step: the variable is
        // literally on that line of that file, and ADR-0018 promises a real
        // file:line. The message names the macro, so the other end of the link
        // is not lost.
        let message = format!(
            "fragment `{}` reads `{}`, which nothing supplies — no `bind:` in scope gives a value, and no earlier step captures it",
            fragment.name,
            missing.join("`, `"),
        );
        sinks.errors.push(
            Diag::error("proef::lower::unbound_placeholder", message)
                .with_source(fragment.file.clone(), Arc::clone(&fragment.source))
                .maybe_span(crate::pack::locate::line_span(
                    &fragment.source,
                    fragment.line,
                ))
                .with_help(format!(
                    "add `bind: {{ {}: … }}` to the step, its macro, or the pack",
                    missing[0]
                )),
        );
        return;
    }

    // Split the bindings by how each reaches hurl. Only literals take the
    // `[Options]` path; a secret's value must never be written into an
    // artifact (ADR-0005), so it is recorded by name and injected at run time.
    // `bindings` is consumed here — it is dead afterwards, so neither half
    // needs to be cloned back out of it.
    let mut literals: BTreeMap<String, String> = BTreeMap::new();
    for (name, bound) in bindings {
        match bound {
            Bound::Secret(secret) => {
                refs.secrets.insert(name, secret);
            }
            Bound::Value(value) => {
                literals.insert(name, value);
            }
        }
    }
    let text = bake_entry_options(
        &fragment.text,
        macro_step.retry,
        macro_step.delay_ms,
        &literals,
    );
    finish_step(
        macro_step,
        step_ref,
        StepKindId::from(fragment.kind.as_str()),
        StepPayload::HurlEntries(text),
        // Qualified here rather than at the reader: `target` is what the pack
        // wrote, which may be the bare name, and a record has to stand on its
        // own — by the time anyone reads it the pack may say something else.
        Some(fragment.qualified()),
        label_fakes_start,
        out,
        refs,
        sinks,
        resolve_in,
    );
}

/// One inline-payload step: resolve `${…}` in place, then bake the step's own
/// `retry:`/`delay:` into `[Options]` (ADR-0010 — artifacts replay identically).
#[allow(clippy::too_many_arguments)]
fn expand_payload_step(
    macro_step: &MacroStep,
    kind: &str,
    payload: &PayloadForm,
    step_ref: &StepRef,
    out: &mut Vec<LoweredStep>,
    refs: &mut Refs,
    sinks: &mut Sinks,
    resolve_in: &impl Fn(&str, &mut Refs, &mut Sinks) -> Option<String>,
) {
    // The label annotates *this* step for humans (artifact comments,
    // events) — it must report the fake values the step actually
    // used, not mint fresh ones of its own. Remember where the
    // occurrence counter stood before the functional resolves
    // (payload, then guard) so the label can replay from the same
    // base afterward.
    let label_fakes_start = refs.fakes;
    let payload = match payload {
        PayloadForm::Raw(text) => {
            let Some(resolved) = resolve_in(text, refs, sinks) else {
                return;
            };
            // Bake `retry:`/`delay:` into hurl `[Options]` so artifacts
            // replay with identical semantics under the stock CLI
            // (ADR-0010); per-entry [Options] override batch defaults.
            let resolved = if macro_step.retry.is_some() || macro_step.delay_ms.is_some() {
                bake_entry_options(
                    &resolved,
                    macro_step.retry,
                    macro_step.delay_ms,
                    &BTreeMap::new(),
                )
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
                resolve_in(text, refs, sinks)
            };
            match resolve_structured(value, &mut resolve) {
                Some(resolved) => StepPayload::Structured(resolved),
                None => return,
            }
        }
    };
    finish_step(
        macro_step,
        step_ref,
        StepKindId::from(kind),
        payload,
        None, // inline `hurl:` block — no fragment to point at
        label_fakes_start,
        out,
        refs,
        sinks,
        resolve_in,
    );
}

/// The tail every expanded step shares: resolve `when:`, resolve `name:` as a
/// *replay*, and push. Both body forms (ADR-0018) end here, so the label rules
/// below are stated and enforced exactly once — a second copy that resolved the
/// label plainly would silently mint fresh `${fake:…}` values.
///
/// `label_fakes_start` is where the occurrence counter stood before *this
/// step's* functional resolves (the payload for an inline step, the step-scope
/// `bind:` for a `ref:` one).
#[allow(clippy::too_many_arguments)]
fn finish_step(
    macro_step: &MacroStep,
    step_ref: &StepRef,
    kind: StepKindId,
    payload: StepPayload,
    // `file.hurl#name` for a `ref:` step, `None` for an inline block — the
    // provenance a run record carries (ADR-0018).
    fragment: Option<String>,
    label_fakes_start: usize,
    out: &mut Vec<LoweredStep>,
    refs: &mut Refs,
    sinks: &mut Sinks,
    resolve_in: &impl Fn(&str, &mut Refs, &mut Sinks) -> Option<String>,
) {
    let when = match &macro_step.when {
        Some(guard) => match resolve_in(guard, refs, sinks) {
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
            let resolved = resolve_in(name, refs, sinks);
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
        kind,
        payload,
        optional: macro_step.optional,
        when,
        label,
        fragment,
        save_as: macro_step.save_as.clone(),
    });
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
    bindings: &BTreeMap<String, String>,
) -> String {
    let mut option_lines: Vec<String> = Vec::new();
    if let Some(retry) = retry {
        option_lines.push(format!("retry: {}", retry.count));
        option_lines.push(format!("retry-interval: {}ms", retry.interval_ms));
    }
    if let Some(delay_ms) = delay_ms {
        option_lines.push(format!("delay: {delay_ms}ms"));
    }
    // Always quoted: an unquoted value that happens to read as a number or a
    // boolean would be stored as one (`variable_value` tries those first), and
    // `records` vs `2` vs `true` must not become three different types by
    // accident. hurl evaluates the quoted form as a template, which is what
    // lets a bound `${url:…}` containing `{{captured}}` finish resolving.
    for (name, value) in bindings {
        option_lines.push(format!("variable: {name}=\"{}\"", quote_option(value)));
    }
    // Nothing to inject: a `ref:` step whose bindings are all secrets, or whose
    // variables all come from earlier captures, would otherwise pay the whole
    // two-pass rewrite to rebuild an identical string — and gain an empty
    // `[Options]` section for it. (The inline path guards at its call site; a
    // `ref:` step cannot, since it does not know whether bindings survived the
    // secret split.)
    if option_lines.is_empty() {
        return text.to_owned();
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
    sinks: &mut Sinks,
    at: &impl Fn(Diag) -> Diag,
) -> Option<(StepKindId, bool, usize)> {
    let Some(previous) = out
        .iter_mut()
        .rev()
        .find(|s| matches!(s.payload, StepPayload::HurlEntries(_)))
    else {
        sinks.errors.push(
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
        sinks.errors.push(at(Diag::error(
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

fn push_warnings(sinks: &mut Sinks, texts: &[String], ctx: &LowerCtx<'_>, where_: &str) {
    for text in texts {
        sinks.warnings.push(
            Diag::warning("proef::lower::dry_run_unknown", format!("{where_}: {text}"))
                .with_source(ctx.feature.path.clone(), Arc::clone(&ctx.feature.source)),
        );
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::engine::StepKindSpec;
    use crate::pack::{self, PackSource};
    use crate::step::StepPayload;

    const KINDS: &[StepKindSpec] = &[StepKindSpec {
        prefix: "hurl",
        schema: "true",
        validate: None,
        fragments: None,
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
            &crate::pack::FragmentCorpus::empty(),
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
        assert!(lowered.secrets.contains_key("apiToken"));

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
            &crate::pack::FragmentCorpus::empty(),
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
            fragments: None,
        }];
        let packs = pack::load(
            &[PackSource {
                name: "alt.yaml".into(),
                text: Arc::from(
                    "macros:\n  probe:\n    match: the alternate step runs\n    steps:\n      - name: probe\n        alt:\n          target: \"${url:base}/item\"\n          checks: [\"${url:base}\", 7]\n",
                ),
            }],
            &crate::pack::FragmentCorpus::empty(),
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
            &crate::pack::FragmentCorpus::empty(),
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
        let baked = bake_entry_options(body, retry, None, &BTreeMap::new());
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
            let baked = bake_entry_options(body, retry, None, &BTreeMap::new());
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
                fragment: None,
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
            &crate::pack::FragmentCorpus::empty(),
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
            &crate::pack::FragmentCorpus::empty(),
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

    // -----------------------------------------------------------------------
    // Fragments (ADR-0018)
    // -----------------------------------------------------------------------

    /// Stands in for an engine's scanner (core cannot depend on one). `@name`
    /// opens a fragment, `?var` is a read, `!var` a capture, `=var` a variable
    /// the fragment supplies itself. The `Result` is never `Err` here but the
    /// seam's signature requires one.
    #[allow(clippy::unnecessary_wraps)]
    fn frag_scan(
        text: &str,
    ) -> Result<Vec<crate::engine::ScannedFragment>, crate::engine::FragmentScanError> {
        let mut out: Vec<crate::engine::ScannedFragment> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if let Some(name) = line.strip_prefix('@') {
                out.push(crate::engine::ScannedFragment {
                    name: name.to_owned(),
                    text: format!("GET http://x/{name}\nHTTP 200\n"),
                    line: index + 1,
                    placeholders: Vec::new(),
                    declared_options: Vec::new(),
                    supplied_variables: Vec::new(),
                });
            } else if let Some(last) = out.last_mut() {
                if let Some(read) = line.strip_prefix('?') {
                    last.placeholders.push(read.to_owned());
                } else if let Some(supplied) = line.strip_prefix('=') {
                    // Honest text, as with captures — and `[Options]` belongs to
                    // the *request* half, so it is inserted before the status
                    // line rather than appended. `bake_entry_options` slots
                    // proef's own lines against this header, so putting it in
                    // the wrong half would test a shape hurl cannot parse.
                    let head = last.text.find("HTTP ").unwrap_or(last.text.len());
                    let section = if last.text[..head].contains("[Options]") {
                        format!("variable: {supplied}=from-fragment\n")
                    } else {
                        format!("[Options]\nvariable: {supplied}=from-fragment\n")
                    };
                    last.text.insert_str(head, &section);
                    last.supplied_variables.push(supplied.to_owned());
                } else if let Some(write) = line.strip_prefix('!') {
                    // A real scanner reads captures out of the entry text, and
                    // so does the core (`emit::capture_names`) — one mechanism,
                    // so keep the stand-in's text honest.
                    if !last.text.contains("[Captures]") {
                        last.text.push_str("[Captures]\n");
                    }
                    last.text.push_str(write);
                    last.text.push_str(": jsonpath \"$.id\"\n");
                }
            }
        }
        Ok(out)
    }

    const FRAG_KINDS: &[StepKindSpec] = &[StepKindSpec {
        prefix: "hurl",
        schema: "true",
        validate: None,
        fragments: Some(crate::engine::FragmentSupport {
            ext: "frag",
            scan: frag_scan,
        }),
    }];

    /// Lower a one-step scenario over the given pack and fragment file.
    fn lower_fragments(pack: &str, fragments: &str) -> Result<LoweredScenario, Vec<Diag>> {
        let packs = pack::load(
            &[PackSource {
                name: "p.yaml".into(),
                text: Arc::from(pack),
            }],
            &pack::FragmentCorpus::new(
                vec![PackSource {
                    name: "api.frag".into(),
                    text: Arc::from(fragments),
                }],
                FRAG_KINDS,
            ),
            FRAG_KINDS,
        )
        .unwrap_or_else(|err| panic!("pack should load: {err:?}"));
        let feature =
            crate::feature::parse("t.feature", "Feature: F\n  Scenario: S\n    When it runs\n")
                .unwrap();
        let scenario = crate::bind::bind(&feature, &packs).unwrap().remove(0);
        let kind_to_engine = BTreeMap::from([("hurl".to_owned(), "hurl".to_owned())]);
        let env = BTreeMap::new();
        let config_vars = BTreeMap::from([("url:base".to_owned(), "http://api".to_owned())]);
        let world = World::new(crate::world::GlobalStore::default());
        let ctx = ctx(
            &feature,
            &packs,
            &kind_to_engine,
            &env,
            &config_vars,
            &world,
        );
        lower(&scenario, &ctx)
    }

    fn only_entry(lowered: &LoweredScenario) -> &str {
        let step = lowered
            .batches
            .iter()
            .flat_map(|b| b.steps.iter())
            .find(|s| matches!(s.payload, StepPayload::HurlEntries(_)))
            .expect("one hurl entry");
        let StepPayload::HurlEntries(text) = &step.payload else {
            unreachable!()
        };
        text
    }

    /// The three scopes cascade, most specific winning, and every literal lands
    /// as a per-entry `[Options] variable:` so the artifact replays identically
    /// under the stock CLI.
    #[test]
    fn bindings_cascade_and_are_injected_as_entry_options() {
        let lowered = lower_fragments(
            "bind:\n  base: ${url:base}\n  who: pack\nmacros:\n  m:\n    match: it runs\n    bind:\n      who: macro\n      extra: yes\n    steps:\n      - ref: f\n        bind:\n          who: step\n",
            "@f\n?base\n?who\n?extra\n",
        )
        .expect("lowers");
        let text = only_entry(&lowered);
        assert!(text.contains("[Options]"), "{text}");
        assert!(text.contains(r#"variable: base="http://api""#), "{text}");
        assert!(
            text.contains(r#"variable: who="step""#),
            "step scope wins: {text}"
        );
        assert!(!text.contains(r#"who="macro""#) && !text.contains(r#"who="pack""#));
        assert!(text.contains(r#"variable: extra="yes""#), "{text}");
    }

    /// A secret binding never reaches the artifact; it is recorded as
    /// variable→secret so the engine injects it at run time (ADR-0005), which
    /// is what lets a fragment keep the variable name its corpus already uses.
    #[test]
    fn a_secret_binding_is_renamed_not_written() {
        let lowered = lower_fragments(
            "macros:\n  m:\n    match: it runs\n    bind:\n      auth_token: ${secret:apiToken}\n    steps:\n      - ref: f\n",
            "@f\n?auth_token\n",
        )
        .expect("lowers");
        let text = only_entry(&lowered);
        assert!(
            !text.contains("auth_token=") && !text.contains("apiToken"),
            "no secret may reach the artifact: {text}"
        );
        assert_eq!(
            lowered.secrets.get("auth_token").map(String::as_str),
            Some("apiToken")
        );
    }

    #[test]
    fn a_secret_mixed_into_a_larger_value_is_refused() {
        let diags = lower_fragments(
            "macros:\n  m:\n    match: it runs\n    bind:\n      auth: \"Bearer ${secret:apiToken}\"\n    steps:\n      - ref: f\n",
            "@f\n?auth\n",
        )
        .expect_err("should refuse");
        assert!(
            diags
                .iter()
                .any(|d| d.code == "proef::lower::secret_in_composite_bind"),
            "{diags:?}"
        );
    }

    /// `$${` is the escape (ADR-0005), so `$${secret:x}` is the literal text
    /// `${secret:x}` and names no secret at all. Reading the value with the
    /// resolver's own scanner is what keeps that true here — a second scanner
    /// that merely searched for `"${secret:"` refused this as a composite.
    #[test]
    fn an_escaped_secret_reference_is_a_literal_not_a_secret() {
        let lowered = lower_fragments(
            "macros:\n  m:\n    match: it runs\n    bind:\n      hint: $${secret:apiToken}\n    steps:\n      - ref: f\n",
            "@f\n?hint\n",
        )
        .expect("an escaped reference is ordinary text");
        assert!(
            lowered.secrets.is_empty(),
            "nothing was bound to a secret: {:?}",
            lowered.secrets
        );
        assert!(
            only_entry(&lowered).contains(r#"variable: hint="${secret:apiToken}""#),
            "the literal is injected verbatim: {}",
            only_entry(&lowered)
        );
    }

    /// A fragment that answers its own question needs no `bind:`. This is what
    /// keeps the file runnable under stock `hurl` with no variables file —
    /// ADR-0018's whole premise — so treating a self-supplied variable as
    /// unsupplied would refuse valid input that hurl itself accepts.
    #[test]
    fn a_variable_the_fragment_supplies_itself_needs_no_binding() {
        let lowered = lower_fragments(
            "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: first\n",
            "@first\n=token\n?token\n",
        )
        .expect("a fragment that supplies its own variable lowers");
        let text = lowered
            .batches
            .iter()
            .flat_map(|b| b.steps.iter())
            .find_map(|s| match &s.payload {
                StepPayload::HurlEntries(text) => Some(text.clone()),
                _ => None,
            })
            .expect("hurl entries");
        // The fragment's own line is the only one: proef must not also inject a
        // `variable: token=`, or the pair would resolve last-wins in the dark.
        assert_eq!(
            text.matches("variable: token=").count(),
            1,
            "exactly one supplier reaches the entry: {text}"
        );
    }

    /// hurl's `[Options] variable:` assigns into one shared set rather than
    /// scoping, so an unbound name would silently inherit whatever an earlier
    /// entry left behind — green, against the wrong value.
    #[test]
    fn a_placeholder_nothing_supplies_is_refused() {
        let diags = lower_fragments(
            "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: f\n",
            "@f\n?missingOne\n",
        )
        .expect_err("should refuse");
        let diag = diags
            .iter()
            .find(|d| d.code == "proef::lower::unbound_placeholder")
            .unwrap_or_else(|| panic!("expected unbound_placeholder in {diags:?}"));
        assert!(diag.message.contains("missingOne"), "{}", diag.message);
        assert!(diag.help.is_some());
    }

    /// Scope decides *when* a binding resolves, and that is observable through
    /// `${fake:…}`. One macro-scope binding is one value for every step of the
    /// macro — the shared identity a register-then-verify flow needs — while two
    /// separate bindings stay two values. The correctness series made repeated
    /// `${fake:…}` *occurrences* distinct; this keeps that true without making a
    /// single binding mean two different things.
    #[test]
    fn one_binding_is_one_value_across_a_macros_steps() {
        let lowered = lower_fragments(
            "macros:\n  m:\n    match: it runs\n    bind:\n      shared: ${fake:email}\n    steps:\n      - ref: first\n        bind:\n          own: ${fake:email}\n      - ref: second\n        bind:\n          own: ${fake:email}\n",
            "@first\n?shared\n?own\n@second\n?shared\n?own\n",
        )
        .expect("lowers");
        let entries: Vec<&str> = lowered
            .batches
            .iter()
            .flat_map(|b| b.steps.iter())
            .filter_map(|s| match &s.payload {
                StepPayload::HurlEntries(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(entries.len(), 2);
        let shared = |text: &str| {
            text.lines()
                .find(|l| l.starts_with("variable: shared="))
                .expect("shared binding")
                .to_owned()
        };
        let own = |text: &str| {
            text.lines()
                .find(|l| l.starts_with("variable: own="))
                .expect("own binding")
                .to_owned()
        };
        assert_eq!(
            shared(entries[0]),
            shared(entries[1]),
            "one macro-scope binding is one value for the whole macro"
        );
        assert_ne!(
            own(entries[0]),
            own(entries[1]),
            "two step-scope bindings are two values"
        );
    }

    /// A `ref:` step's `name:` is a **replay** of what the request was built
    /// from, exactly as an inline step's is: it must display the value the
    /// request actually sent, and must not consume an occurrence that shifts
    /// every later step's fakes by one. Both halves regressed when this path
    /// copied the inline tail without its counter rewind.
    #[test]
    fn a_ref_steps_label_replays_its_binding_instead_of_minting_a_fresh_value() {
        let labelled = lower_fragments(
            "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: first\n        name: signup ${fake:email}\n        bind:\n          who: ${fake:email}\n      - ref: second\n        bind:\n          who: ${fake:email}\n",
            "@first\n?who\n@second\n?who\n",
        )
        .expect("lowers");
        let steps: Vec<&LoweredStep> = labelled
            .batches
            .iter()
            .flat_map(|b| b.steps.iter())
            .collect();
        assert_eq!(steps.len(), 2);
        let who = |step: &LoweredStep| {
            let StepPayload::HurlEntries(text) = &step.payload else {
                unreachable!()
            };
            text.lines()
                .find_map(|l| l.strip_prefix("variable: who="))
                .expect("who binding")
                .trim_matches('"')
                .to_owned()
        };
        assert_eq!(
            steps[0].label.as_deref(),
            Some(format!("signup {}", who(steps[0])).as_str()),
            "the label must report the value its own binding sent"
        );

        // Same pack without the label: a label is a replay, so removing it
        // cannot change what any later step resolves to.
        let control = lower_fragments(
            "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: first\n        bind:\n          who: ${fake:email}\n      - ref: second\n        bind:\n          who: ${fake:email}\n",
            "@first\n?who\n@second\n?who\n",
        )
        .expect("lowers");
        let control_steps: Vec<&LoweredStep> = control
            .batches
            .iter()
            .flat_map(|b| b.steps.iter())
            .collect();
        assert_eq!(
            who(steps[1]),
            who(control_steps[1]),
            "a label must not shift a later step's fake values"
        );
    }

    mod properties {
        #![allow(clippy::ignored_unit_patterns)]

        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// **No secret value can reach an artifact through a binding, and a
            /// rename cannot lose which secret feeds which variable.**
            ///
            /// The rename is the risky part: `bind: { <var>: ${secret:<name>} }`
            /// deliberately decouples the two, and the engine later joins them
            /// again to call `insert_secret`. If the pair were dropped or
            /// crossed, either the request goes out unauthenticated or the wrong
            /// credential is sent — and the emitted text, which the artifact
            /// *is* (ADR-0010), must carry neither name as a value.
            #[test]
            fn a_renamed_secret_binds_by_name_and_never_by_value(
                variable in "[a-z][a-z_]{2,12}",
                secret in "[a-zA-Z][a-zA-Z0-9]{3,12}",
                literal in "[a-z][a-z0-9]{2,10}",
            ) {
                // `bind:` keys are distinct by construction; skip the collision
                // rather than assert on a pack that could not be written.
                prop_assume!(variable != "plain");
                let pack = format!(
                    "macros:\n  m:\n    match: it runs\n    bind:\n      {variable}: ${{secret:{secret}}}\n      plain: {literal}\n    steps:\n      - ref: f\n"
                );
                let fragments = format!("@f\n?{variable}\n?plain\n");
                let lowered = lower_fragments(&pack, &fragments)
                    .unwrap_or_else(|d| panic!("should lower: {d:?}"));
                let text = only_entry(&lowered);

                // The secret takes the `insert_secret` path, so the artifact
                // names it nowhere — not as a variable line, not as a value.
                let variable_line = format!("variable: {variable}=");
                prop_assert!(!text.contains(&variable_line));
                prop_assert!(!text.contains(&secret));
                // The literal beside it still takes the ordinary path, so the
                // assertion above cannot pass by injecting nothing at all.
                let plain_line = format!("variable: plain=\"{literal}\"");
                prop_assert!(text.contains(&plain_line));
                // And the pairing survives: variable → secret, not either alone.
                prop_assert_eq!(
                    lowered.secrets.get(&variable).map(String::as_str),
                    Some(secret.as_str())
                );
            }
        }
    }

    /// A capture from an earlier step counts as supplied — that is the whole
    /// point of chaining requests, and requiring a `bind:` for it would be wrong.
    #[test]
    fn a_capture_from_an_earlier_step_supplies_a_later_fragment() {
        lower_fragments(
            "macros:\n  m:\n    match: it runs\n    steps:\n      - ref: first\n      - ref: second\n",
            "@first\n!recordId\n@second\n?recordId\n",
        )
        .expect("a preceding capture supplies it");
    }
}
