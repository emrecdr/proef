# ADR-0017 — `proef lsp` language server

**Status:** Proposed (design approved, pre-implementation) · **Date:** 2026-08-03
**Design spec:** [docs/superpowers/specs/2026-08-03-proef-lsp-design.md](../superpowers/specs/2026-08-03-proef-lsp-design.md)

## Context

Feature/pack authoring has no editor support — no live diagnostics, no jump-to-macro, no
step completion. Weak IDE support is a documented gap in Karate and the BDD field
(IMPROVEMENT-PLAN §10), and it is the one substantive unbuilt item on the Round-1 roadmap
(§5 item #11, graded ✅ FITS). proef is unusually well-placed for it: its analysis is already
**headless and sans-IO** — `front::run` yields the same `Diag` objects (stable `code`, byte
`span`, `severity`, `help`) regardless of driver — so an LSP is a *second front-end over
existing analysis*, not new analysis.

## Decision

Build a **`proef-lsp` crate** (surfaced as the `proef lsp` subcommand) as a **server-only,
generic-LSP stdio binary** delivering the **full v1 feature set** — diagnostics,
go-to-definition, completion, and find-references. Key choices:

- **Sync `lsp-server` + `lsp-types`** (rust-analyzer-family), not an async stack — honouring
  ADR-0006's tokio ban.
- **Whole-suite model, recomputed wholesale on change** (debounced), *not* an incremental
  (salsa-style) index. Mature LSPs index the whole project because cross-file features are
  core; they carry incremental machinery only because their scale is large. proef's suite is
  tens of small files and the pipeline is milliseconds, so wholesale recompute buys the
  cross-file capabilities (live cross-file re-validation, find-references) at per-document
  simplicity. Incremental is YAGNI until a real perf ceiling.
- **Second front-end over sans-IO core.** The one enabling refactor: an **injectable source
  provider** so file discovery + reading go through a trait (disk for the CLI, overlay-then-
  disk for the LSP), and a **collect-all** mode (accumulate every diagnostic instead of
  fail-fast). Both keep `proef-core` sans-IO — the IO is injected, the ADR-0012 pattern.
- **Server-only v1**; a VS Code extension is deferred (off proef's pure-Rust brand; a thin
  wrapper can follow).

## Consequences

- A new crate + two deps (`lsp-server`, `lsp-types`, MIT/Apache — confirm clean under
  `cargo-deny` at scaffold); the provider trait moves the `proef-core` `public-api` snapshot
  (a deliberate, reviewed change).
- The front-end refactor (injectable provider + collect-all) touches `front.rs` and its
  callers; the CLI path must stay behaviourally identical, guarded by the existing integration
  + snapshot suites.
- Highest bug-risk surface is the byte↔UTF-16 + source-normalization (BOM, trailing newline)
  converter; mitigated by property tests and by reusing `tests/errors/` (real spans/ranges).
- A genuine competitive differentiator, and the natural depth move after the v0.4.0 breadth
  work — but a multi-week (L) effort; sequenced so each step (handshake → provider/collect-all/
  converter → diagnostics → definition → completion → references) is independently testable.

## Alternatives considered

- **Incremental (salsa) index** — rejected for v1: it is the complexity mature LSPs accept for
  *large* scale, which a test suite does not have. Buys nothing at proef's scale; can be added
  later behind the same interface.
- **Per-document analysis (no cross-file model)** — rejected: it would leave stale diagnostics
  when a pack changes and cannot do find-references; the wholesale-recompute model gives the
  cross-file behaviour without the index cost.
- **Async LSP stack (tokio + tower-lsp)** — rejected: violates ADR-0006; `lsp-server` is the
  sync, rust-analyzer-proven alternative.
- **Ship a VS Code extension in v1** — deferred: a TypeScript/npm deliverable off the pure-Rust
  brand and a separate release surface; generic-LSP config covers Neovim/Helix/Emacs/Sublime
  today, and the extension is a thin follow-up.
- **Diagnostics-only or diagnostics+go-to-def MVP** — considered; the operator chose the full
  feature set for v1 (the complete authoring experience), sequenced internally so value still
  lands incrementally.
