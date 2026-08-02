# Design — `proef lsp` language server

**Status:** approved design (pre-implementation) · **Date:** 2026-08-03 · **Owner:** Emre
**Decision record:** [ADR-0017](../../adr/ADR-0017-lsp-language-server.md) ·
**Roadmap:** IMPROVEMENT-PLAN §5 item #11

## 1. Goal & motivation

Give feature/pack authors editor-native support — live diagnostics, go-to-definition, and
completion — for `.feature` prose and YAML macro packs. Weak IDE support is a documented gap
in Karate and the BDD field (IMPROVEMENT-PLAN §10); proef is unusually well-positioned because
its analysis is already **headless and sans-IO**: `front::run` produces the same `Diag`
objects (stable `code`, byte `span`, `severity`, `help`) whether driven by the CLI or an
editor. The LSP is therefore a **second front-end over existing analysis**, not new analysis —
which is what makes a bounded implementation possible.

## 2. Scope

**v1 (this design) — the full LSP:** diagnostics, go-to-definition, completion, and (low-cost)
find-references, delivered as a **server-only** generic-LSP binary that works today with
Neovim / Helix / Emacs / Sublime via a few lines of LSP config.

**Non-goals for v1 (deliberate):**

- **No VS Code extension** — a thin TypeScript/npm/vsce wrapper is off proef's pure-Rust brand
  and a separate release surface; it can follow once the server proves out.
- **No incremental (salsa-style) index** — see §5; the suite scale makes wholesale recompute
  cheap, so the incremental-computation machinery mature LSPs carry is YAGNI until a real perf
  ceiling appears.
- No rename, code actions, formatting-via-LSP (`proef fmt` already exists), or semantic tokens.

## 3. Architecture

A new **`proef-lsp` crate** (bin surfaced as the `proef lsp` subcommand from `proef-cli`) on
the sync **`lsp-server`** + **`lsp-types`** crates (the rust-analyzer-family building blocks —
no tokio, satisfying ADR-0006's async ban). Modules:

- **`server`** — the stdio LSP event loop: JSON-RPC read/dispatch, holds the `Connection`,
  owns the initialize/initialized/shutdown handshake. Single-threaded main loop (analysis is
  fast enough to need no worker pool in v1; the escape hatch is a thread + the core's existing
  `CancellationToken`).
- **`documents`** — the in-memory overlay: open-doc text keyed by `Url`, plus the file reader
  that prefers the overlay and falls back to disk (cached). **The only IO lives here**, at the
  edge — `proef-core` stays sans-IO.
- **`analysis`** — the recompute step: given the workspace root + overlay, drive the existing
  front-end to produce a `SuiteAnalysis`. Wholesale, debounced.
- **`convert`** — the byte-span ↔ LSP-position bridge (§6.3), reused by every feature.
- **`features`** — three thin handlers (`diagnostics`, `definition`, `completion`) plus
  `references`, each a read over `SuiteAnalysis`.

## 4. Data flow & the recompute model

One structure — `SuiteAnalysis` — is the product of a wholesale recompute and the single
source every feature reads:

- `diagnostics: Map<Url, Vec<Diag>>` — per-file errors + warnings (features *and* packs).
- `bindings: Vec<{ step_span, feature_url, macro_name }>` — every bound prose step → the macro
  it resolved to. This one relation powers **both** go-to-def (step→macro) and find-references
  (macro→steps).
- `macros: Vec<{ name, pattern, params, def_url, def_span }>` — the macro vocabulary, for
  completion and as go-to-def targets.

**Loop:** `didOpen`/`didChange`/`didSave`/watched-file change → mark dirty → **debounce
(~200 ms)** → recompute (front-end over the suite, open buffers from the overlay) →
`SuiteAnalysis` → `publishDiagnostics` for every file (**including clearing** now-clean files)
→ cache. `definition`/`completion`/`references` requests read the cached `SuiteAnalysis`
synchronously.

## 5. Workspace model — whole-suite, wholesale recompute (the validated choice)

Mature LSPs (rust-analyzer, gopls, clangd, TS) model the **whole project**, because their
highest-value features are inherently cross-file; rust-analyzer carries **salsa** incremental
computation precisely so a whole-workspace model stays responsive at *large* scale. proef's
"workspace" is a **test suite** — tens of small files — so the whole `front::run` pipeline is
milliseconds and sans-IO. The scale therefore **collapses the per-document-vs-index tradeoff**:
proef gets the *capabilities* of the full-workspace model (cross-file re-validation — editing a
pack live-refreshes dependent features — and find-references) at nearly per-document
*simplicity*, by modelling the whole suite but **recomputing it wholesale on change** instead
of building incremental invalidation. Incremental indexing is YAGNI until a suite reaches the
hundreds-of-features range, and can be added later behind the same interface.

## 6. The real work: three front-end affordances (not the protocol)

`lsp-server` handles the wire protocol; the actual effort is making the **existing analysis
reusable**:

### 6.1 Injectable source provider
Refactor the front-end so file *discovery* + *reading* go through a provider trait — disk for
the CLI, overlay-then-disk for the LSP (so unsaved buffers analyze). This **preserves core
sans-IO**: the IO moves into the injected provider, the ADR-0012 pattern. It is the main
refactor. The sans-IO orchestration (bind/lower/resolve-Probe over discovered units) can live
in `proef-core` taking the provider; the disk provider lives in `proef-cli`, the overlay
provider in `proef-lsp`.

### 6.2 Collect-all mode
Today `front::run` is fail-fast (`Result<FrontEnd, FrontError>`). The LSP needs *every*
diagnostic at once: accumulate per-unit errors and continue rather than early-returning. A
parse-failed file reports its parse diagnostic and is **skipped downstream — no cascade** of
bogus binding errors (how good LSPs degrade). The per-unit analysis is unchanged; only the
orchestration stops early-returning. Additive — the CLI keeps its fail-fast path.

### 6.3 byte↔UTF-16 + normalization converter
The honest edge case: the front-end **normalizes** source (strips a UTF-8 BOM, appends a
trailing newline when missing) and its spans index the *normalized* text, while the editor
holds *raw* text and speaks (line, **UTF-16** code-unit column). The converter must bridge
**both** transforms — a line-start byte index for line/col, char-boundary walking for UTF-16,
and the normalization offset (BOM strip shifts every span). Property-tested for round-trip,
including non-ASCII and BOM/no-trailing-newline inputs.

## 7. Feature handlers (all thin reads over `SuiteAnalysis`)

- **Diagnostics** — push model: each `Diag` → LSP `Diagnostic` (`code`→`code`,
  `severity`→`severity`, byte `span`→`range` via §6.3, `help`→related-information). Publish
  per-file; clear stale files.
- **Go-to-definition** — position → byte offset → the `binding` whose step-span contains it →
  `Location` of that macro's `match:` in its pack; also a `use:` reference → its target macro.
- **Completion** — in a feature step → the macro `match:` patterns offered as snippets
  (`{capture}` → tabstops), ranked by the existing `closest` / `literal_skeleton` logic (the
  same "did you mean" substrate that powers unbound-step suggestions). The headline authoring
  win.
- **References** — macro `match:`/name → all `bindings` that resolved to it. Falls out of the
  index at near-zero extra cost.

## 8. Error handling / robustness

Parse-failed file → its diagnostic, no downstream cascade. A file outside any suite (no
discoverable packs) → analyze what's possible; unbound steps report as they would on the CLI.
A new edit mid-debounce cancels the pending recompute. The recompute runs under `catch_unwind`
(as the runner wraps scenarios) so a panic in analysis never kills the server. Malformed
`initialize` params or an unexpected file scheme → a clean LSP error, never a crash.

## 9. Testing strategy

- **Reuse `tests/errors/` as LSP fixtures** — each seeded broken case already pins a diagnostic
  `code`; an LSP test asserts it surfaces as an LSP `Diagnostic` with that code + the right
  range. High-value reuse — the corpus already guarantees the diagnostics; the test confirms
  the LSP mapping.
- **Property-test the converter** (§6.3) — round-trip byte↔(line,UTF-16), including non-ASCII,
  BOM, and missing-trailing-newline.
- **Unit-test collect-all** — N independent errors across files are all accumulated (not
  fail-fast), and a parse failure suppresses that file's downstream diagnostics.
- **Scripted JSON-RPC integration** — drive the server over stdin/stdout: open → diagnostics;
  definition on a step → the macro `Location`; completion in a step → expected patterns;
  edit a pack → dependent feature diagnostics refresh. Responses snapshot-locked (insta).
- Everything sans-IO/deterministic except the in-memory overlay.

## 10. Build sequencing

MVP is full-scope, but the *build* sequences, each step independently testable:

1. `proef-lsp` crate scaffold + stdio handshake (initialize/initialized/shutdown) + overlay.
2. **Injectable provider + collect-all + converter** (§6 — the hard core, most of the risk).
3. Diagnostics (first visible value).
4. Go-to-definition.
5. Completion.
6. Find-references.
7. `proef lsp` subcommand wiring; editor-setup docs (Neovim/Helix/Emacs); ADR-0017 →
   Accepted; CHANGELOG; `public-api` snapshot for any new `proef-core` surface (the provider
   trait); `docs-check`.

## 11. Dependencies & gates

New deps: `lsp-server` + `lsp-types` (rust-analyzer-family, MIT/Apache — expected clean for
`cargo-deny`; confirm at scaffold). All existing gates apply. The provider-trait addition to
`proef-core` moves the `public-api` snapshot — a deliberate, reviewed change.

## 12. Risks / open questions

- **Provider refactor blast radius** — extracting front-end orchestration to take a provider
  touches `front.rs` and its `proef-cli` callers; must leave the CLI path behaviourally
  identical (the existing integration + snapshot suites are the guard).
- **Normalization/UTF-16 correctness** — the highest-bug-risk area; mitigated by the converter
  property tests and by reusing the corpus (real spans, real ranges).
- **Multi-suite workspaces** — a workspace root may contain more than one suite; v1 discovers
  suites under the root and analyzes each (the CLI's discovery already handles a path → its
  features + packs). Confirm the discovery boundary during step 2.
