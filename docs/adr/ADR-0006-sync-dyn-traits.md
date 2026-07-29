# ADR-0006 — Engine traits are sync + dyn; no async machinery in v1

**Status:** Accepted · **Date:** 2026-07-28

## Context

All planned engines are blocking at the edge: hurl is synchronous libcurl; adb_client is
blocking; a CDP engine can be driven blocking or
async. Verified ecosystem facts (mid-2026, stable 1.97): `async fn` in traits is stable
for static dispatch but **still not dyn-compatible** (AFIDT is nightly-only, no
timeline; RTN unstable); `maybe-async`'s sync/async toggle is a non-additive cargo
feature with documented ecosystem breakage. Calls across the seam are coarse
(batch-level), so async buys no throughput inside the runner itself.

## Decision

`EngineFactory` and `EngineSession` are synchronous traits, used as `Box<dyn …>`.
Parallelism is scenario-per-OS-thread. If a future host (server mode, desktop UI) is
async, it wraps engine calls in `spawn_blocking` at *its* edge; the core never learns
about executors (reinforced by the sans-IO-lite rule: core does no IO at all).
`maybe-async` is explicitly banned. Traits meant to be implemented externally
(`Engine*`) are not sealed; internal traits that must stay evolvable are sealed.

## Consequences

Simple, dyn-compatible seam today; no async runtime in the dependency tree
(`tokio-util`'s `CancellationToken` is runtime-independent — ADR-0007); a future async
migration is additive (an `AsyncEngineSession` adapter or AFIDT adoption when stable),
not a rewrite. Cost: a long-running async host pays one thread per in-flight scenario —
acceptable at e2e-suite scale.

## Alternatives considered

Async-first trait via `async-trait` — boxes every call, forces an executor decision on
every consumer, and models nothing real while engines block. `maybe-async` dual API —
non-additive feature hazard. Callback/actor-per-engine threading model — more machinery
than batch dispatch needs; revisit only if an engine genuinely multiplexes (e.g. CDP
event streams), and then *inside* that engine crate, invisible to the seam.
