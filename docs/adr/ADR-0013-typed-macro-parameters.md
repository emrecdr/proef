# ADR-0013 — Typed macro parameters

**Status:** Proposed — **recommendation: defer** (the shape below is recorded for when a
real need appears) · **Date:** 2026-08-02

## Context

A macro declares `params` and binds `{capture}` values from prose, data-table rows,
`defaults`, and `use:`…`with:`. Every arg is an **untyped string** today: the matcher
only checks that a capture *names* a declared param (`matcher.rs`), never that the value
looks like what the step expects. A mistyped value (`the record abc is fetched` where a
UUID was meant) is caught only at hurl run time — if at all — not by `--dry-run`.

Cucumber Expressions solve this with typed parameters (`{int}`, `{uuid}`, custom types)
validated before execution. proef wants the same shift-left check, expressed within its
architecture. Round-2 code validation (IMPROVEMENT-PLAN §12, item N1) established three
hard constraints:

1. **Placement must be the declaration site, not the pattern.** Three of the four arg
   sources are not captures (data-table rows, `defaults`, `with:`), and `use:`-only
   macros have *no pattern at all* — so an inline `{name:type}` form could not type them,
   and a type buried in the `match:` string is invisible to `proef schema` (schemars).
2. **The two-tier variable rule caps the value.** An arg may legitimately be `${…}` /
   `{{…}}` that only resolves at lower/run time (ADR-0005). Type-checking such an arg at
   bind time would false-positive, so the check must **skip any raw arg containing `${`
   or `{{`** — it is a best-effort lint over *literal* args, not a runtime type system.
3. **One-canonical-way forces a single `params` spelling.** `params` is a YAML sequence
   today (`[q, index]`); adding a *typed* spelling alongside it would be two ways to
   declare params — the golden-rule violation that killed the `templates:` alias and the
   `# key:` directive.

## Decision

- `params` becomes a **name → type map**: `params: {q: uuid, index: int, note: any}`.
  A missing/`any` type means "declared, unchecked" (today's behaviour). Parsed with a
  custom `Deserialize` (not an untagged enum — CLAUDE.md bans those near the
  `arbitrary_precision` footgun). **This is a breaking pack-shape change** — every
  existing pack migrates `params: [q, index]` → `params: {q: any, index: any}`, with no
  alias (consistent with the `templates:`→`macros:` and `# key:` removals).
- The type set is a **closed registry** — `any`, `int`, `number`, `uuid`, `email`,
  `iso8601`, `word` — modelled on the `fake::GENERATORS` registry (`fake.rs`), validated
  at load so an unknown type is a pack error. **No user-defined types in v1** (revisit if
  a real need appears).
- Validation lands at three sites: bound captures + data-table rows at **bind time**
  (`proef::bind::param_type_mismatch`), and static `defaults` / `with:` values at **load
  time** (next to the existing `default_not_param` / `unknown_with_key` checks). All
  checks are **skipped when the raw arg contains `${` or `{{`** (constraint 2). Datetime
  types parse via `jiff` (sans-IO — literal parse reads no clock).
- `proef schema` reflects the typed `params` shape (constraint 1 satisfied).

## Consequences

- **Breaking**: every pack's `params` migrates to the map form. A one-time mechanical
  change, called out in CHANGELOG and GETTING-STARTED; the error-corpus gains a
  `bind__param_type_mismatch` case.
- **Best-effort, not a guarantee**: literal args are checked; `${…}`/`{{…}}` args are
  not. This must be documented so authors do not read it as a type system — its value is
  catching *typos in literal prose* at `--dry-run`, not enforcing runtime types.
- `--dry-run` gains a real new class of caught mistake; `proef schema` autocomplete gets
  richer.
- The honest cost/benefit: a **breaking migration** for a **best-effort literal-args
  lint**. Recorded so the trade is deliberate, not incidental.

## Best-practice basis & recommendation

Research into the field (2025–2026) is decisive on **form** and honest about **worth**:

- **Form (if built):** the industry norm is **declaration-site typing referenced by name**,
  never a full type spec inline. Cucumber Expressions put only a bareword type-name in the
  pattern (indexing a registry); SpecFlow/Reqnroll type by the binding method's *return
  type*; Bruno (v4, 2024) added declaration-site typed variables with a string default. A
  **`name → type` map is the canonical shape** (JSON Schema `properties`, OpenAPI). The
  inline `{name:type}` form (behave/pytest-bdd) only works because their "declaration" is
  code, not a schema — with a schema it *loses* tooling. So the decision above (single-shape
  map, closed vocabulary) is the correct form, and the **list-or-map union is the worst
  option on every axis** (permanent double surface, weaker autocomplete, bifurcated
  examples — the exact ambiguity "one canonical way" forbids). ESLint's flat-config hard
  break (v9→v10, bounded deprecation then cut) is the precedent for a single-owner format
  choosing a clean break over an indefinite dual-shape.
- **Worth (candid):** a best-effort lint over *literal* values is the mypy/TypeScript
  gradual-typing bargain — genuinely useful for shallow typos, *provided* it degrades to
  "unchecked," never "pass," on the parts it can't see. But an API test runner's args are
  **deferred-heavy** (base URLs, captured ids, run-time tokens — all `${…}`/`{{…}}`), the
  exact population the lint cannot check, which shrinks the realized benefit. proef's own
  corpus bears this out: today's params are overwhelmingly string-ish, so the high-value
  types (`uuid`/`int`/`iso8601`) would rarely fire. Karate — the nearest-neighbour API tool
  — never types inputs at all; it types *responses*.

**Recommendation: defer.** The clean, best-practice-aligned move for proef *right now* is
not to add a low-current-value lint behind a breaking format change; it is to record the
correct shape (above) and adopt it if a pack corpus emerges where structured literal args
are common. Building it earlier is defensible only in the disciplined form above — never as
a list-or-map union. *(If the operator wants it now regardless, implement the single-shape
map with the honest-degradation discipline; the migration is mechanical.)*
([Cucumber Expressions](https://cucumber.io/docs/cucumber/cucumber-expressions/),
[Reqnroll conversions](https://docs.reqnroll.net/latest/automation/step-argument-conversions.html),
[Bruno typed variables](https://docs.usebruno.com/variables/overview),
[OpenAPI 3.1](https://spec.openapis.org/oas/v3.1.0.html),
[ESLint 10 removal](https://infoq.com/news/2026/04/eslint-10-release),
[Karate schema validation](https://docs.karatelabs.io/assertions/schema-validation/))

## Alternatives considered

- **Inline `{name:type}` (Cucumber-Expression style)** — rejected: invisible to
  `proef schema`, mis-binds in the tokenizer (`{q:uuid}` becomes a capture literally named
  `q:uuid`), and covers only 1 of the 4 arg sources.
- **`params` accepts *either* a sequence (untyped, non-breaking) *or* a map (typed)**, via
  custom `Deserialize` — **the non-breaking alternative**. Rejected under one-canonical-way
  (two shapes for one field), but it is the fallback if a breaking migration is judged too
  costly for a best-effort lint. *(This is the open fork for the operator.)*
- **Keep params untyped (do not do N1)** — the status quo; the shift-left gap stays. A
  legitimate choice given the cost/benefit above.
- **User-defined type registry** — deferred; a closed set is simpler and covers the common
  shapes.
