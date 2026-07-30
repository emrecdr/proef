# ADR-0007 — Cancellation: cooperative at batch boundaries, with budgets

**Status:** Accepted · **Date:** 2026-07-28

## Context

Source-verified: hurl has **no cancellation mechanism anywhere** — no signal handling in
the workspace, no abort check in the entry loop; `delay`/`retry-interval` are
uninterruptible `thread::sleep`s; `retry`/`repeat` accept `Count::Infinite`; the only
bounds are libcurl's per-request timeouts (default 300 s), which do not bound total
entry time under retries. The standard here is structural cancellation
(`CancellationToken` threaded everywhere). Verified: `tokio_util::sync::CancellationToken`
is runtime-agnostic (tokio sync primitives are documented runtime-independent;
`default-features = false` — no tokio runtime enters the tree); `is_cancelled()` polling
and `child_token()` work from plain threads.

## Decision

Cancellation is **cooperative at batch boundaries**: the orchestrator checks a per-run
`CancellationToken` (child token per scenario) before opening sessions and before each
batch; `EngineSession::run_batch` receives the token so engines *may* honor it at finer
grain when they can (a future non-hurl engine might; engine-hurl cannot mid-`run_entries`).
Stuck-batch policy, layered: (1) the pack lint **rejects infinite retries** (`retry:`
must carry a finite count) and unbounded `repeat`; (2) engine-hurl clamps per-request
timeouts and computes a **batch budget** = Σ(entry timeout × (retries+1)) + retry
intervals + margin; (3) a watchdog marks a scenario thread **abandoned** when its budget
expires — the runner records a `System` failure with full context and detaches the
thread (process exit reaps it) rather than blocking the run on an unjoinable thread.
Ctrl-C: first signal cancels the token (graceful: finish current batches, run
teardowns, write reports); second signal hard-exits.

## Consequences

Bounded, explainable runs; no dependency on hurl gaining cancellation; a clean seam for
engines that can do better. Costs: a cancel can wait out one in-flight batch (bounded by
its budget); abandoned threads leak until process exit (accepted: the process is
short-lived by design). If interrupt support ever lands upstream, adopting it is an
engine-internal change (candidate for the ADR-0003 patch pipeline).

## Alternatives considered

Killing scenario threads — unsound in Rust (no safe thread kill). Running each batch in
a subprocess for killability — reintroduces the subprocess architecture ADR-0001
rejected, per-batch. Relying on timeouts alone — unbounded under retry loops (verified),
and no graceful-report path on Ctrl-C.
