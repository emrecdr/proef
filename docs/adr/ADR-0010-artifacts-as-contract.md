# ADR-0010 — Artifacts as contract: emitted `.hurl` is the executed input

**Status:** Accepted · **Date:** 2026-07-28

## Context

The backend team's hurl corpus makes `.hurl` the interop format. The spike proved
generated artifacts run identically under the prototype engine and the stock CLI — and
that keeping *two* implementations honest requires differential testing. Embedding hurl
(ADR-0001) enables something stronger: the artifact and the executed input can be the
*same bytes*. Verified seam facts that shape the mechanics: `run_entries` creates its
HTTP client per call (fresh connections; cookie jar seedable only via Netscape-format
file; variables chain losslessly via `HurlResult.variables`); per-entry `[Options]`
override batch-level `RunnerOptions` defaults (clone-then-override, verified).

## Decision

For every scenario, the emitter produces canonical `.hurl` text; **that exact text** is
what `parse_hurl_file` + `run_entries` execute — drift between artifact and execution is
structurally impossible. Alongside each artifact: a **sidecar map** (`<slug>.map.json`:
entry ↔ feature file/line/step text, optional flags, capture names, batch boundaries)
and, when World/global values are referenced, a generated `<slug>.vars` file so the
backend team can replay with `hurl --variables-file`. `optional:` entries carry an
`# optional` marker comment (no hurl equivalent — the runner segments around them).
Execution batches **maximally**: one `run_entries` call per scenario unless `optional:`
boundaries or interleaved other-engine steps force a split; when a split occurs,
variables chain via `HurlResult.variables` and cookies (if used) round-trip via a
Netscape temp file behind a `SessionState` struct. Queued upstream patch #1 (per
ADR-0003): `run_entries` accepting `&mut http::Client` — verified two-call-site change —
which erases per-segment connection/cookie costs entirely. Artifacts never contain
secret values (ADR-0005). Artifact output: per-run under `.proef-runs/<id>/artifacts/`
plus `proef artifacts` for a stable CI hand-off directory.

## Consequences

Debugging = opening the artifact with tools the team already knows; the backend corpus
and proef packs stay mutually copy-paste-able; `--dry-run` validation includes parsing
the real artifact with the real parser. Cost: canonical formatting is a compatibility
surface (snapshot-tested); segmented scenarios pay reconnection costs until patch #1
lands upstream.

## Alternatives considered

Internal-only IR execution with optional export — loses the same-bytes guarantee and
demotes artifacts to lossy exports. Differential-oracle architecture (v1 plan) —
superseded by ADR-0001; preserved in `research/` as the static-distribution fallback.
