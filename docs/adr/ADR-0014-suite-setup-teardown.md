# ADR-0014 — Suite-level setup & teardown

**Status:** Accepted · **Date:** 2026-08-02

## Context

Gherkin `Background` runs steps once **per scenario**, and a persistent global store
threads `saveAs: global` captures across scenarios and runs (ADR-0005). What is missing is
a **once-per-suite** phase: authenticate once and share the token, seed a fixture before
the run and tear it down after. Cucumber solves this with `@BeforeAll`/`@AfterAll` hooks;
Karate with `callSingle`. Round-2 code validation (IMPROVEMENT-PLAN §12, item N3) fixed the
constraints:

- **Tags never reach the sans-IO core runner.** `ScenarioSpec`/`ScenarioOutcome` carry no
  tags; `@quarantine` is computed at the *CLI edge* (`exec.rs`) as a `non_gating` set. So a
  lifecycle phase must be orchestrated at the CLI edge, not inside the runner.
- **Only `saveAs: global` promotions cross scenarios**, and each pooled scenario snapshots
  the global store at prepare time — so a setup phase must **finish and merge its globals
  before the parallel pool starts**, or early scenarios miss the seeded value.
- **An assert-failed setup must not be masked.** A setup step that fails an assertion
  classifies as a test failure (`fault: None`), which the worst-wins fold would let through
  as exit 1 *without aborting the pool* — every scenario would then run against un-seeded
  state and cascade confusing failures.

## Decision

- Two new `proef.toml` keys — **`[run] setup` and `[run] teardown`** — each naming a
  feature file. `setup` runs (its scenarios, in order) **once before** the suite pool;
  `teardown` **once after**. State reaches the suite through the existing `saveAs: global`
  store; setup completes and merges before the pool is built.
- **Orchestrated in `execute()`** around the parallel pool (CLI edge), never in the sans-IO
  core. The setup/teardown feature is **excluded from `build_specs`** so it never also runs
  as an ordinary scenario, and it is invisible to `--tags`/`--scenario`/`--rerun`.
- **Failure semantics (exit-code contract, ADR-0009)** — validated against Playwright,
  Jest, k6, and pytest (see basis below):
  - A **setup** failure of any kind **aborts the run before the pool launches** and is
    never masked. It maps to a **user (2)** or **system (3)** fault, *not* a test failure
    (exit 1) — a broken fixture is not a failing test, the same distinction Playwright
    draws between a "clear setup error" and a "cryptic test failure."
  - **Teardown runs only when setup succeeded** (gated on setup-success, not pool-success):
    k6 and pytest-`yield` both skip teardown when setup threw, because tearing down
    un-created state is itself a fault source. The setup feature is responsible for cleaning
    its own partial state on failure.
  - **Teardown does run after the pool even when scenarios failed** (Playwright/pytest/k6
    all do), so cleanup is reliable.
  - A **teardown** failure is loudly reported and yields a **distinct non-zero signal**
    (a system/cleanup fault, exit 3) — never a silently-green suite (no mainstream tool
    masks a teardown failure). It stays distinct from a test failure (exit 1): a cleanup
    hiccup does not mean the API under test is broken, but it is not hidden.
- `--dry-run` is unaffected: it validates only (never calls the runner), and the
  setup/teardown features are validated like any other feature but never executed.

## Consequences

- A genuine once-per-suite lifecycle, expressed declaratively (Hurl prose in a feature),
  with **no code hooks** — the glue-code path proef exists to avoid.
- Exactly **one** setup and **one** teardown mechanism — a `[run]` construct, not a second
  `Background` concept and not a tag. The setup feature is authored like any suite feature
  and reuses the whole pipeline (bind → lower → emit → execute).
- Touches the pinned exit-code tests: a failing setup gates the run; new `assert_cmd`
  cases cover the short-circuit and the "setup fault is never masked" invariant.
- Product-neutral: the setup steps are ordinary prose bound to macros; nothing about the
  mechanism assumes a particular backend.
- **Auth-once boundary (explicit):** the pattern that motivates suite-setup elsewhere —
  "authenticate once, share the token" (Playwright `storageState`, Karate `callSingle`) —
  is *largely already covered* in proef by a **pre-set secret** (`proef secret set` /
  `PROEF_SECRET_*`) resolved per scenario. Sharing a *runtime-obtained* secret through setup
  would collide with the invariant that `saveAs: global` refuses secret-valued captures
  (ADR-0005). Promoting a runtime capture into the secret channel is therefore **out of
  scope** for this ADR: the global store carries only non-secret setup state (seeded ids,
  fixture config). Recorded so the boundary is explicit rather than discovered later.

## Best-practice basis

Config-key-names-a-file is the dominant model (Playwright `globalSetup`/`globalTeardown`,
Jest `globalSetup`, Vitest) — the `[run]` table already supplies the "once per run"
qualifier, so `[run] setup` reads like `globalSetup` without repeating `global`. Passing
state out-of-band through a serialized shared store (not live memory) is universal —
Playwright's `storageState` file, Jest's env-var workaround, Karate's `callSingle` cache,
k6's returned data — and proef's global store is the direct analog. Setup-completes-before-
workers and setup-failure-aborts are unanimous. The refinements above (teardown skipped on
setup failure; teardown failure is non-zero, not silent) come straight from k6/pytest and
the Playwright/pytest "teardown is not silently swallowed" norm.
([Playwright global setup](https://playwright.dev/docs/test-global-setup-teardown),
[Jest config](https://jestjs.io/docs/configuration),
[k6 lifecycle](https://grafana.com/docs/k6/latest/using-k6/test-lifecycle/),
[pytest #2508](https://github.com/pytest-dev/pytest/issues/2508),
[Karate callSingle](https://docs.karatelabs.io/core-syntax/configuration/))

## Alternatives considered

- **`@setup` / `@teardown` tags** — rejected. A tag means "filter/select"; overloading it
  with "change execution phase" is a second meaning, and tagged setup scenarios would
  entangle with `--tags`/`--scenario`/`--rerun`/`flows`/name-dedup with undefined ordering
  between two `@setup` scenarios. A `[run]` construct is single, explicit, and ordered.
- **Code hooks (`Before`/`After` functions)** — rejected. Arbitrary-code hooks are exactly
  the imperative glue the declarative-macro model avoids, and they break the sans-IO
  boundary. The legitimate need (setup/teardown IO) is met with Hurl steps instead.
- **Per-scenario `Background` only (status quo)** — does not cover once-per-suite work;
  authenticating in every scenario's Background is wasteful and cannot seed shared state
  that must exist before the first scenario.
- **A dedicated `[setup]`/`[teardown]` top-level table** — rejected as heavier than needed;
  these are run-orchestration knobs, so they belong under `[run]` beside `suite`/`jobs`.
