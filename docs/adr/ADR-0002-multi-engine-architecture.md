# ADR-0002 — Multi-engine core: factory/session seam, step-kind routing, batching

**Status:** Accepted · **Date:** 2026-07-28

## Context

Requirement: all tests are Gherkin; the parser dispatches to pluggable engines — API
(hurl) now; browser and Android engines later — a factory/session seam
(`driver-web`/`driver-adb` behind one trait, `web:`-prefixed steps → browser, bare →
tablet, one shared variable scope). Balanced-architecture stance: deliberate seams for
future engines, no gold-plating. Ecosystem survey: probe-rs's `ProbeFactory`/`DebugProbe`
split is the closest production analog; sqlx registers compiled-in drivers explicitly;
dispatch cost at batch granularity is noise, so `dyn` vs enum is decided on coupling, not
performance (`enum_dispatch` would couple core to every engine crate — wrong direction).

## Decision

Two traits in `proef-core`; engines implement both; the CLI assembles the registry.

```rust
pub trait EngineFactory: Send + Sync {
    fn id(&self) -> &'static str;
    fn step_kinds(&self) -> &'static [StepKindSpec];  // pack namespace + schema fragment
    fn doctor(&self) -> Vec<DoctorCheck>;
    fn open(&self, ctx: &ScenarioCtx) -> Result<Box<dyn EngineSession>, EngineError>;
}
pub trait EngineSession: Send {
    fn run_batch(&mut self, batch: &StepBatch, world: &mut World,
                 events: &EventSink, cancel: &CancellationToken) -> BatchResult;
    fn finish(&mut self) -> Result<(), EngineError>;
}
```

Routing: a macro step's kind names its engine (`http:` → engine-hurl; `web:`/`tablet:`
reserved). A lowered scenario is an ordered heterogeneous step list; the core dispatches
**contiguous same-engine batches** in order. The World is the interop bus between
batches and engines. Sessions are per-scenario, opened lazily, torn down in `finish`
(+ `Drop` backstop); engines may hold sessions concurrently within a scenario.
Registry: `Vec<Box<dyn EngineFactory>>` in `proef-cli`, engines optionally behind cargo
features (`engines-web`). Lifecycle is enforced by ownership shape (only a session runs
batches), not typestate generics (which would break `dyn`).

## Consequences

Adding an engine = one crate + one registry line; pack schema and `doctor` extend via
`step_kinds()`/`doctor()` without core edits — **the acceptance test: engine-web lands
with zero `proef-core` diff**. Core stays free of engine-specific types. Costs accepted:
`Box<dyn>` indirection (irrelevant at batch granularity); two traits instead of one
(justified: lifecycle safety + capability discovery). Engines own their artifacts
(hurl files / screenshots / HAR).

## Alternatives considered

Single `Engine` trait with runtime lifecycle state (v3 draft) — weaker lifecycle
guarantees; enum dispatch — inverts the dependency direction; dynamic loading (dlopen/
WASM) — rejected as over-architecture, compiled-in covers every stated future; typestate
generics — fights `dyn`, ownership shape gives most of the safety.

## Errata

**2026-07-28 (M1/M5):** The routing example above names the API step kind
`http:`; ADR-0004's examples and TECH-SPEC §6's normative pack schema use
`hurl:` (the raw-block key doubles as the routing kind). The implementation
follows the tech spec: the step kind **and** the engine id are `hurl`, so
"a step's kind names its engine" holds verbatim. Read `http:` in the Decision
above as `hurl:`. `web:`/`tablet:` remain reserved as written.
