# ADR-0001 — Embed hurl's crates in-process as the API engine

**Status:** Accepted · **Date:** 2026-07-28

## Context

The API engine must execute HTTP tests with Hurl semantics. Constraint history: the
original "entirely in Rust" rule (which forbade C-linked deps and favored a pure-Rust
reimplementation) was relaxed to **"mostly Rust — C system libraries acceptable within
margin"**, and the product intent was fixed as *"a wrapper / Gherkin adapter over hurl,
riding upstream hurl as it improves"*. Verified facts: hurl's `runner::run_entries` is
the seam hurl's own parallel workers use (buffered stdio, per-entry results with spans,
captures, libcurl timings); hurl links libcurl + OpenSSL + libxml2 and needs libclang at
build; `hurl_core` alone also links libxml2; the crates break API in minor releases
(issue #3846); the format itself is spec'd and stable. A working spike cross-ran
generated artifacts under a prototype pure-Rust runner and the stock hurl CLI: the first
cross-check caught a real semantic divergence (`contains` = element equality on
collections, not substring) — demonstrating the standing cost of reimplementation.

## Decision

`proef-engine-hurl` embeds the hurl crates, pinned exactly: generate `.hurl` text from
the IR, `hurl_core::parser::parse_hurl_file`, then `hurl::runner::run_entries` with
`WriteMode::Buffered` terms, an `EventListener` for progress, and `VariableSet` in/out.
No pure-Rust HTTP reimplementation ships; no hurl subprocess is required at runtime.

## Consequences

Positive: hurl semantics by construction (zero drift class); full assert/filter/XPath/
cookie/redirect/HTTP-2 surface available to packs on day one; rich in-process results
with source spans mapped back to `.feature` lines; ~a third less engine code than the
reimplementation plan. Negative: build prereqs on every machine (`libcurl-dev
libxml2-dev libclang pkg-config`; macOS ships the libs) — mitigated by docs + `proef
doctor`; dynamically-linked release binaries (no musl static); crate API instability —
mitigated by ADR-0003; `run_entries` is `#[doc(hidden)]` (semi-blessed seam) — covered
by a pinned integration test.

## Alternatives considered

**Pure-Rust native engine + hurl-CLI differential oracle** (the original recommendation
under the zero-C rule): zero C deps and static binaries, but permanent semantic-drift
liability and a differential harness to maintain; recorded in `research/` as the path
back if static distribution ever becomes a requirement. **Transpile + hurl CLI
subprocess:** full fidelity, least code, but an external binary dependency, no in-process
events, and clunky cross-scenario variable threading; its remnant lives on as the
upgrade-canary and `proef artifacts` hand-off. **Divergent fork of hurl:** rejected —
upstream keeps the format low-level by philosophy (issue #2090) and a divergent fork
defeats riding upstream improvements (see ADR-0003 for the thin-fork nuance).
