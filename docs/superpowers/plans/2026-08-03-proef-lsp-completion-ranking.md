# Prefix-Aware Completion Ranking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `proef lsp` completion rank macro patterns by relevance to the partially-typed step prose, instead of falling back to suite order for partial input.

**Architecture:** A new sans-IO `matcher::prefix_rank` primitive in `proef-core` scores a pattern against a partial prefix in tiers (prefix / substring / prefix-aligned edit distance), reusing `literal_skeleton` + `levenshtein`. `completion::complete` sorts by it (stable → suite order breaks ties) and emits `sortText` + `filterText` so the editor orders and narrows correctly. Nothing is filtered server-side.

**Tech Stack:** Rust (edition 2024, toolchain 1.97.1), `proef-core::matcher`, `lsp-types` 0.97, `insta`-free `assert_cmd`/scripted-JSON-RPC integration tests.

## Global Constraints

- No new dependencies. `proef-core` stays sans-IO (no file/clock/env/random). Copy values verbatim from the spec `docs/superpowers/specs/2026-08-03-proef-lsp-completion-ranking-design.md`.
- One ranking implementation: `matcher::closest` is UNTOUCHED (it solves a different problem — single-best typo suggestion on whole steps for unbound-step diagnostics). `prefix_rank` is a new sibling, not a replacement, and completion stops calling `closest`.
- `proef-core` has `missing_docs = "warn"` — every new `pub` item needs a doc comment (prose, no task ids).
- NO task ids / task numbers in code comments or doc comments (they belong only in the changelog). NO AI-attribution trailers in commit messages.
- Work on the current branch `feat/proef-lsp` (the ranker modifies `completion.rs`, which exists only on this branch). Do NOT branch; do NOT touch main.
- The `proef-core` `public-api` snapshot gains exactly one line (`prefix_rank`) — regenerate/update `crates/proef-core/public-api.txt` (Task 2).
- Every task ends green:
  ```bash
  cargo fmt --all --check
  cargo clippy --all-targets --all-features -- -D warnings
  cargo nextest run --profile ci
  cargo test --doc
  cargo run -p xtask -- docs-check
  ```

## File Structure

- `crates/proef-core/src/matcher.rs` — add `pub fn prefix_rank` beside `closest`/`levenshtein`/`literal_skeleton`; add unit tests to the existing inline `#[cfg(test)] mod tests` (around `matcher.rs:313`).
- `crates/proef-lsp/src/features/completion.rs` — rewire `complete` (replace the `closest` block at lines ~51-61; add `sort_text`/`filter_text` in the item map at ~63-73).
- `crates/proef-lsp/tests/lsp.rs` — extend `completion_offers_macro_pattern_snippets` with ordering + `filter_text` assertions.
- `crates/proef-core/public-api.txt` — one added line (Task 2).

---

## Task 1: `matcher::prefix_rank` (proef-core)

**Files:**
- Modify: `crates/proef-core/src/matcher.rs` (add `prefix_rank` + unit tests)

**Interfaces:**
- Produces: `pub fn proef_core::matcher::prefix_rank(typed: &str, pattern: &str) -> (u8, usize)` — lower sorts first; `u8` is the tier (0 prefix, 1 substring, 2 fallback), `usize` is the in-tier tiebreak.
- Consumes: existing `matcher::literal_skeleton(pattern: &str) -> String` (`matcher.rs:236`) and `matcher::levenshtein(a: &str, b: &str) -> usize` (`matcher.rs:298`).

- [ ] **Step 1: Write the failing unit tests**

In `crates/proef-core/src/matcher.rs`, inside the existing `#[cfg(test)] mod tests` (starts around `matcher.rs:313`), add:

```rust
#[test]
fn prefix_rank_tiers_prefix_over_substring_over_miss() {
    // "I gr" is a prefix of "I greet {who}" (skeleton "I greet ") -> tier 0.
    let greet = prefix_rank("I gr", "I greet {who}");
    // "gr" appears inside "I grab {thing}" as a substring but not a prefix -> tier 1.
    let grab = prefix_rank("gr", "I grab {thing}");
    // "I gr" is neither prefix nor substring of "the note is saved" -> tier 2.
    let note = prefix_rank("I gr", "the note is saved");
    assert_eq!(greet.0, 0);
    assert_eq!(grab.0, 1);
    assert_eq!(note.0, 2);
    // Ordering: prefix < substring < miss.
    assert!(greet < grab);
    assert!(grab < note);
}

#[test]
fn prefix_rank_prefix_match_beats_large_full_pattern_distance() {
    // The bug fix: "I gr" is a full-pattern edit-distance of ~9 from
    // "I greet {who}" (so `closest` would reject it), but prefix_rank ranks it
    // top (tier 0) and well above an unrelated pattern.
    let greet = prefix_rank("I gr", "I greet {who}");
    let unrelated = prefix_rank("I gr", "the note is saved");
    assert!(greet < unrelated);
    // Sanity: closest, the old substrate, finds nothing at this distance.
    assert!(closest("I gr", ["I greet {who}"].into_iter()).is_none());
}

#[test]
fn prefix_rank_tier2_uses_prefix_aligned_distance_not_full_pattern() {
    // "I greex" is neither a prefix nor a substring of "I greet {who}" -> tier 2.
    // Its distance is measured against the LEADING 7 chars of the skeleton
    // ("i greet"), giving 1 — far smaller than against an unrelated pattern.
    let near = prefix_rank("I greex", "I greet {who}");
    let far = prefix_rank("I greex", "the note is saved");
    assert_eq!(near.0, 2);
    assert_eq!(far.0, 2);
    assert!(near.1 < far.1, "prefix-aligned distance ranks the near pattern first");
}

#[test]
fn prefix_rank_is_case_insensitive() {
    assert_eq!(prefix_rank("i gr", "I greet {who}").0, 0);
}

#[test]
fn prefix_rank_empty_typed_is_uniform_tier0() {
    // Empty prefix is a prefix of everything -> all (0, 0) -> stable order.
    assert_eq!(prefix_rank("", "I greet {who}"), (0, 0));
    assert_eq!(prefix_rank("", "the note is saved"), (0, 0));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p proef-core prefix_rank`
Expected: FAIL — `prefix_rank` is not defined.

- [ ] **Step 3: Implement `prefix_rank`**

Add to `crates/proef-core/src/matcher.rs`, directly below `literal_skeleton` (after `matcher.rs:244`):

```rust
/// Rank `pattern` against the partially-typed prose `typed`, for completion
/// ordering. Lower sorts first: the returned tuple is `(tier, tiebreak)`.
///
/// Comparison is against the pattern's [`literal_skeleton`] (captures dropped —
/// the prose the author actually types), case-insensitively, in tiers:
/// tier 0 the skeleton starts with `typed`; tier 1 `typed` occurs inside the
/// skeleton (tiebreak = the match position); tier 2 the edit distance between
/// `typed` and the skeleton's leading `typed`-length slice (the prefix-aligned
/// distance, not the whole-pattern distance). An empty `typed` is a prefix of
/// every skeleton, so all patterns share `(0, 0)` and keep their prior order.
///
/// This is a distinct problem from [`closest`], which finds the single most
/// likely mistyped *complete* step; the two coexist.
#[must_use]
pub fn prefix_rank(typed: &str, pattern: &str) -> (u8, usize) {
    let skeleton = literal_skeleton(pattern).to_lowercase();
    let typed = typed.to_lowercase();
    if skeleton.starts_with(&typed) {
        (0, 0)
    } else if let Some(idx) = skeleton.find(&typed) {
        (1, idx)
    } else {
        // Prefix-aligned distance: compare `typed` against only the leading
        // `typed`-length slice of the skeleton, so divergence past the typed
        // portion does not inflate the score the way whole-pattern distance does.
        let n = typed.chars().count();
        let prefix: String = skeleton.chars().take(n).collect();
        (2, levenshtein(&typed, &prefix))
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p proef-core prefix_rank`
Expected: PASS (all five).

- [ ] **Step 5: Run the focused gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run -p proef-core
```
Expected: green. (`prefix_rank` carries a doc comment, satisfying `missing_docs`.)

- [ ] **Step 6: Commit**

```bash
git add crates/proef-core/src/matcher.rs
git commit -m "feat(core): add matcher::prefix_rank for partial-prefix completion ranking"
```

---

## Task 2: rewire completion + integration test + public-api

**Files:**
- Modify: `crates/proef-lsp/src/features/completion.rs` (`complete`)
- Modify: `crates/proef-lsp/tests/lsp.rs` (`completion_offers_macro_pattern_snippets`)
- Modify: `crates/proef-core/public-api.txt` (one added line)

**Interfaces:**
- Consumes: `matcher::prefix_rank` (Task 1) and existing `matcher::literal_skeleton`.
- Produces: no new public API (LSP internals + a docs/test change).

- [ ] **Step 1: Rewire `complete` to sort by `prefix_rank` and set sortText/filterText**

In `crates/proef-lsp/src/features/completion.rs`, replace the ranking block (currently lines ~51-61, the `let mut patterns …` collection plus the `if let Some(best) = matcher::closest(…)` sort) with a `prefix_rank` stable sort. Rust's `sort_by_key` is stable, so equal ranks keep suite order — that IS the `(prefix_rank, original_index)` ordering, no explicit index needed:

```rust
    // Rank patterns by relevance to the typed prose prefix (best first);
    // equal ranks keep suite order (stable sort). Patterns with no `match:`
    // (use-only macros) are skipped.
    let mut patterns: Vec<(&str, &str)> = analysis
        .suite
        .macros
        .iter()
        .filter_map(|m| m.pattern.as_deref().map(|p| (m.name.as_str(), p)))
        .collect();
    patterns.sort_by_key(|(_, p)| matcher::prefix_rank(&prefix, p));
```

Then replace the item-building map (currently lines ~63-73) so each item carries a zero-padded
`sort_text` (from its post-sort position, so the client honors our order) and a `filter_text`
(the pattern's literal skeleton, so the client narrows against prose):

```rust
    // Zero-pad the rank index so the client's lexical sortText comparison
    // matches our numeric order (e.g. "00", "01", … for up to 99 items).
    let width = patterns.len().to_string().len();
    patterns
        .into_iter()
        .enumerate()
        .map(|(i, (macro_name, pattern))| CompletionItem {
            label: pattern.to_owned(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(format!("macro {macro_name}")),
            insert_text: Some(pattern_to_snippet(pattern)),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            sort_text: Some(format!("{i:0width$}")),
            filter_text: Some(matcher::literal_skeleton(pattern)),
            ..Default::default()
        })
        .collect()
```

Update the `complete` doc comment (lines ~38-42) to describe the new behavior (rank via
`prefix_rank`; emit sortText/filterText; nothing hidden) — prose, no task ids. The
`use proef_core::matcher;` import stays; `matcher::closest` is simply no longer called here.

- [ ] **Step 2: Verify no leftover `closest` reference and it compiles**

Run: `cargo build -p proef-lsp`
Expected: compiles. If clippy later flags an unused import, note that `matcher` is still used (`prefix_rank`, `literal_skeleton`) so the import stays; only remove a symbol if genuinely unused.

- [ ] **Step 3: Extend the integration test with ordering + filterText assertions**

Open `crates/proef-lsp/tests/lsp.rs` and READ the current `completion_offers_macro_pattern_snippets` test (it opens a feature typing `"When I gr"` with a pack defining `greet: match "I greet {who}"`, and asserts a snippet item with a `${1:` tabstop). Extend it:

1. Add a SECOND macro to the test's pack YAML that does NOT prefix-match the typed `"I gr"` — e.g. under `macros:` add:
   ```yaml
     saved:
       match: "the note is saved"
       steps:
         - hurl: |
             GET http://x
   ```
2. After collecting the returned `items`, add assertions (adapt the item-lookup to the test's existing style):

```rust
    // The prefix-matching macro sorts ahead of the non-matching one.
    let greet = items
        .iter()
        .find(|i| i.label == "I greet {who}")
        .expect("greet completion offered");
    let saved = items
        .iter()
        .find(|i| i.label == "the note is saved")
        .expect("saved completion offered");
    assert!(
        greet.sort_text < saved.sort_text,
        "prefix match must sort first: greet={:?} saved={:?}",
        greet.sort_text,
        saved.sort_text
    );
    // filterText is the pattern's prose skeleton, so the editor narrows on prose.
    assert_eq!(greet.filter_text.as_deref(), Some("I greet "));
```

- [ ] **Step 4: Run the LSP tests to verify they pass**

Run: `cargo nextest run -p proef-lsp completion`
Expected: PASS — `completion_offers_macro_pattern_snippets` now asserts ordering + filterText, and still asserts the SNIPPET tabstop.

- [ ] **Step 5: Regenerate the public-api baseline**

`prefix_rank` adds one line to `proef-core`'s public surface. Run:

```bash
cargo run -p xtask -- public-api
```

If it succeeds and reports drift, follow its output to update `crates/proef-core/public-api.txt`, then re-run until clean. If the local toolchain cannot run nightly `cargo public-api` (a known constraint on this machine), add the single line by hand to `crates/proef-core/public-api.txt` in its correct sorted position among the `matcher` entries, matching the exact rendered form of its siblings (`closest`, `levenshtein`, `literal_skeleton`) — the line is:
```
pub fn proef_core::matcher::prefix_rank(typed: &str, pattern: &str) -> (u8, usize)
```
The nightly CI `public-api` job is the authoritative gate; ensure the baseline reflects exactly this one addition and nothing else.

- [ ] **Step 6: Full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
cargo run -p xtask -- docs-check
```
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add crates/proef-lsp/src/features/completion.rs crates/proef-lsp/tests/lsp.rs crates/proef-core/public-api.txt
git commit -m "feat(lsp): rank completion by prefix relevance with sortText/filterText"
```

---

## Self-Review

**1. Spec coverage:**
- §2 rank-don't-hide (sortText + filterText, no server filtering) → Task 2 Step 1 (both fields set; every pattern returned). ✓
- §3 `prefix_rank` (tiers 0/1/2, prefix-aligned distance, case-insensitive, empty→Tier 0 stable, coexists with `closest`) → Task 1 (impl + 5 unit tests, one per property). ✓
- §4 rewire (drop `closest`-promote, stable sort by `prefix_rank`, sortText zero-pad, filterText skeleton, label/insert_text unchanged) → Task 2 Step 1. ✓
- §5 testing (prefix_rank unit tests; extend the integration test for ordering + filterText) → Task 1 Step 1 + Task 2 Step 3. ✓
- §6 scope (no deps; sans-IO; one impl; public-api +1 line) → Global Constraints + Task 2 Step 5. ✓

**2. Placeholder scan:** No TBD/TODO/"handle edge cases". All code blocks are concrete. Task 2 Step 3 instructs reading the current test before extending (the test may have drifted since Task 5) but supplies the exact YAML addition and assertion code — not a placeholder.

**3. Type consistency:** `prefix_rank(typed: &str, pattern: &str) -> (u8, usize)` is defined in Task 1 and consumed in Task 2's `sort_by_key` identically. `literal_skeleton(pattern) -> String` matches its use as `filter_text`. `sort_text`/`filter_text` are `Option<String>` on `CompletionItem` (0.97) — set via `Some(String)`. The empty-typed → `(0,0)` claim in Task 1's test matches the impl's `starts_with("")` branch. Consistent.
