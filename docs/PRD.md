# proef — Product Requirements Document

**Status:** approved; US-1…US-12 all in service (multi-engine architectural only), plus
post-M5 work: external config/environments (ADR-0012), the v0.6–v0.8 correctness series,
v0.9.0, named hurl fragments (ADR-0018 — see the §3 amendment), the adoption response,
the 0.11–0.14 hardening and CI-scale series, the RF capability audit and reserved tags
(ADR-0019/0020), and the deep-improvement waves plus round 19. `CLAUDE.md`'s Status
block is the running ledger; the goals and non-goals below are what has not changed.
· **Date:** 2026-07-28, status refreshed 2026-08-31 · **Owner:** Emre
**Companion docs:** [ADRs](README.md#decision-log-adr-index) for the *why*, [TECH-SPEC](TECH-SPEC.md) for the *how*,
[IMPLEMENTATION-PLAN](IMPLEMENTATION-PLAN.md) for the *when*.

## 1. Problem & context

End-to-end tests written as Gherkin business prose — bound to a compiled-in vocabulary
of YAML macro packs — let non-programmers author real tests. Meanwhile the backend team maintains a
corpus of hand-written **Hurl** files as its API-testing lingua franca. There is no tool
that joins the two: Gherkin prose on top, Hurl-grade API execution underneath — and none
whose engine seam can later admit further engines behind the same prose (architectural
readiness only).

**proef** is that tool: a declarative, modular, multi-engine e2e runner (mostly Rust).
One core parses/binds/lowers Gherkin; pluggable engines execute. The API engine embeds
hurl itself (ADR-0001), so API semantics are hurl's by construction, and every scenario
also produces real `.hurl` artifacts the backend team can run and read (ADR-0010).

## 2. Goals

G1. Author API e2e tests as pure Gherkin prose in the 500-series style — no code,
no URLs in prose, data tables and Scenario Outlines supported.
G2. Execute with hurl's exact semantics (asserts, captures, retries, templating) via the
embedded engine; identical results to the stock hurl CLI on the same artifacts.
G3. Emit canonical `.hurl` artifacts + sidecar maps for every scenario — debuggable with
the tools the backend team already uses, replayable via `hurl --variables-file`.
G4. Be modular: adding an engine is a new crate + a registry line, with
zero changes to `proef-core` (the structural acceptance test, ADR-0002).
G5. Track upstream hurl safely over time: exact pins, a zero-diff fork as patch vehicle,
and an upgrade-canary CI job so new hurl releases are absorbed deliberately (ADR-0003).
G6. First-class failure UX: every failure maps to the `.feature` line (and the artifact
span), rendered with miette-style labeled diagnostics; stable exit codes 0/1/2/3.
G7. CI-native: JUnit XML, GitHub job summary, JSONL run records, tag filtering, parallel
scenarios, `--dry-run` validation gate, and (M5) a libtest-mimic harness so
`cargo nextest run` and IDEs can drive scenarios.

## 3. Non-goals (v1)

Further engines (designed-for, not built — M6+); a desktop dashboard or server
mode (precedent exists when needed); **generating** Gherkin, macros, or prose from
hand-written hurl files (see the amendment below); OpenTelemetry export (semconv still
immature — JSONL is the source of truth); dynamic plugin loading; Windows-static or
musl-static binaries (dynamically linked like hurl's own — consequence of ADR-0001);
API mocking/contract testing; load testing.

### Amendment (2026-08-11): the hurl non-goal is about *generation*, not direction

This non-goal previously read "importing/round-tripping *hand-written* hurl files into
Gherkin (artifacts flow outward only)". The clause and its parenthetical said two
different things, and the parenthetical was the broader of the two: read literally it
forbids hurl text being an *input* at all, which ADR-0018 (named hurl fragments) needs
it not to.

What the non-goal protects is that **proef never authors a test for you**. A suite's
prose and its binding vocabulary are written by people, deliberately; a tool that
derives them from an existing corpus produces scenarios nobody chose the words for,
and the review that makes a feature file worth having never happens. That reasoning is
untouched, and ADR-0016 (OpenAPI generation) stays declined on the same ground.

It does not extend to hurl text being an input *source*. A macro pack is already an
input written in a non-Gherkin language; a `.hurl` file naming reusable fragments is
another, and §1's own framing — "the backend team maintains a corpus of hand-written
Hurl files as its API-testing lingua franca. There is no tool that joins the two" —
describes joining that corpus as the product's purpose, not as a boundary. Nothing is
generated: features and macros stay hand-authored, and a fragment is inert until a
macro names it. The worklist reached the same reading independently for a different
item (OPEN-FINDINGS M2: "the non-goal forecloses a direction of data flow, not the
ability to check your own work").

**Recorded honestly:** M3 asked that this charter be re-examined *with* a measured
port cost, and that measurement does not exist. The narrowing is therefore argued from
the non-goal's own rationale rather than from evidence that pasting is expensive. If
the measurement later shows pasting is cheap, that argues about *priority* — it does
not restore a prohibition this amendment finds was never the point.

## 4. Users & personas

**P1 — Test author** (QA, PM, support engineer; not necessarily a programmer). Writes
`.feature` files against the existing step vocabulary. Cares about: prose that reads
naturally, `--dry-run` telling them *exactly* which step is wrong, failures pointing at
their line, not internals.
**P2 — Pack maintainer** (developer). Owns the macro packs: adds steps, params,
asserts. Cares about: pack readability (raw hurl blocks, ADR-0004), schema autocomplete,
load-time validation with did-you-mean hints, safe refactors (duplicate/cycle detection).
**P3 — Backend engineer** (owns the hurl corpus). Consumes emitted artifacts; pastes
between corpus and packs, or — since ADR-0018 — annotates a corpus file once and lets
packs name its entries, so the same file stays runnable under stock `hurl`. Cares
about: artifacts being idiomatic hurl, never containing secrets, runnable standalone;
and that proef reading their corpus never edits or reformats it.
**P4 — CI pipeline** (machine). Cares about: stable exit codes, JUnit/JSONL outputs,
deterministic behavior, bounded runtime (cancellation/budgets, ADR-0007), the canary job.

## 5. User stories & acceptance criteria

US-1 (P1) As a test author I write a scenario in prose and run it against a live
environment. *AC:* the four 500-series `.feature` files run via proef packs with
prose unchanged except agreed wording fixes; `proef test` runs them green against the
fixture; failures name feature file + line + step text.
US-2 (P1) I validate without executing. *AC:* `proef test --dry-run` binds every step,
expands every macro/outline, resolves `${…}`, parses every generated artifact with hurl's
parser, and exits 2 with labeled diagnostics on any failure — no network I/O.
US-3 (P1) I pass data per step. *AC:* inline `{captures}`, `| key | value |` data tables,
and Scenario Outline `<placeholders>` all fill macro params; conflicts and missing
required params are parse-time errors naming the line.
US-4 (P1) Steps chain state. *AC:* a capture in one step (`clientId`) is usable in later
steps of the scenario as `{{clientId}}`; `saveAs: global` persists across scenarios and
runs (World, ADR-0005).
US-5 (P1) Slow backends don't flake. *AC:* a step with `retry:` polls until its asserts
pass or the finite budget ends (maps to hurl `[Options] retry`); `optional:` steps warn
instead of failing.
US-6 (P2) I extend the vocabulary. *AC:* adding a macro with a `match:` pattern + raw
hurl block to a pack makes the new prose step available; `proef schema` reflects it; load
rejects ambiguous names, adjacent captures, literal-free patterns, infinite retries.
US-7 (P3) I get artifacts. *AC:* `proef artifacts` writes per-scenario `.hurl` +
`.map.json` (+ `.vars` when Worlds are referenced); stock `hurl --test` yields the same
verdicts (spike-proven); no secret values appear in any artifact.
US-8 (P4) CI consumes results. *AC:* exit codes 0/1/2/3 stable and integration-tested;
`--junit auto` under GITHUB_ACTIONS; JSONL event log written per run; `--tags` filters.
US-9 (P1/P4) Runs are observable. *AC:* console BDD tree with per-step timing/attempts;
`proef explain [run]` summarizes the latest failures from run records; `proef diff
[base] [new]` compares two runs for regressions, fixes, flakiness, and perf deltas;
`proef report [run]` writes a self-contained HTML report of a run.
US-10 (P2) Secrets stay secret. *AC:* `proef secret set` stores encrypted values;
secret values never appear in artifacts, logs, reports, or events (property-tested).
US-11 (P4) hurl upgrades are safe. *AC:* the canary job builds against the next hurl
release and replays the suite; pins move only after it is green (runbook in
IMPLEMENTATION-PLAN §7).
US-12 (P1, M5) IDE/nextest integration. *AC:* the libtest-mimic harness lists one test
per scenario and `cargo nextest run` executes and reports them.
US-13 (P1) I can start from something that works. *AC:* `proef init` writes a
minimal suite (`proef.toml`, one `.feature`, one matching pack) that passes
`--dry-run` unchanged, installs the pack JSON Schema for editor completion, and
never overwrites an existing file.

## 6. Functional requirements (condensed; TECH-SPEC is normative)

**Authoring:** full gherkin-crate grammar (Feature/Rule/Background/Scenario/Outline/
Examples/tables/docstrings/tags/i18n); tags filter runs; variables come from
`proef.toml` (`${url:}`/`${vars:}`, ADR-0012), never the feature files. **Packs:** YAML
skeleton with `match:` patterns
(`{name}` captures), `params`/`defaults`/`tags`/`description`; step bodies as raw
`hurl:` blocks (primary) or structured form (reserved for future engines); assert-only
`expect:` macros merge into the previous request (Then-steps); `use:`/`with:` nesting
with cycle/depth limits; schemars-derived JSON Schema; lint pass at load. **Execution:**
scenario = unit of isolation and parallelism (`--jobs`); contiguous same-engine batching;
engine-hurl via embedded `run_entries` with buffered I/O; per-entry `[Options]` override
batch defaults (verified); World seeding/merge-back; cooperative cancellation + budgets.
**Artifacts:** canonical emit, sidecars, vars files, `# optional` markers. **Reporting:**
event spine → console/JUnit/JSONL/GitHub-summary reporters; run-record rotation.
**CLI:** `test` (`[path] --env --dry-run --tags --jobs --junit --format json|tap --watch --run-id --sarif --rerun
--scenario[-file]`; path optional — `[run] suite` then the `tests/` convention), `flows`,
`macros` (call counts + dead-macro report), `artifacts`, `schema [--add-to]`,
`secret set|list`, `explain`, `diff [base] [new] --fail-on-regression`,
`report [run] -o <file>`, `doctor`. **Config
(`proef.toml`, ADR-0012):** runner settings (`[run]` incl. `setup`/`teardown` suite
lifecycle features, ADR-0014; `[http]`/`[sla]`) + suite variables
(`[url]`/`[vars]`, referenced `${url:key}`/`${vars:key}`) + per-environment overrides
(`[env.<name>]`); precedence defaults < base tables < active `[env.<name>]` (via
`--env`/`PROEF_ENV`) < flags. Secrets via `PROEF_SECRET_<NAME>` env override → the
encrypted store (`proef secret set` — hidden prompt, or `--stdin` for
scripts), never in `proef.toml`.

## 7. Non-functional requirements

**Correctness/fidelity:** API semantics are hurl's own binary-identical engine — no
reimplementation drift is possible (ADR-0001/0010). **Portability:** Linux (glibc),
macOS, Windows; documented build prereqs (libcurl/libxml2/libclang), `doctor`-checked;
dynamically-linked dist binaries (hurl's own model). **Performance:** startup overhead
(parse+bind+lower for a 30-scenario suite) under ~1 s; execution dominated by the network;
scenario-level parallelism with worker threads. **Reliability:** deterministic lowering
(injected clock/run-id, sans-IO-lite core); bounded runtime under cancellation budgets;
state writes atomic (temp+rename). **Security:** secret redaction invariants
property-tested; artifacts guaranteed secret-free; encrypted-at-rest secret store;
0600 on sensitive outputs. **Maintainability:** exact pins + canary + thin-fork policy;
CI gates: fmt, clippy `-D warnings`, nextest, doc `-D warnings`, deny, machete,
zizmor, docs-check, public-api (audit nightly).
**Extensibility:** new engine = new crate implementing `EngineFactory`/`EngineSession` +
one registry line; step-kind schema contributed by the engine; **zero core changes**
(the M6 acceptance test).

## 8. Success metrics

M-1: the four 50x API features run under proef with prose unchanged (US-1) — the
port-fidelity bar. M-2: artifact parity — stock hurl CLI verdicts match proef verdicts on
every artifact in the integration suite (already spike-proven; kept as a CI invariant
until M4, then by construction). M-3: `--dry-run` catches 100% of the seeded
pack/feature error corpus with line-accurate diagnostics. M-4: one hurl upstream release
absorbed via the canary runbook with zero suite regressions. M-5: a future non-hurl engine lands (M6)
with `git diff --stat proef-core` empty.

## 9. Release phasing

v0.1 = M0–M3 (authoring, validation, embedded execution, artifacts, console+JSONL);
v0.2 = M4 (upstream tracking hardened, JUnit/GitHub reporters); v0.3 = M5 (breadth:
multipart/form/docstring bodies, watch, explain, libtest-mimic harness); v1.0 = stability
declaration of pack schema + CLI + event schema; M6 engines version independently.
Detailed task breakdown: [IMPLEMENTATION-PLAN.md](IMPLEMENTATION-PLAN.md).
