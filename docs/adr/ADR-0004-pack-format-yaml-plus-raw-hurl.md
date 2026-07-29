# ADR-0004 — Pack format: YAML skeleton + embedded raw Hurl blocks

**Status:** Accepted · **Date:** 2026-07-28

## Context

Requirement (stated): macro packs must be *human-readable* — "is there a better
alternative than macro YAML?" Analysis (architecture review §8) evaluated YAML+schema,
KDL, TOML, Pkl/CUE/Dhall, RON/JSON5, Rhai/Lua scripting, a custom DSL, and Karate-style
Gherkin-native macros against readability/writability, comments, multiline bodies,
templating interplay, editor tooling, serde support, and team familiarity (existing packs
are YAML with schemars-driven autocomplete). Key insight: the unreadable part of packs
was never YAML itself — it was HTTP-as-YAML-trees, while hurl's own plaintext format *is*
the human-readable HTTP DSL, and the backend team already reads/writes it fluently.

## Decision

Packs stay YAML (serde_norway; schemars JSON Schema; comments; block scalars) but only
as the **thin binding skeleton**: macro name, `match:` pattern, `params`, `defaults`,
`tags`, `description`, composition (`use:`/`with:`), step modifiers (`optional:`,
`when:`, `retry:`, `saveAs:`). The HTTP payload of a hurl step is a **raw Hurl block**:

```yaml
steps:
  - name: Resolve the client surname to its id
    hurl: |
      GET ${baseURL}/api/v1/admin/search/clients
      Authorization: Bearer ${secret:apiToken}
      [Query]
      q: ${lastName}
      HTTP 200
      [Captures]
      clientId: jsonpath "$[0].id"
```

Blocks are validated at pack load by `parse_hurl_file` after `${…}` lowering — real hurl
syntax errors with real spans. Structured step trees are *reserved for future non-hurl
engines* (`web:`/`tablet:`), which have no native text DSL. Assert-only macros use
`expect:` (merged into the previous request entry — the Then-step rule).

## Consequences

Pack bodies are literally hurl: copy-paste flows both ways with the backend corpus; no
bespoke assert/capture schema to maintain for the API engine; the emitter for hurl steps
approaches the identity function. Costs: autocomplete inside the block is plain-text
(mitigated: load-time parse errors are immediate; editors have hurl highlighting; a
`proef fmt` pass can normalize blocks); one lowering pass must run before parse (already
required for `${…}`).

## Alternatives considered

KDL — pleasant syntax but no schema/LSP story comparable to YAML, zero team familiarity;
recorded as the fallback if YAML friction materializes. TOML — wrong shape for nested
step lists (kept for `proef.toml` config). Pkl/CUE/Dhall — second language + toolchain,
over-architecture at this size. Rhai/Lua — packs become programs; kills static
validation and `--dry-run` guarantees. Custom DSL — a parser/LSP/formatter to own
forever. Karate-style callable feature files — collapses the macro/test distinction;
a typed params/defaults/validation model is strictly stronger.
