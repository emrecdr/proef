# proef — Implementation Plan

**Status:** M0–M5 delivered; M6 future · **Date:** 2026-07-28.
Normative design: [TECH-SPEC](TECH-SPEC.md); decisions: [ADRs](adr/). Sizes are
t-shirt (S ≈ days, M ≈ small weeks, L ≈ multi-week) — deliberately not fake-precise.

## 0. Guiding principles

The porting rule: when the prior spike proved a feature, understand
the *feature* and implement it the cleanest way in this architecture — no copy-paste, no
compatibility with spike code. Every milestone lands green on all CI gates. Core stays
pure (no IO/clock/rand — TECH-SPEC §4). Nothing merges without its tests
(TESTING-STRATEGY). The spike (`research/`) is evidence, not a starting codebase.

## Global definition of done (every milestone)

fmt, clippy `-D warnings`, nextest, doctests, rustdoc `-D warnings`, deny, audit all
green · new behavior has unit + (where applicable) snapshot/property tests · no `unwrap`/
`expect` in library code paths (CLI main excepted) · public items documented · CHANGELOG
entry · exit codes unbroken (assert_cmd suite).

---

## M0 — Foundations (size M)

**Objective:** compilable, gated, empty-but-shaped workspace with the seam in place.

Tasks:
1. Init repo `proef/`; virtual workspace (resolver 3), `workspace.package` +
   `workspace.dependencies` + `workspace.lints`; `rust-toolchain.toml`
   pinned to current stable (1.97.1 at writing); `deny.toml`, nextest config, README.
2. Crates: `proef-core`, `proef-engine-hurl` (empty adapter), `proef-cli` (clap
   skeleton), `xtask` (+ `justfile` aliases). Reserve crates.io names (0.0.0
   placeholders, `publish = false` locally thereafter).
3. Core scaffolding: `ExitCode` enum + `CoreError`/`EngineError` taxonomy (ADR-0009);
   `Event` enum v1 + `EventSink` (ADR-0008); `World`/`Value` types + `GlobalStore`
   (atomic temp+rename) (ADR-0005); `EngineFactory`/`EngineSession`/`StepKindSpec`/
   `DoctorCheck` traits (ADR-0002); `CancellationToken` plumbing (ADR-0007).
4. CI: gates workflow + a **stub canary job** (builds `proef-engine-hurl` against hurl
   `=8.0.1` — becomes the real canary in M4); Renovate config (pins grouped; hurl
   excluded from auto-bump).
5. `proef-cli`: `doctor` (native-lib checks from engine `doctor()` — first proof the
   capability hook works), `--version`, exit-code integration tests.

**Acceptance:** workspace builds on Linux+macOS CI; `proef doctor` reports libcurl/
libxml2 status via the engine-contributed check; assert_cmd pins exit codes; all gates
green. **Proves:** ADR-0002 seam compiles and registry assembly works.

## M1 — Front end: packs, binding, Gherkin, lowering (size L)

**Objective:** `.feature` + packs → validated, lowered scenarios; `--dry-run` without
artifacts.

Tasks:
1. Pack model (serde_norway, `deny_unknown_fields`) incl. `hurl:` raw blocks, `expect:`,
   `use:`/`with:`, `when:`, `retry:`, `saveAs:`; schemars derivation + engine
   `StepKindSpec` fragment merge; `proef schema [--add-to]`.
2. Validation passes 1–8 (TECH-SPEC §4.1) with miette diagnostics + stable codes;
   finite-retry lint; probe-instantiation parse of hurl blocks via `hurl_core`.
3. Matcher: `{name}` tokenizer + leftmost matcher + guard rails (cucumber-expression
   semantics;
   property tests: no-panic, quote round-trip, adjacent-capture rejection).
4. Gherkin: parse, directives, tags, Background, Rule pass-through, outline expansion,
   data-table merge; binding with ambiguity detection + closest-pattern suggestions.
5. Lowering: macro expansion (cycle/depth), recursive `${…}` resolver (depth 8, `$${`
   escape; property + fuzz targets), Then-merge (`expect:` → previous entry), batch
   segmentation (maximal; splits at `optional:`/engine change).
6. `proef flows`, `proef test --dry-run` (no emit yet), corpus test over `tests/`.

**Acceptance:** the four 500-series features (+ seeded error corpus) dry-run with
line-accurate diagnostics; property/fuzz targets in CI (fuzz smoke = N seconds, full =
nightly). **Proves:** PRD US-2/3/6 front-end half.

## M2 — IR, emitter, artifacts (size M)

**Objective:** lowered scenarios → canonical `.hurl` + sidecars; `--dry-run` complete.

Tasks:
1. Canonical emitter (stable formatting rules) + `# optional` markers + per-entry
   feature-ref comments; line-map construction.
2. Sidecar `<slug>.map.json` (schema v1) + `<slug>.vars`; `proef artifacts -o DIR`;
   per-run layout under `.proef-runs/<id>/artifacts/`.
3. `--dry-run` gains artifact parse-validation (`parse_hurl_file` on every emitted
   file — the real parser as the validator).
4. insta snapshot suite: features+packs → artifacts + sidecars (golden corpus).

**Acceptance:** spike parity — the 500-series features emit artifacts that stock hurl parses
(checked in CI via the canary toolchain image); snapshots reviewed. **Proves:** ADR-0010
emit half; US-7 static half.

## M3 — engine-hurl: embedded execution (size L)

**Objective:** `proef test` runs scenarios end to end via embedded hurl.

Tasks:
1. Adapter: `VariableSet` seeding (World + secrets via `insert_secret`),
   `parse_hurl_file` + `run_entries` with Buffered terms + `EventListener` → events;
   `EntryResult` → `StepOutcome` mapping via sidecar spans.
2. RunnerOptions mapping (config/directives → builder; per-entry `[Options]` override
   relied on as verified); `HurlResult.variables` merge-back; `saveAs: global`
   promotion; typed Value bridging.
3. Segmentation runtime: `optional:` warn-and-continue; `SessionState` cookie
   round-trip (Netscape temp file) for split scenarios; variables chaining.
4. Parallelism: scenario threads + `--jobs`; budgets + watchdog + token checks;
   Ctrl-C graceful/hard paths.
5. Reporters v1: console BDD tree (attempts/timings), JSONL event record, run-record
   rotation; `--output json`.
6. Fixture server (`tiny_http`, ADR-0011) + integration suite: success/4xx/retry-delay/auth/
   malformed-JSON/optional/World-chaining/cancellation-budget cases.

**Acceptance:** the 500-series features run green against the fixture with prose unchanged
(US-1); failure demo maps to feature line + artifact span; exit codes correct under
pass/fail/user-error/system-error; cancellation bounded-time test passes. **Proves:**
ADR-0001/0005/0007 runtime; US-1/4/5/9.

## M4 — Upstream tracking hardened + CI reporters (size S–M)

**Objective:** riding upstream is a runbook, not a risk; CI outputs complete.

Tasks:
1. Canary job real: build+test against next hurl release (scheduled + on release);
   failure opens an issue with the diff of `HurlResult` behavior.
2. Thin-fork rehearsal: apply a scratch one-commit patch via `[patch."crates-io"]` from
   the fork tag, build, revert — documents the mechanics; draft upstream PR #1
   (`run_entries(&mut Client)`) from the verified two-call-site change.
3. JUnit (quick-junit) + GitHub job summary reporters (`--junit auto` under
   GITHUB_ACTIONS); reporter-stack decorators (Normalize/Summarize) formalized.

**Acceptance:** canary catches an artificially-pinned older/newer hurl mismatch in
rehearsal; JUnit consumed by CI UI. **Proves:** ADR-0003; US-8/11; M-4 metric path.

## M5 — Breadth + integrations (size M)

**Objective:** the conveniences that make proef the daily tool.

Tasks: multipart/form/docstring bodies through packs; `expect:` raw-hurl assert
fragments; more `[Options]` exposure (delays, location); `--watch` (notify 8.2.0);
`proef explain`; `proef secret set|list` (encrypted store port); `${fake:*}` NL
generators; **libtest-mimic harness** (one Trial per scenario; nextest contract:
`--list --format terse`, `--exact --nocapture`) + docs for IDE use; `proef fmt` for
pack hurl blocks.

**Acceptance:** US-10/12 green; nextest runs the suite; watch-mode demo. **Proves:**
ADR-0008 harness leg; PRD v0.3 scope.

## M6 — More engines (future; sized when scheduled)

A future non-hurl engine — its step vocabulary carried as structured step payloads rather
than raw hurl. **Structural acceptance test: `git diff --stat proef-core` is empty.**
Mixed-engine 500-series suites become runnable.
(Note 2026-07-29: a driver bringing its own async runtime would conflict with the
tokio-runtime ban — pick a sync driver at sizing time. The core's structured-payload paths
are already exercised by tests; no core work is expected.)

---

## 5. Sequencing & parallelization

M0 → M1 → M2 → M3 strictly ordered (each consumes the previous stage's types). Within
M1, tasks 1–3 parallelize with 4; M2.4 can start as soon as M2.1 emits. M4.3 reporters
can start during M3.5. Docs (GETTING-STARTED + AUTHORING, delivered 2026-07-29) draft during M2–3
and finalize at M5 (deliberately after the schema stabilizes).

## 6. Risk register

| Risk | L | I | Mitigation / trigger |
|---|---|---|---|
| hurl minor release breaks the seam | M | M | exact pins; canary (M4); thin-fork shim; pinned seam integration test |
| `run_entries` `#[doc(hidden)]` churn | M | M | same as above + upstream PR #1 conversation opens a stability dialogue |
| Native build prereqs trip up a machine | M | L | `doctor` first-run UX; README one-liner; CI images pre-baked |
| Segmented scenarios lose connections/cookies | L | L | batch-maximally; SessionState; upstream patch #1 erases it |
| Runaway scenario under retries | L | M | finite-retry lint; budgets + watchdog (ADR-0007); CI timeout |
| Pack schema churn post-v1 | M | M | `schema: 1` field; additive-only until v1.0; snapshots catch drift |
| Event-schema consumers break | L | M | versioned events; JSONL replay tests |
| gherkin-crate stagnation returns | L | M | active again (0.16); parser is replaceable behind core's parse stage |

## 7. Runbook — absorbing a new hurl release

1. Canary red/green report arrives (scheduled job). 2. Read upstream CHANGELOG diff.
3. Bump pins on a branch (`=X.Y.Z`, `--locked`), run full suite + snapshots. 4. If
breakage: fix adapter; if upstream regression or removed seam: add minimal patch on the
fork branch, consume via `[patch]`, open upstream PR, note in ADR-0003 log. 5. Merge;
tag; update TECH-SPEC §14 versions. 6. If the fork carries patches: rebase them onto
the new release tag; drop any that merged upstream.

## 8. Day-one checklist

`git init proef && cd proef` → commit toolchain+workspace manifests (M0.1) → `cargo new`
the four crates → copy workspace lints/deny/nextest configs → wire CI → first green
pipeline → open M0 tracking issue with this plan's task list. Suggested first PR
sequence: M0.1+M0.2 together, then M0.3 split by module, then M0.4/M0.5.
