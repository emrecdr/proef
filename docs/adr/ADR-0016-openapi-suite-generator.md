# ADR-0016 — OpenAPI → suite generator (scope decision)

**Status:** Proposed — **recommendation: defer**; the *oracle/drift* mode is permanently
rejected regardless · **Date:** 2026-08-02

## Context

A recurring market expectation of a "serious" API-testing tool is to **generate tests from
an OpenAPI spec** (Schemathesis, Dredd, Step CI). proef has none, and the Round-2 scope
stress-test (IMPROVEMENT-PLAN §12.5-A) flagged it as the single biggest capability a reviewer
would call missing — while also being the one that sits *on* proef's permanent charter line.

PRD §3 names, as a **permanent non-goal**, *"API mocking/contract testing"*, and the operative
expansion (IMPROVEMENT-PLAN §3) spells it: *"contract testing (OpenAPI drift / Pact /
Schemathesis)."* Yet the validation established that a **generate-then-freeze** framing — read
the spec once at the CLI edge, emit concrete editable `.feature` + macro packs, then execute
them through the normal deterministic pipeline — technically clears the sans-IO and
determinism objections (ADR-0012 is exact precedent for IO-at-the-edge). So the question is
genuinely open and needs an ADR to settle the boundary rather than let it erode.

## Decision

**The bright line (normative, decided either way):** an OpenAPI spec may be a one-shot
**seed** — read *once* to emit prose + packs the author then **owns, edits, and maintains** —
but it may **never** become a recurring **oracle**: re-read on every run, used to
drift-check the live API against the spec, or gated on a generated-vs-committed diff. *That*
is OpenAPI-drift contract testing (PRD §3), and it is **permanently rejected.** Concretely, a
generator, if it ever exists, must have **no `--check`/`--verify`/`--diff` mode** and must
never be consulted after the initial emission.

**On the narrow scaffolder itself: defer (recommendation).** A one-shot
`proef generate --openapi spec.yaml -o suite/` that scaffolds editable prose + packs (the
shipped `bind::unbound_step` stub-gen at suite scale) is *defensible* in-charter under the
bright line, but is **not worth building now** for the reasons below. If a concrete need
emerges, it may be adopted **only** as: CLI-only (never `proef-core`), deterministic given a
seed, output fully owned by the author after emission, and bound by the line above.

## Consequences

- **Deferring records the boundary** so the omission is an intentional, documented call — and
  so the *oracle* mode is now explicitly foreclosed, not merely absent.
- **Output-quality tension (the strongest argument against).** OpenAPI describes an API in
  *endpoint* terms (`POST /records/{id}/notes → 201`); proef's value is *business* prose
  ("a member posts a note to a record"). A generator produces the former, which the author
  must rewrite into the latter — so it saves little over hand-authoring and risks a corpus of
  mechanical prose that undercuts proef's prose-first premise.
- **Dependency + direction cost.** A robust OpenAPI 3.0/3.1 parser (the dialect split,
  `$ref`, `oneOf`/`allOf`, discriminators) is a heavy, churny supply-chain surface to pin and
  clear through `cargo-deny`/`cargo-audit`. It also introduces a **new inward-ingestion
  direction** — proef has only ever flowed *outward* (ADR-0010: artifacts are emitted, never
  imported).
- **One-canonical pressure.** The first emission is `cargo new`-style and harmless; the risk
  is *re-generation*, which would give packs a second maintenance path (regenerate vs
  hand-edit). The one-shot-seed discipline is not machine-enforced, so it must be a documented
  rule.
- **Already partly covered.** The `#9` stub-gen convenience emits a paste-ready
  `match:`+`hurl:` macro skeleton for an unbound step today — the same idea at step scale,
  without any spec dependency.

## Alternatives considered

- **Full OpenAPI-drift checker** (Schemathesis/Dredd-style: spec re-consulted per run to catch
  divergence) — **permanently rejected.** It is verbatim the PRD §3 non-goal, non-deterministic
  against a live API, and the reason the bright line exists.
- **Property/fuzz-case generation from the spec** (Schemathesis-style negative cases) — out.
  *Runtime* fuzzing breaks the sans-IO/deterministic-artifact invariants; *frozen* generated
  fuzz cases are a variant of the one-shot scaffolder and share its deferral.
- **Build the narrow scaffolder now** — declined on cost/value (output quality, dependency
  weight, inward direction) despite being technically in-charter under the bright line.
  Buildable later without a new ADR, provided it obeys this one.
- **Status quo (no generator)** — the recommended near-term state; authors write prose against
  the macro vocabulary, aided by stub-gen for missing steps.

## Best-practice basis

The generate-then-freeze vs re-consult-the-oracle distinction, the sans-IO-clearance via the
ADR-0012 IO-at-the-edge precedent, and the "one `--check` flag from the non-goal" risk are
from the Round-2 scope validation (IMPROVEMENT-PLAN §12.5-A). Schemathesis and Dredd are the
reference generators; both re-consult the spec as an oracle, which is exactly what this ADR
forecloses. ([Schemathesis](https://schemathesis.io/),
[Dredd](https://dredd.org/), PRD §3, IMPROVEMENT-PLAN §12.5-A.)
