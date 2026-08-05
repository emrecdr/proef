# Design — `use:` / `match:` go-to-definition (proef LSP)

**Status:** approved design (pre-implementation) · **Date:** 2026-08-04 · **Owner:** Emre
**Follows:** [ADR-0017](../../adr/ADR-0017-lsp-language-server.md) (the LSP) ·
**Touches:** `proef-core` (`pack::locate`, `pack`, `analyze`), `proef-lsp::features::definition`

## 1. Motivation

v1 go-to-definition resolves a **feature step → the macro that binds it**, landing on the macro's
*name key* in the pack. Two documented gaps remain (recorded only in a source comment today):

- **Gap A — `use:` references are a navigation dead-end.** A `use: <target>` line inside a pack
  points at another macro, but the cursor there jumps nowhere (`definition.rs::goto` returns
  `None` for any position in a pack file). `use:` composition is a first-class pack feature
  (ADR-0004), so this is a real authoring papercut — you have to grep the target name by hand.
- **Gap B — go-to-def lands on the macro's *name key*, not its `match:` line.** The name key
  gets you to the right macro block, but the `match:` pattern is what a step author is usually
  looking for.

This design closes both in one combined change. It is pure LSP feature-completeness — no
architecture, engine, or seam impact.

## 2. Decision

Add two **best-effort text-scan locators** in `proef-core::pack::locate` — the same idiom as the
three that already exist there (`serde_norway` yields no byte spans for YAML, so *every* span in
`locate.rs` is a text scan). Build the `use:`-reference index at **analyze time**, which avoids
touching the pack parser/normalizer entirely.

Rejected alternative (a `target_span` field on `MacroStepKind::Use` populated in the parser):
because serde gives no spans, that field would *still* be text-scan-populated — it buys no extra
accuracy over a locator, while adding a `normalize_step` signature change + ordinal threading.
Not worth it.

## 3. Two new locators — `pack::locate`

Both scan a macro's block region (reuse the existing private `macro_region` helper), return
`Option<Span>`, and degrade to `None` on anything unexpected (block scalars, unusual quoting) —
never panic. Spans are 0-based byte offsets into the *normalized* pack source, like the existing
locators.

- `pub fn use_span(text: &str, macro_name: &str, ordinal: usize) -> Option<Span>` — the byte span
  of the `ordinal`-th `use:` key line within `macro_name`'s block (mirrors `payload_line_span`'s
  ordinal approach, already trusted for engine-probe error mapping).
- `pub fn match_span(text: &str, macro_name: &str) -> Option<Span>` — the byte span of the
  `match:` key line within `macro_name`'s block (exactly one per macro, so no ordinal).

## 4. Gap A — `use:`-reference navigation (analyze-time index)

`analyze_suite` already holds every `Macro` (with `.source`, `.name`, `.body`). Extend it to
build a new relation, walking each `MacroBody::Steps` and, for each `MacroStepKind::Use { target,
.. }` (tracking its ordinal among that macro's `use:` steps):

```rust
// proef_core::analyze
pub struct UseRef {
    pub pack: String,          // source name of the pack the use: line lives in
    pub span: Span,            // byte span of the `use:` line in the normalized pack source
    pub target_macro: String,  // resolved target macro name
}
// SuiteAnalysis gains:
pub use_refs: Vec<UseRef>,
```

Population, per `Use` step: `locate::use_span(&macro.source, &macro.name, ordinal)` → if `Some`,
resolve the target with the existing `PackSet::find_use_target(target)`; record a `UseRef` only
when both the span *and* a resolved target exist (an unresolved `use:` is already a
`proef::pack::unknown_use` diagnostic — not this feature's concern).

`definition.rs::goto` gains a **pack-position path**, tried when the feature-binding lookup
misses: if the cursor's document is a pack (its name matches a `UseRef.pack`), find the `UseRef`
whose `span` contains the offset (half-open `start <= off < end`) → return a `Location` at the
resolved target macro's definition anchor (§5) in *that macro's* pack, built with a `LineIndex`
over the target pack's raw text (already in `Analysis.raw`).

## 5. Gap B — retarget onto the `match:` line

- `Macro` gains `pub match_span: Option<Span>`, populated in `normalize_macro` beside the existing
  `locate::macro_span` call: `locate::match_span(&source.text, name)`.
- `MacroRef` gains `pub match_span: Option<Span>`, copied in the `analyze_suite` macro loop.
- The definition anchor becomes `m.match_span.or(m.def_span)` — the `match:` line when locatable,
  else the name key (use-only macros have no `match:`; the fallback keeps them navigable). This
  anchor is used by **both** the feature-step path and the new Gap-A pack path (§4), so the two
  features share one destination rule.

## 6. Robustness / error handling

- Locators return `None` on any unexpected shape → Gap A silently records no `UseRef` for that
  step (the `use:` line is simply not navigable); Gap B falls back to the name key. No panics, no
  diagnostics, no user-visible error — the feature is additive and fail-soft.
- A definition request on a pack position that isn't a recorded `use:` line → `None` (no-op), as
  before.
- `proef-core` stays sans-IO (pure text scanning). No new dependencies.

## 7. Testing

- **`pack::locate` unit tests** — `use_span` (single + multiple `use:` steps → correct ordinal
  line) and `match_span` (locatable + a shape it declines to `None`), mirroring the existing
  `macro_names_are_located` / `payload_lines_map_back_to_the_file` tests.
- **`analyze` unit test** — over an in-memory pack with a `use:` macro, assert `use_refs` records
  the `(pack, span, target_macro)` and that the span covers the `use:` line.
- **LSP integration tests (`tests/lsp.rs`)** — (a) open a **pack** document, request definition on
  a `use:` line, assert the `Location` is the target macro in its pack; (b) request definition on
  a feature step and assert the range now lands on the **`match:`** line (not the name key).

## 8. Scope / non-goals

- No parser/normalizer change (analyze-time scanning). No new deps. `proef-core` sans-IO.
- One implementation per outcome: the `match:` anchor rule (§5) is shared by both go-to-def paths;
  the locators live with their siblings in `locate.rs`.
- Public-api snapshot moves by three additions (`UseRef`, `Macro.match_span`, `MacroRef.match_span`)
  — a deliberate, reviewed change; regenerate the `proef-core` baseline.
- Not find-references-from-a-macro-def, not rename, not hover — out of scope.
- **ADR:** amend ADR-0017 with a short note that these two go-to-def targets (formerly v1
  scope-cuts) are now implemented, closing the source-comment-only deviation.
