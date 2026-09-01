# ADR-0002 — Multi-engine core: factory/session seam, step-kind routing, batching

**Status:** Accepted · **Date:** 2026-07-28 (amended 2026-09-01 — the core's entry
grammar is a named closed set; see the Amendment below)

## Context

Requirement: all tests are Gherkin; the parser dispatches to pluggable engines — API
(hurl) now, a future non-hurl engine possible later behind the same seam — a
factory/session seam (multiple engine implementations behind one trait, a step's
kind-prefix routes it to its engine, one shared variable scope). Balanced-architecture
stance: deliberate seams for a future engine, no gold-plating. Ecosystem survey: probe-rs's `ProbeFactory`/`DebugProbe`
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

Routing: a macro step's kind names its engine (`http:` → engine-hurl; other kind prefixes
reserved for a future non-hurl engine). A lowered scenario is an ordered heterogeneous step list; the core dispatches
**contiguous same-engine batches** in order. The World is the interop bus between
batches and engines. Sessions are per-scenario, opened lazily, torn down in `finish`
(+ `Drop` backstop); engines may hold sessions concurrently within a scenario.
Registry: `Vec<Box<dyn EngineFactory>>` in `proef-cli`, engines optionally behind cargo
features (one feature per engine). Lifecycle is enforced by ownership shape (only a session runs
batches), not typestate generics (which would break `dyn`).

## Consequences

Adding an engine = one crate + one registry line; pack schema and `doctor` extend via
`step_kinds()`/`doctor()` without core edits — **the acceptance test: a future non-hurl
engine lands with zero `proef-core` diff**. Core stays free of engine-specific types. Costs accepted:
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
above as `hurl:`. Other kind prefixes remain reserved for a future non-hurl engine as written.

## Amendment — the core's entry grammar is a named closed set

**2026-09-01 · Accepted.** "Core stays free of engine-specific types" is true and
stays true. "Core stays free of engine-specific *syntax*" was never true, and the
worklist carried the gap for two rounds without resolving it. This amendment
states the real boundary and makes it enforceable.

### Why the core knows any hurl at all

The core performs **text surgery on entries**: `bake_entry_options` splices an
`[Options]` block into each entry after its header block, and an `expect:` macro
merges asserts into the *previous* request entry (ADR-0004). Both operations have
to find an entry boundary in text the engine will later parse. That is structural,
not incidental — the surgery is what the pack format is built on — so a boundary
recogniser has to live somewhere, and pushing it behind the seam would move the
literals without making the algorithm engine-independent.

### The measurement

Nineteen production lines, across three files (`lower.rs`, `emit.rs`,
`pack/validate.rs`) — **not** the "~290 lines, all in `lower.rs`" the worklist
recorded. That figure counted `#[cfg(test)]` fixtures, where a core test
exercising the pipeline necessarily writes *some* engine's payload. The vocabulary
is thirteen distinct literals in three groups:

| Group | Tokens | Where |
|---|---|---|
| **written** — the core generates this hurl | `[Options]`, `[Asserts]`, `HTTP *`, `variable:`, `retry:`, `retry-interval:`, `delay:` | `lower.rs` |
| **recognised** — read to find an entry boundary | ` ``` ` (body fence), `HTTP` / `HTTP ` / `HTTP/` | `lower.rs`, `emit.rs`, `pack/validate.rs` |
| **not hurl** — proef's own pack syntax | `secret:`, `use:` | `lower.rs`, `pack/validate.rs` |

The four boundary recognisers (`is_method_line`, `is_section_header`,
`is_response_line`, `is_header_line`) are already one canonical `pub(crate)` set
shared by all three files. That half is done.

### The asymmetry this exposes

`StepKindSpec::options` exists, in its own words, as "the seam that keeps option
*spellings* out of `proef-core`" — added because matching `"retry-interval:"` as a
literal meant "one rule lived at two altitudes." It covers **recognising** options.
The core still **writes** `retry:`, `retry-interval:`, `delay:` and `variable:` as
literals, so the same rule still lives at two altitudes, in the other direction.

### Decision

1. The set above is the **sanctioned** core entry grammar. It is closed: a token
   outside it, or an existing token appearing in another core module, is a
   defect against this ADR.
2. It is pinned by `crates/proef-cli/tests/source_guards.rs`
   (`hurl_grammar_in_core_is_the_closed_set_the_adr_names`), which fails on both
   growth and relocation and sends the author back here. A claim of this shape
   decays the moment it is only prose — this one already had, by an order of
   magnitude and in the direction that made it look worse than it is.
3. Migrating the **written** group behind the seam (an emitter beside
   `StepKindSpec::options`) is **deferred, not rejected**. It buys nothing today:
   hurl is the only engine and no other is scheduled, so the migration would add
   a fn pointer, a trait obligation and a public-API break to relocate seven
   literals that exactly one implementation will ever supply. **Trigger:** a
   second engine being scheduled. That is also when ADR-0002's acceptance test
   — a new engine lands with zero `proef-core` diff — first has anything to say
   about them; until then it is unfalsifiable here either way.

### Consequences

The acceptance test is narrowed on the record: a second engine lands with zero
`proef-core` diff **except** the written group, which is a known, enumerated,
guarded debt with a named trigger rather than an open question. Anyone reaching
for new hurl syntax in the core hits a failing test that names both remedies.
