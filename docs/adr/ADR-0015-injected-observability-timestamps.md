# ADR-0015 — Injected observability timestamps (run-level timeline)

**Status:** Accepted · **Date:** 2026-08-02

## Context

The HTML report has a per-scenario timing *waterfall* (IMPROVEMENT-PLAN §12, N6a) derived
purely from each step's `duration_ms`. It cannot show **cross-worker occupancy** — which
scenarios ran concurrently, on which of the `--jobs` workers — because the sans-IO core
reads no clock and the event stream carries no wall-clock timestamp or worker identity.

The core's purity is deliberate (deterministic snapshots/properties). ADR-0012 established
the escape valve: values the core must not compute (config) are **injected at the CLI
edge**. `run_id` is already such a value — the core *carries* it (`RunStarted.run_id`) but
the CLI *generates* it. The same pattern extends to timing.

## Decision

- Add two **optional, additive** fields to the `ScenarioStarted` and `ScenarioFinished`
  events: `timestamp_ms: Option<u64>` (milliseconds since the run began) and
  `worker: Option<u64>` (0-based worker index). Both are
  `#[serde(default, skip_serializing_if = "Option::is_none")]`, so old records parse
  unchanged and single-threaded/None cases serialize identically (ADR-0008 additive-only).
  They are kept **off `RunStarted`**, whose exact wire bytes are pinned.
- **The core leaves them `None`** — it carries fields it never fills, exactly as it carries
  a `run_id` it never generates (sans-IO preserved). The CLI **wraps the event sink** in a
  stamping closure: `EventSink::new(move |ev| inner.emit(&stamp(ev)))`. `stamp` runs on the
  **worker thread** (emit is synchronous, called from the scenario worker), so it reads the
  run-start `Instant` and maps `thread::current().id()` → a stable 0-based index there, at
  the edge. The core never sees a clock or a thread id.
- **Errata (2026-08-11): only `ScenarioStarted` carries a `worker`.** As implemented,
  `ScenarioFinished` is emitted from the main dispatcher thread, not the worker that ran the
  scenario, so stamping a thread index there would name the wrong one; it carries the end
  timestamp and `worker: None`. The worker identity comes from `ScenarioStarted`, which *is*
  emitted on the worker thread, and the timeline pairs the two. `EVENTS.md` has always
  described it this way — the bullet above did not.
- The HTML report gains a **run-level timeline**: a lane per worker, each scenario a bar from
  its start to its finish timestamp — the Gantt/occupancy view. It renders only when the
  stamps are present; a record without them falls back to the N6a per-scenario waterfall
  alone. The view stays a pure function of the (now richer) event record.

## Consequences

- Core stays sans-IO; the injection is the established `run_id`/config pattern.
- The JSONL record gains observability fields, additively (ADR-0008 — still the one record;
  no sidecar timing file).
- **Snapshot honesty**: old records parse unchanged, *but* every event in a *new* run now
  carries a (non-deterministic) `timestamp_ms`, so the `reference_event_stream` snapshot
  changes — it needs a new insta filter (`"timestamp_ms":\d+` → `0`) plus a deliberate
  `cargo insta review`. This is allowed (the same deliberate-acceptance policy as emitter
  changes), and is called out rather than glossed. The HTML snapshot likewise regenerates.
- Enables the cross-worker timeline; the derived HTML view remains pure over the record.

## Alternatives considered

- **Stamp inside the core** — rejected: reading a clock in `proef-core` breaks the sans-IO
  invariant that makes snapshots and property tests deterministic.
- **A second sidecar timing file** — rejected: ADR-0008 makes the JSONL event stream *the*
  record; a parallel timing artifact is a second record format.
- **Derive occupancy from `duration_ms` alone** (no injected fields) — impossible: without an
  absolute clock there is no way to know which scenarios overlapped in wall-clock time; only
  the sequential per-scenario waterfall (N6a) is derivable, which is why it shipped first.
- **Wall-clock (unix ms) instead of run-relative** — rejected: run-relative starts the
  timeline at 0, is cleaner to render, and avoids putting absolute wall-clock in the record.
