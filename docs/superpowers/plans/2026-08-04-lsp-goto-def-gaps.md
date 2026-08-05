# LSP `use:` / `match:` Go-to-Definition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend `proef lsp` go-to-definition so it jumps *from* a `use:` reference in a pack to the target macro, and lands *on* a macro's `match:` line (name-key fallback).

**Architecture:** Two best-effort text-scan locators in `proef-core::pack::locate` (the same idiom as the three that already live there — serde gives no YAML spans). A `use:`-reference index (`SuiteAnalysis.use_refs`) is built at analyze time by walking each macro's steps (no parser/normalizer change). The LSP `definition` handler gains a pack-position path and a shared `match:`-or-name-key destination rule.

**Tech Stack:** Rust (edition 2024, toolchain 1.97.1), `proef-core` (sans-IO), `proef-lsp` (`lsp-types` 0.97), `insta`-free scripted-JSON-RPC integration tests.

## Global Constraints

- `proef-core` stays sans-IO (pure text scanning; no fs/clock/env/random). No new dependencies.
- **No parser/normalizer signature change** — the `use:` index is built at analyze time.
- Existing locators (`macro_span`, `macro_region`, `payload_line_span`) are untouched except for additions; the `macro_region` helper is reused, not duplicated.
- **One implementation per outcome:** the destination rule `match_span.or(def_span)` lives in a single `definition.rs` helper used by *both* go-to-def paths.
- Spans are 0-based **byte** offsets into the *normalized* pack source; span containment is half-open (`start <= offset < end`).
- NO task ids / task numbers in code comments or doc comments (changelog only). NO AI-attribution trailers in commit messages.
- Work on branch `feat/lsp-goto-def-gaps` (already checked out, off `main` at v0.5.0). Do NOT branch further; do NOT touch main.
- Public-api snapshot moves (new public struct + fields) — regenerate `crates/proef-core/public-api.txt` (Task 4).
- LSP integration tests must stay green on **Windows** — reuse the OS-portable `native_abs()` helper and the shared helpers already in `crates/proef-lsp/tests/lsp.rs`.
- Every task ends green:
  ```bash
  cargo fmt --all --check
  cargo clippy --all-targets --all-features -- -D warnings
  cargo nextest run --profile ci
  cargo test --doc
  cargo run -p xtask -- docs-check
  ```

## File Structure

- `crates/proef-core/src/pack/locate.rs` — add `use_span`, `match_span`, and a shared private `key_line_span` helper; make the module reachable from `analyze`.
- `crates/proef-core/src/pack/mod.rs` — `mod locate;` → `pub(crate) mod locate;`; add `Macro.match_span`.
- `crates/proef-core/src/pack/validate.rs` — populate `Macro.match_span` in `normalize_macro`.
- `crates/proef-core/src/analyze.rs` — add `UseRef`, `SuiteAnalysis.use_refs`, `MacroRef.match_span`; build the index in `analyze_suite`.
- `crates/proef-lsp/src/features/definition.rs` — pack-position path + shared destination helper; update module doc.
- `crates/proef-lsp/tests/lsp.rs` — two integration tests.
- Docs: `docs/adr/ADR-0017-lsp-language-server.md`, `docs/CHANGELOG.md`, `crates/proef-core/public-api.txt`.

---

## Task 1: Two locators in `pack::locate`

**Files:**
- Modify: `crates/proef-core/src/pack/locate.rs` (add `use_span`, `match_span`, `key_line_span`, tests)
- Modify: `crates/proef-core/src/pack/mod.rs` (`mod locate;` → `pub(crate) mod locate;`)

**Interfaces:**
- Produces:
  - `pub(crate) fn locate::use_span(text: &str, macro_name: &str, ordinal: usize) -> Option<Span>` — content span of the `ordinal`-th (0-based) `use:` line in `macro_name`'s block.
  - `pub(crate) fn locate::match_span(text: &str, macro_name: &str) -> Option<Span>` — content span of `macro_name`'s `match:` line.
- Consumes: the existing private `macro_region` (`locate.rs:28`) and `lines_with_offsets` (`locate.rs:95`).

- [ ] **Step 1: Write the failing locator tests**

In `crates/proef-core/src/pack/locate.rs`, inside `#[cfg(test)] mod tests` (after the existing tests, ~line 126), add a fixture with `use:` and `match:` lines plus the two tests:

```rust
    const USE_PACK: &str = "macros:\n  base:\n    match: the base\n    steps:\n      - hurl: |\n          GET http://x\n  wrapper:\n    steps:\n      - use: base\n      - use: base#other\n";

    #[test]
    fn match_lines_are_located() {
        let span = match_span(USE_PACK, "base").expect("span");
        assert_eq!(&USE_PACK[span.start..span.end], "match: the base");
        // A macro with no `match:` (use-only) yields None.
        assert!(match_span(USE_PACK, "wrapper").is_none());
        assert!(match_span(USE_PACK, "absent").is_none());
    }

    #[test]
    fn use_lines_are_located_by_ordinal() {
        let first = use_span(USE_PACK, "wrapper", 0).expect("span");
        assert_eq!(&USE_PACK[first.start..first.end], "use: base");
        let second = use_span(USE_PACK, "wrapper", 1).expect("span");
        assert_eq!(&USE_PACK[second.start..second.end], "use: base#other");
        // Ordinal past the last `use:` → None (never panics).
        assert!(use_span(USE_PACK, "wrapper", 2).is_none());
        // A macro with no `use:` → None.
        assert!(use_span(USE_PACK, "base", 0).is_none());
    }
```

- [ ] **Step 2: Run to verify the tests fail**

Run: `cargo nextest run -p proef-core locate`
Expected: FAIL — `use_span` / `match_span` undefined.

- [ ] **Step 3: Implement the two locators + the shared helper**

In `crates/proef-core/src/pack/locate.rs`, add (after `payload_line_span`, before `lines_with_offsets`):

```rust
/// Content span of the `ordinal`-th (0-based) `use:` line within `macro_name`'s
/// block — the whole reference line (indent and any `- ` sequence dash stripped),
/// so a cursor anywhere on the reference resolves. `None` when not locatable.
pub(crate) fn use_span(text: &str, macro_name: &str, ordinal: usize) -> Option<Span> {
    key_line_span(text, macro_name, "use", ordinal)
}

/// Content span of a macro's `match:` line (there is at most one), when
/// locatable — the go-to-definition landing anchor. `None` otherwise.
pub(crate) fn match_span(text: &str, macro_name: &str) -> Option<Span> {
    key_line_span(text, macro_name, "match", 0)
}

/// The `ordinal`-th line in `macro_name`'s block whose content (after stripping a
/// leading `- ` sequence dash) begins `<key>:`, returned as that line's trimmed
/// content span. Shared by [`use_span`] and [`match_span`]; best-effort, never panics.
fn key_line_span(text: &str, macro_name: &str, key: &str, ordinal: usize) -> Option<Span> {
    let (begin, end) = macro_region(text, macro_name)?;
    let region = &text[begin..end];
    let mut seen = 0usize;
    for (offset, line) in lines_with_offsets(region) {
        let trimmed = line.trim_start();
        let lead = line.len() - trimmed.len();
        let after_dash = trimmed.strip_prefix("- ").unwrap_or(trimmed);
        let dash = trimmed.len() - after_dash.len();
        if after_dash.starts_with(&format!("{key}:")) {
            if seen == ordinal {
                let start = begin + offset + lead + dash;
                let stop = begin + offset + line.trim_end().len();
                return Some(Span::clamped(start, stop.max(start), text.len()));
            }
            seen += 1;
        }
    }
    None
}
```

Then make the module reachable from `analyze` (Task 2 calls `crate::pack::locate::use_span`): in `crates/proef-core/src/pack/mod.rs`, change the `locate` module declaration from `mod locate;` to `pub(crate) mod locate;`. (This exposes it crate-internally only — no public-api impact.)

- [ ] **Step 4: Run to verify the tests pass**

Run: `cargo nextest run -p proef-core locate`
Expected: PASS (both new tests + the two pre-existing locate tests).

- [ ] **Step 5: Focused gate + commit**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run -p proef-core
git add crates/proef-core/src/pack/locate.rs crates/proef-core/src/pack/mod.rs
git commit -m "feat(core): add use:/match: line locators in pack::locate"
```

---

## Task 2: `Macro.match_span` + the `use_refs` analyze index

**Files:**
- Modify: `crates/proef-core/src/pack/mod.rs` (add `Macro.match_span`)
- Modify: `crates/proef-core/src/pack/validate.rs` (populate it in `normalize_macro`)
- Modify: `crates/proef-core/src/analyze.rs` (`UseRef`, `SuiteAnalysis.use_refs`, `MacroRef.match_span`, index build)

**Interfaces:**
- Consumes: `locate::use_span` / `locate::match_span` (Task 1); `PackSet::find_use_target` (`pack/mod.rs:132`); `MacroBody::Steps` / `MacroStepKind::Use` (`pack/mod.rs:182,219`).
- Produces:
  - `Macro.match_span: Option<Span>` (`pack/mod.rs`).
  - `pub struct analyze::UseRef { pub pack: String, pub span: Span, pub target_macro: String }`.
  - `analyze::SuiteAnalysis.use_refs: Vec<UseRef>`.
  - `analyze::MacroRef.match_span: Option<Span>`.

- [ ] **Step 1: Add `Macro.match_span` and populate it**

In `crates/proef-core/src/pack/mod.rs`, add a field to `Macro` (after `span`, ~line 176):

```rust
    /// Span of the macro's `match:` line in the pack file, when locatable.
    pub match_span: Option<Span>,
```

In `crates/proef-core/src/pack/validate.rs`, in `normalize_macro`, next to the existing `macro_span` call (line 40):

```rust
    let span = locate::macro_span(&source.text, name);
    let match_span = locate::match_span(&source.text, name);
```

and add `match_span,` to the `Macro { … }` literal returned at the end of `normalize_macro` (~line 125, beside `span`):

```rust
    Some(Macro {
        // …existing fields…
        span,
        match_span,
    })
```

- [ ] **Step 2: Write the failing `use_refs` unit test**

In `crates/proef-core/src/analyze.rs`, inside its `#[cfg(test)] mod tests` (reuse the existing `MemProvider` / `ctx_over` helpers there — read them first), add:

```rust
    #[test]
    fn analyze_records_use_refs_and_match_spans() {
        let mut files = std::collections::BTreeMap::new();
        files.insert(
            "packs/p.yaml".to_owned(),
            std::sync::Arc::from(
                "macros:\n  base:\n    match: the base\n    steps:\n      - hurl: |\n          GET http://x\n  wrapper:\n    steps:\n      - use: base\n",
            ),
        );
        let provider = MemProvider {
            features: vec![],
            packs: vec!["packs/p.yaml".to_owned()],
            files,
        };
        let empty = std::collections::BTreeMap::new();
        let analysis = analyze_suite(&ctx_over(&provider, &empty));

        // The `use: base` line is indexed, resolved to `base`.
        let u = analysis
            .use_refs
            .iter()
            .find(|u| u.target_macro == "base")
            .expect("use_ref for base");
        assert_eq!(u.pack, "packs/p.yaml");
        let src = &provider_text(&provider, "packs/p.yaml");
        assert_eq!(&src[u.span.start..u.span.end], "use: base");

        // `base` carries a match_span; `wrapper` (use-only) does not.
        let base = analysis.macros.iter().find(|m| m.name == "base").unwrap();
        assert!(base.match_span.is_some());
        let wrapper = analysis.macros.iter().find(|m| m.name == "wrapper").unwrap();
        assert!(wrapper.match_span.is_none());
    }

    // Small helper to read a source back for span assertions.
    fn provider_text(p: &MemProvider, name: &str) -> String {
        use crate::provider::SourceProvider;
        p.read(name).unwrap().to_string()
    }
```

> If the existing `MemProvider` in `analyze.rs` tests has different field names or a different constructor than `{ features, packs, files }`, adapt the fixture to match it (read the existing `analyze_surfaces_bindings_and_no_errors_on_a_clean_suite` test for the exact shape) — keep the assertions identical.

- [ ] **Step 3: Run to verify it fails**

Run: `cargo nextest run -p proef-core analyze_records_use_refs`
Expected: FAIL — `use_refs` / `MacroRef.match_span` undefined.

- [ ] **Step 4: Add the analyze types and build the index**

In `crates/proef-core/src/analyze.rs`:

1. Add the `UseRef` struct near `Binding`/`MacroRef` (after `MacroRef`, ~line 47):

```rust
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
```

2. Add `match_span` to `MacroRef` (after `def_span`, ~line 46):

```rust
    /// Byte span of the macro's `match:` line, when locatable — the preferred
    /// go-to-definition landing anchor (falls back to `def_span`).
    pub match_span: Option<Span>,
```

3. Add the collection to `SuiteAnalysis` (after `macros`, ~line 57):

```rust
    /// Every `use:` reference across the loaded packs, resolved to its target.
    pub use_refs: Vec<UseRef>,
```

4. In `analyze_suite`, in the macro loop (~line 124), copy `match_span`:

```rust
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
```

5. Immediately after that loop, build the `use_refs` index (add the needed imports `use crate::pack::{MacroBody, MacroStepKind};` at the top of the file):

```rust
    // Index every `use:` reference → its resolved target, for go-to-def from a
    // `use:` line. The per-macro ordinal matches locate::use_span's counting.
    for m in packs.macros.values() {
        let MacroBody::Steps(steps) = &m.body else { continue };
        let mut ordinal = 0usize;
        for step in steps {
            if let MacroStepKind::Use { target, .. } = &step.kind {
                if let Some(span) = crate::pack::locate::use_span(&m.source, &m.name, ordinal) {
                    if let Some(target_macro) = packs.find_use_target(target) {
                        out.use_refs.push(UseRef {
                            pack: m.pack.clone(),
                            span,
                            target_macro: target_macro.name.clone(),
                        });
                    }
                }
                ordinal += 1;
            }
        }
    }
```

- [ ] **Step 5: Run to verify the test passes + existing core tests stay green**

Run: `cargo nextest run -p proef-core`
Expected: PASS — the new test plus every pre-existing proef-core test (adding fields with sensible copies must not break `bind`/`pack`/`analyze` tests).

- [ ] **Step 6: Focused gate + commit**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run -p proef-core
git add crates/proef-core/src/pack/mod.rs crates/proef-core/src/pack/validate.rs crates/proef-core/src/analyze.rs
git commit -m "feat(core): index use: references and macro match: spans in analyze_suite"
```

---

## Task 3: LSP definition — pack path + `match:` retarget

**Files:**
- Modify: `crates/proef-lsp/src/features/definition.rs`
- Modify: `crates/proef-lsp/tests/lsp.rs`

**Interfaces:**
- Consumes: `Analysis.suite.use_refs` (Task 2), `MacroRef.match_span`/`def_span`/`pack` (Task 2), `LineIndex`, `name_to_url`/`url_to_name`.
- Produces: no new public API (LSP internals + tests).

- [ ] **Step 1: Rewrite `goto` with a shared destination helper + the pack path**

Replace the body of `goto` in `crates/proef-lsp/src/features/definition.rs` (lines 21-48) with two lookup paths funnelling through one `macro_location` helper (the single `match_span.or(def_span)` rule):

```rust
pub fn goto(analysis: &Analysis, url: &Uri, position: Position) -> Option<Location> {
    let name = url_to_name(url);
    let raw = analysis.raw.get(&name)?;
    let offset = LineIndex::new(raw).position_to_offset(position);

    // Path 1 — cursor on a feature step → the macro it binds.
    if let Some(macro_name) = analysis
        .suite
        .bindings
        .iter()
        .find(|b| b.feature == name && b.step_span.start <= offset && offset < b.step_span.end)
        .map(|b| b.macro_name.as_str())
    {
        return macro_location(analysis, macro_name);
    }

    // Path 2 — cursor on a `use:` line in a pack → the referenced macro.
    if let Some(target) = analysis
        .suite
        .use_refs
        .iter()
        .find(|u| u.pack == name && u.span.start <= offset && offset < u.span.end)
        .map(|u| u.target_macro.as_str())
    {
        return macro_location(analysis, target);
    }

    None
}

/// A `Location` at `macro_name`'s definition anchor — its `match:` line when
/// locatable, else its name key — in the pack that defines it.
fn macro_location(analysis: &Analysis, macro_name: &str) -> Option<Location> {
    let m = analysis.suite.macros.iter().find(|m| m.name == macro_name)?;
    let anchor = m.match_span.or(m.def_span)?;
    let pack_url = name_to_url(&m.pack)?;
    let pack_raw = analysis.raw.get(&m.pack)?;
    let range = LineIndex::new(pack_raw).span_to_range(anchor);
    Some(Location {
        uri: pack_url,
        range,
    })
}
```

Update the module doc comment at the top of the file (lines 1-10) — it currently states the `match:` and `use:` targets are out of scope. Reword it to describe the current behavior (feature step → macro; `use:` line → target macro; anchored on the `match:` line with a name-key fallback). No task ids.

- [ ] **Step 2: Add the two integration tests**

In `crates/proef-lsp/tests/lsp.rs`, read the existing `definition_on_a_step_jumps_to_the_macro` test and its shared helpers (`FakeDisk`, `open`, `wait_for_any_diagnostics`, `wait_for_response`, `shutdown`, `native_abs`, `name_to_url`). Add two tests reusing them. Use a pack fixture whose lines are known so the cursor positions are exact:

Pack text (`native_abs("suite/packs/p.yaml")`):
```
macros:
  base:
    match: I am the base
    steps:
      - hurl: |
          GET http://x
  wrapper:
    match: the wrapper
    steps:
      - use: base
```
(Line indices 0-based: line 2 = `    match: I am the base`; line 9 = `      - use: base`, where `use` starts at char 8 and `base` ends at char 17.)

```rust
#[test]
fn definition_on_a_use_line_jumps_to_the_target_macro() {
    // ... set up FakeDisk with a feature (any) + the pack above; init; open the PACK doc.
    // Request textDocument/definition at Position { line: 9, character: 13 } (on `use: base`).
    // Assert the response Location.uri is the pack, and its range.start.line == 2
    // (the `base` macro's `match:` line — the anchor).
}

#[test]
fn definition_on_a_step_lands_on_the_match_line() {
    // Feature step binds `wrapper` (match "the wrapper"); pack as above.
    // Request definition on the step; assert Location.range.start.line == 7
    // (wrapper's `match:` line), NOT line 6 (its name key).
}
```

Fill these in concretely by mirroring `definition_on_a_step_jumps_to_the_macro`'s structure exactly (same `run(ServerConfig{..})` on a `Connection::memory()`, same `wait_for_response::<GotoDefinitionResponse>`), substituting the pack fixture, the opened document (the **pack** for test 1), and the assertions above. All paths via `native_abs(...)`; build URIs via `name_to_url`. Use `debounce: Duration::ZERO`.

> The exact line numbers (test 1 → `range.start.line == 2`; test 2 → `range.start.line == 7`) come from the fixture above; if you adjust the fixture text, recompute them. Assert the LINE precisely — that's what distinguishes "lands on match:" (Gap B) from the old name-key behavior.

- [ ] **Step 3: Run to verify fail → implement → pass**

Run: `cargo nextest run -p proef-lsp definition`
Expected: the two new tests FAIL before Step 1's code is in, PASS after. Iterate until green; the pre-existing `definition_on_a_step_jumps_to_the_macro` test may now assert the name-key line — if it does, update its expected line to the `match:` line (the behavior deliberately changed) rather than weakening it.

- [ ] **Step 4: Focused gate + commit**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run -p proef-lsp
git add crates/proef-lsp/src/features/definition.rs crates/proef-lsp/tests/lsp.rs
git commit -m "feat(lsp): go-to-definition from use: lines and onto match: lines"
```

---

## Task 4: Docs, ADR, and public-api

**Files:**
- Modify: `docs/adr/ADR-0017-lsp-language-server.md`
- Modify: `docs/CHANGELOG.md`
- Regenerate: `crates/proef-core/public-api.txt`

**Interfaces:** none (docs + snapshot).

- [ ] **Step 1: Amend ADR-0017**

Add a short dated note to `docs/adr/ADR-0017-lsp-language-server.md` recording that two go-to-definition targets, deferred as v1 scope-cuts, are now implemented: (a) jump from a `use:` reference to its target macro; (b) land on a macro's `match:` line (name-key fallback). Keep it to a few lines under a "Consequences" or a new "Amendments" note — closing the "deviation lived only in a source comment" gap. No task ids.

- [ ] **Step 2: CHANGELOG entry**

Under `## [Unreleased]` in `docs/CHANGELOG.md` (create the section if absent), add an `### Added` bullet: go-to-definition now jumps from a `use:` reference to the target macro and lands on the macro's `match:` line (name-key fallback for use-only macros). Task/plan identifiers may appear here (changelog) but nowhere in code.

- [ ] **Step 3: Regenerate the public-api baseline**

The additions (`analyze::UseRef` + its fields, `SuiteAnalysis.use_refs`, `Macro.match_span`, `MacroRef.match_span`) move the `proef-core` public surface. Run:

```bash
PROEF_PUBLIC_API_UPDATE=1 cargo run -p xtask -- public-api
```

(If the local toolchain cannot run nightly `cargo public-api`, prepend a nightly toolchain to `PATH` as prior release work did, or add the new lines to `crates/proef-core/public-api.txt` by hand matching the exact rendered form of sibling entries.) Then run `cargo run -p xtask -- public-api` (no env) and confirm it reports the surface as current. Review the diff: exactly the four additions above, nothing else.

- [ ] **Step 4: Full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add docs/adr/ADR-0017-lsp-language-server.md docs/CHANGELOG.md crates/proef-core/public-api.txt
git commit -m "docs(lsp): record use:/match: go-to-def; refresh public-api"
```

---

## Self-Review

**1. Spec coverage** (spec `docs/superpowers/specs/2026-08-04-lsp-goto-def-gaps-design.md`):
- §2 decision (analyze-time text-scan locators, no parser change) → Task 1 (locators) + Task 2 (analyze-time build). ✓
- §3 two locators (`use_span` ordinal, `match_span`) degrade to None → Task 1 (impl + tests for None cases). ✓
- §4 Gap A (`UseRef {pack, span, target_macro}`, `SuiteAnalysis.use_refs`, `find_use_target`, pack-position path) → Task 2 (index) + Task 3 (pack path). ✓
- §5 Gap B (`Macro.match_span`, `MacroRef.match_span`, `match_span.or(def_span)` shared by both paths) → Task 2 (fields) + Task 3 (`macro_location` helper). ✓
- §6 robustness (None-degrade, no panic, sans-IO) → locators return Option; `.or(def_span)`; use_refs skips on None. ✓
- §7 testing (locate units, analyze use_refs unit, two LSP integration tests) → Tasks 1/2/3. ✓
- §8 scope (no parser change, sans-IO, public-api +additions, ADR note) → Global Constraints + Task 4. ✓

**2. Placeholder scan:** Task 3 Step 2 gives the test *structure* + exact fixture + exact assertion line numbers, and points at the concrete `definition_on_a_step_jumps_to_the_macro` test to mirror — not a vague "write tests." The `MemProvider`-shape caveat (Task 2 Step 2) instructs adapting to the real helper with identical assertions. No "TBD"/"handle edge cases" remain.

**3. Type consistency:** `use_span(text, macro_name, ordinal) -> Option<Span>` / `match_span(text, macro_name) -> Option<Span>` are defined in Task 1 and called identically in Task 2. `UseRef { pack, span, target_macro }` is defined in Task 2 and read in Task 3 (`u.pack`, `u.span`, `u.target_macro`). `MacroRef.match_span` / `def_span` / `pack` consumed in Task 3's `macro_location` match Task 2's field names. `Macro.match_span` populated in Task 2 Step 1 and copied in Task 2 Step 4. Consistent.
