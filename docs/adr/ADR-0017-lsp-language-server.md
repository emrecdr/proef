# ADR-0017 — `proef lsp` language server

**Status:** Accepted · **Date:** 2026-08-03 (implemented 2026-08-03)
**Design spec:** [docs/superpowers/specs/2026-08-03-proef-lsp-design.md](https://github.com/emrecdr/proef/blob/main/docs/superpowers/specs/2026-08-03-proef-lsp-design.md)

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

- A new crate + two deps, pinned as shipped: **`lsp-server 0.7.9`** and
  **`lsp-types 0.97.0`** (MIT/Apache — clean under `cargo-deny`); the provider trait moves the
  `proef-core` `public-api` snapshot (a deliberate, reviewed change).
- **`lsp-types 0.97` models document URIs as its own `Uri` type (RFC-3986), not `url::Url`.**
  The 0.97 line dropped the `url` dependency, so the converter and every handler key documents on
  `Uri` (parsed/compared as an RFC-3986 string), never `url::Url` — a change from the pre-0.97 API
  that would silently fail to compile against the old assumption. **Superseded — see the
  amendment below.**
- The front-end refactor (injectable provider + collect-all) touches `front.rs` and its
  callers; the CLI path must stay behaviourally identical, guarded by the existing integration
  + snapshot suites.
- Highest bug-risk surface is the byte↔UTF-16 + source-normalization (BOM, trailing newline)
  converter; mitigated by property tests and by reusing `tests/errors/` (real spans/ranges).
- A genuine competitive differentiator, and the natural depth move after the v0.4.0 breadth
  work — but a multi-week (L) effort; sequenced so each step (handshake → provider/collect-all/
  converter → diagnostics → definition → completion → references) is independently testable.

## Amendment — the types crate is `gen-lsp-types`, and `Uri` is `url::Url` again

`lsp-types` stopped receiving releases after 0.97; `gen-lsp-types` is the maintained
successor, generated from the LSP metamodel. proef depends on it under the original
name — `lsp-types = { version = "0.11", package = "gen-lsp-types", features = ["url"] }`
— which is rust-analyzer's own aliasing pattern and leaves every `lsp_types::` path in
the crate untouched. The *decision* above is unchanged: still sync, still the
rust-analyzer family, still server-only.

Two consequences change:

- **`Uri` is `url::Url`.** The generated crate gates its URI type behind features
  (`url`, `fluent-uri`, or a bare `String` newtype). Choosing `url` costs nothing —
  the embedded hurl engine already pulls `url` into the workspace graph — and buys
  back `from_file_path`/`to_file_path`, the native-path bridge the 0.97 `Uri` had no
  equivalent for and that `documents.rs` therefore hand-rolled (drive-letter prefixes,
  segment joining, percent-encoding). That bridge is deleted; the wrapper that remains
  exists only to pin the pipeline's source-name identity rule. The consequence the
  original ADR recorded is retired with it, and the swap **removes** three crates
  (`lsp-types`, `fluent-uri`, `serde_repr`) while adding none.
- **Methods are enums, not string constants.** `Request::METHOD` is now an
  `LspRequestMethod<'static>` whose `From<&str>` falls back to `Custom`, so dispatch
  compares enum values and an unrecognised method lands in a variant rather than
  matching nothing.

One behaviour moved: what counts as a malformed document URI. `fluent-uri` rejected a
raw space; `url` percent-encodes it. The malformed-params test therefore asserts on a
*schemeless* URI, which `url` genuinely rejects — the guarantee under test (a bad URI
is answered with `InvalidParams`, never a dead server) is unchanged.

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

## Amendment — 2026-08-04 (go-to-definition gaps closed)

v1 go-to-definition resolved only feature step → macro, landing on the macro's name key; two
narrower targets were cut and recorded only in a source comment. Both are now implemented: a
`use:` reference inside a pack jumps to the macro it names, and either path lands on the
macro's `match:` line when one is locatable (falling back to the name key for use-only
macros). Both are best-effort text-scan locators in `proef-core::pack::locate`, indexed at
analyze time, following the existing sans-IO/text-scan idiom already used there — no parser
change.
