# Architecture — proef

> Grounded in `CLAUDE.md` (Workspace architecture) and `docs/` (ADR-0001 onward, TECH-SPEC).
> Those sources WIN on any conflict. Read by the `architect` agent and arch-scanner.

proef is a declarative, multi-engine end-to-end **API** test runner. Gherkin `.feature`
files carry business prose; YAML macro packs (with embedded raw hurl blocks) bind prose to
steps; an engine-agnostic core lowers and dispatches batches to a pluggable engine. The
only engine embeds hurl in-process. The seam is architectural readiness — nothing else is
scheduled (see `[[hurl-engine-only]]`).

## The pipeline (all in `proef-core`, pure)

```
.feature + packs → parse (gherkin) → bind (matcher) → lower (${…} resolve, batch)
                 → emit (.hurl + sidecars) → dispatch (Box<dyn EngineSession>) → events
```

Core is pure: no IO, no clock, no env, no randomness — `run_id`, `now`, and env snapshots
are injected. This determinism is a hard invariant, not a convenience (ADR-0005, TECH-SPEC
§4). Do not break it.

## Crates

```
crates/
  proef-core/         engine-agnostic: parse, packs, bind, lower, IR, emit, dispatch,
    helpers/          World/state, events, errors, reporters; built-in packs embedded
  proef-engine-hurl/  the API engine: EngineFactory/EngineSession over embedded hurl
  proef-cli/          bin `proef`: clap, engine registry assembly, miette rendering
  proef-fixture/      dev-only in-process sync fixture API server (tiny_http, ADR-0011)
  proef-harness/      libtest-mimic bridge: one Trial per scenario (US-12)
xtask/                automation as Rust; `just` = thin aliases (no shell scripts)
```

## Dependency direction (hard invariant)

```
proef-cli → proef-core, proef-engine-hurl      (cli is the only miette user — ADR-0009)
proef-engine-hurl → proef-core
proef-core → (no engine, no engine-specific type)
engines never import each other
```

- `proef-core` importing an engine or engine-specific type — CRITICAL violation.
- miette used outside `proef-cli` — CRITICAL (ADR-0009: typed errors in core/engines,
  miette only at the CLI edge).
- IO / clock / env / rand inside `proef-core` — CRITICAL (breaks determinism).
- Cargo refuses inter-crate cycles; the same discipline applies intra-crate.

**Structural acceptance test:** adding a future engine must leave `proef-core` diff-empty
(`git diff --stat proef-core` empty — ADR-0002, IMPLEMENTATION-PLAN M6).

## The central seam (ADR-0002, ADR-0006)

`EngineFactory` (`id`, `step_kinds()` schema contribution, `doctor()`, `open`) +
`EngineSession` (`run_batch`, `finish`) — both **sync**, used as `Box<dyn …>`. No async
machinery in v1 (no async-trait/maybe-async, no tokio runtime). Routing: a macro step's
kind names its engine (`hurl:` → engine-hurl); other kind prefixes are reserved. The core
batches **contiguous same-engine steps** and dispatches in order; the **World** (typed vars
+ persistent global store) threads captures between batches. The engine registry lives in
`proef-cli`, one cargo-feature-gated line per engine.

## Engine-hurl seam facts (TECH-SPEC §5 — verified, file:line there)

- `run_entries` builds its HTTP client **per call** → batch maximally; split only at
  `optional:` boundaries and engine changes.
- On forced splits, chain `HurlResult.variables` and round-trip cookies via a Netscape
  temp file (`SessionState`).
- hurl has no cancellation and allows infinite retries → finite-retry pack lint + batch
  budgets + watchdog; token checked between batches only (ADR-0007).
- Per-entry `[Options]` override batch `RunnerOptions` (clone-then-override).
- Always `WriteMode::Buffered` in library paths.

## Contracts that shape structure

- **Artifacts = executed input** (ADR-0010): emitted `.hurl` bytes == bytes parsed;
  canonical format snapshot-locked.
- **Events** (ADR-0008): one versioned serde `Event` spine; the JSONL stream is the run
  record; additive-only.
- **Scenario** is the unit of isolation, parallelism, retry, and artifact emission
  (scenario-per-OS-thread, `--jobs` bounded).

## Newtypes carry meaning

Domain identity lives in newtypes (`StepKindId`, `EngineId`, `StepRef`, `Value`), not bare
primitives. `Value` uses hand-rolled scalar visitors — never a `#[serde(untagged)]` enum
carrying numbers (arbitrary_precision hazard, `[[hurl-engine-only]]` build note in
`golden-rules.md` §8).
