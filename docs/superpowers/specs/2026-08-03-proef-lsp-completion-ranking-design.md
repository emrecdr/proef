# Design — prefix-aware completion ranking (proef LSP)

**Status:** approved design (pre-implementation) · **Date:** 2026-08-03 · **Owner:** Emre
**Follows:** [ADR-0017](../../adr/ADR-0017-lsp-language-server.md) (the LSP) ·
**Touches:** `proef-core::matcher`, `proef-lsp::features::completion`

## 1. Motivation

`proef lsp` completion offers every macro `match:` pattern as a snippet but ranks them with
`proef_core::matcher::closest`, which is Levenshtein-distance-≤3 against the **full** pattern
string. For a partial step prefix — `"I gr"` against `"I greet {who}"` (distance ≈ 9) — the
threshold is exceeded, `closest` returns `None`, and the list falls back to suite order. In
practice completion is *offered but unranked* for the common case (partial typing). This makes
the ranking meaningful for partial input.

## 2. Decision

**Rank, don't hide.** The server returns every pattern (no server-side filtering), but computes
a prefix-aware relevance order and hands the client two signals so it orders and narrows
correctly:

- `sortText` — a zero-padded rank index reflecting the server's ordering.
- `filterText` — the pattern's literal skeleton (prose with captures dropped), so the editor's
  own incremental filtering narrows against the prose the author actually types, not the
  `{capture}`-laden pattern.

Nothing is hidden by the server; Neovim / Helix / Emacs each do live narrowing as the author
types more. This respects the client's role rather than duplicating or fighting it.

## 3. The ranking primitive — `matcher::prefix_rank`

A new sans-IO, dependency-free `pub fn` in `proef-core::matcher`, beside the existing primitives
and reusing `literal_skeleton` + `levenshtein`:

```rust
/// Rank `pattern` against the partially-typed prose `typed`, for completion
/// ordering. Lower sorts first. Compares against the pattern's literal
/// skeleton (captures dropped) so partial input ranks sensibly — unlike
/// `closest`, which scores whole mistyped steps for "did you mean".
pub fn prefix_rank(typed: &str, pattern: &str) -> (u8, usize);
```

Scoring against `skeleton = literal_skeleton(pattern)`, **case-insensitively**, in tiers (the
`u8`); the `usize` is the in-tier tiebreak (lower first):

- **Tier 0** — `skeleton` starts with `typed`. Tiebreak `0`. (The case today's code misses.)
- **Tier 1** — `typed` is a substring of `skeleton` (but not a prefix). Tiebreak = the byte
  index where `typed` first occurs (earlier match ranks higher).
- **Tier 2** — fallback: tiebreak = `levenshtein(typed, &skeleton_prefix)` where `skeleton_prefix`
  is the leading `typed.chars().count()` characters of the skeleton (char-boundary safe) — the
  **prefix-aligned** distance, not the full-pattern distance. This is the specific bug fix.

Empty `typed`: `skeleton.starts_with("")` is universally true, so every pattern is Tier 0 with
tiebreak `0` → equal keys → the caller's stable sort preserves suite order. Case-folding uses
`str::to_lowercase` on both sides (ASCII-dominant prose; no new deps).

`prefix_rank` and `closest` solve **different** problems and legitimately coexist (not "two ways
to do one thing"): `closest` finds the single most-likely-**mistyped complete** step for
unbound-step diagnostics (typo distance on whole strings, single best, threshold-gated);
`prefix_rank` produces a **total ordering** of patterns against a **partial** prefix for
completion. `closest` is untouched.

## 4. Rewire — `completion::complete`

Replace the current `closest`-based "promote the single best to the front" block with:

1. Collect the candidate `(macro_name, pattern)` pairs as today (macros without a `match:`
   pattern are still skipped).
2. Stable-sort by `(matcher::prefix_rank(&prefix, pattern), original_index)` — `original_index`
   as the final tiebreak keeps the order deterministic across equal ranks.
3. On each emitted `CompletionItem`, additionally set:
   - `sort_text: Some(format!("{i:0width$}", ...))` — `i` is the item's post-sort position,
     `width` chosen from the candidate count so the zero-padding sorts lexically (e.g. 3 for
     up to 999 items).
   - `filter_text: Some(matcher::literal_skeleton(pattern))`.

`label` (full pattern), `kind` (SNIPPET), `detail`, and `insert_text` (the snippet) are
unchanged. `pattern_to_snippet` and `current_step_prefix` are unchanged.

## 5. Testing

- **`matcher::prefix_rank` unit tests** (inline `#[cfg(test)] mod tests` in `matcher.rs`):
  prefix beats substring beats non-match; a prefix match outranks a large full-pattern distance
  (`"I gr"` ranks `"I greet {who}"` above `"the note is saved"`); empty `typed` yields equal
  keys (stable order); case-insensitivity (`"i gr"` matches `"I greet"`); the Tier 2 tiebreak is
  the **prefix-aligned** distance, not the full-pattern distance (a pattern whose skeleton
  diverges only after the typed prefix still ranks well).
- **Extend the LSP integration test** `completion_offers_macro_pattern_snippets`
  (`crates/proef-lsp/tests/lsp.rs`): with a feature typing `"When I gr"` and a pack defining a
  prefix-matching macro plus a non-matching one, assert the prefix-matching item has the lowest
  `sort_text` and that its `filter_text` equals the pattern's skeleton.

## 6. Scope / non-goals

- No new dependencies. `proef-core` stays sans-IO. One ranking implementation (in `matcher`,
  reused by the LSP — the CLI has no completion consumer today).
- No server-side filtering/hiding (§2 decision).
- Not subsequence/fuzzy scoring: tiers + prefix-aligned distance cover partial typing, and the
  client does fuzzy narrowing. A richer scorer is a future refinement, not v1.
- `public-api` snapshot moves by exactly one line (`matcher::prefix_rank`) — a deliberate,
  reviewed addition; regenerate the `proef-core` baseline.
