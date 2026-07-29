# ADR-0005 — Two-tier variables (`${…}` / `{{…}}`), the World, and secrets

**Status:** Accepted · **Date:** 2026-07-28

## Context

The author-time variable system (`${var}`, `${env:NAME:-default}`, `${run:id}`, `${global:key}`,
`${fake:*}` seeded from the run id, `$${}` escape, recursive expansion) is proven with
test authors. Hurl has its own runtime templating (`{{name}}`) fed by captures. The spike
validated running both tiers side by side — including the bug it surfaced: step-captured
args can themselves contain `${…}` and need recursive (depth-capped) resolution.

## Decision

Two explicit tiers. **`${…}` is author time**: params, env (with defaults), run id,
World reads (`${global:key}`), fake data, secrets references — resolved during lowering,
recursively with a depth cap of 8, and *baked into artifacts* (except secrets). **`{{…}}`
is run time**: hurl-native templates for captures, left verbatim in artifacts so the
embedded engine and the stock CLI resolve them identically. **World:** one typed variable
scope per scenario plus a persistent global store (`.proef-state.json`, atomic
temp+rename). Engine
bridging: seed hurl's `VariableSet` from the World before each batch; merge
`HurlResult.variables` back after; `saveAs: global` promotes a capture into the
persistent store. **Secrets:** `${secret:NAME}` resolves from the `PROEF_SECRET_<NAME>` environment
override, else the encrypted store `.proef-secrets.json` (chacha20poly1305 + rpassword); values are injected via
`VariableSet::insert_secret` (hurl redacts them in logs/reports); artifacts carry
`{{secret_name}}` placeholders, never values; our reporters additionally redact by value
(property-tested invariant).

## Consequences

Artifacts are runnable by both toolchains with identical meaning; authors keep a familiar
mental model unchanged; secrets are structurally absent from every persisted output.
Cost: two syntaxes coexist in packs — mitigated by the strict rule of thumb ("`$` =
before the run, `{{` = during the run") documented in the pack authoring guide.

## Alternatives considered

Single-tier (resolve everything at author time) — breaks capture chaining and makes
artifacts non-parametric. Single-tier (everything hurl `{{}}`) — loses env defaults,
fakes, and World reads. String-only World — kept
*typed* here (hurl `Value` model) because captures cross engines; stringly-typed
round-trips would lose numbers/bools at engine boundaries.

## Errata

**2026-07-28 (post-M5 hardening; API removed 2026-07-29 per YAGNI):**
"snapshot/restore across scenario retries" originally described a mechanism whose
*trigger* was never specified anywhere in the corpus — no
CLI flag, tag, or pack directive schedules a scenario-level retry (US-5's step-level
`retry:` is implemented and is the flake tool in practice). Decision: scenario-level
retries are **deferred** indefinitely, and the unused `GlobalStore::snapshot`/`restore`
API has been **removed** (YAGNI: no dead promised-behavior code). Whoever implements
scenario retries specifies the trigger surface and its mechanism in a superseding ADR. Also note: the scenario merge-back is **write-set-only** (`World`
tracks its `saveAs` promotions) — merging a whole snapshot back would lose concurrent
scenarios' updates.
