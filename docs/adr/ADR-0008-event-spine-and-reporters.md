# ADR-0008 — Serde event spine, decorator reporters, libtest-mimic harness

**Status:** Accepted · **Date:** 2026-07-28

## Context

a live-event seam (`EventSink(Arc<dyn Fn(RunEvent)>)`, domain events decoupled
from wire format) is proven; its run *record* is a separate reporter path. The two best
in-domain designs both converge on "typed event stream + composable consumers":
cargo-nextest (runner emits structured events; reporter fans out to human/JUnit/machine
outputs) and cucumber-rs (decorator Writer stack: `Normalize` → `Summarize` → leaves,
with `Tee`, marker traits for ordering guarantees). Verified: nextest officially
supports libtest-mimic custom harnesses (documented CLI contract; libtest-mimic 0.8.x);
libtest's JSON format itself is still unstable nightly territory; OTel test semconv is
"development" maturity.

## Decision

One **serde-able event enum** in `proef-core` is the spine: `RunStarted`,
`ScenarioStarted`, `BatchStarted`, `StepFinished` (engine id, StepRef
feature/line, status, attempts, duration, capture *names*), `ScenarioFinished`,
`RunFinished`. The JSONL run record **is** the appended event stream — `explain`,
history, and any future UI replay from disk; no second record format. Reporters are
event consumers composed decorator-style: `Normalize` (repairs interleaving from
parallel scenarios) → `Summarize` → leaves: console BDD tree, JUnit XML (via
`quick-junit`), GitHub job summary, JSONL appender. Secret values never enter events
(capture names only — redaction invariant, ADR-0005). **M5:** a `libtest-mimic` harness
binary exposes one `Trial` per scenario, making `cargo nextest run` and IDE test UIs
drive proef with zero custom protocol work; libtest JSON remains an output adapter,
never the native schema. OTel export: deferred; if added, a thin optional reporter
mapping to `test.*` semconv names.

## Consequences

Single source of truth for live progress *and* persistence; reporters are ~a page each;
new outputs are additive leaves. Replayability makes run records diffable and testable
(insta snapshots over event streams). Cost: event schema becomes a compatibility
surface — versioned with a `schema` field from day one.

## Alternatives considered

a split design (live events + separate JSON record) — two sources of truth to keep
consistent. `tracing` as the event bus — wrong tool: tracing is operator telemetry, not
a typed result stream (kept for diagnostics). Cucumber's writers verbatim — async trait
+ World coupling we don't need; the decorator *shape* is what's adopted.

## Errata

**2026-07-29:** the variant set has grown additively since acceptance:
`EntryRunning` (live per-attempt engine progress) joined the six original
variants, `RunFinished` gained a `cancelled` flag, and `StepFinished` gained a
`detail` failure field — all serialized only when present, so pre-existing
streams parse unchanged. The additive-only rule held; this note keeps the
variant inventory honest.
