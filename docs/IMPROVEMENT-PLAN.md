# proef — Improvement Plan

**Status:** proposed (post-M5 competitive review) · **Date:** 2026-07-31 · **Owner:** Emre
**Companion docs:** [PRD](PRD.md) (scope + the binding **non-goals**, §3), [adr/](adr/) (the
invariants every item must respect), [TECH-SPEC](TECH-SPEC.md) (types/pipeline),
[IMPLEMENTATION-PLAN](IMPLEMENTATION-PLAN.md) (milestones + definition of done).

## 0. What this is (and isn't)

A competitive analysis of proef against the BDD and API-testing field, converted into a
roadmap of candidate improvements — each **validated against the current architecture at
file:line**, and filtered strictly through proef's permanent non-goals (PRD §3).

Nothing here is committed work; it is the durable output of the post-M5 review round.
Effort grades (S/M/L) and sequencing are advisory. Feature numbers (#1…#15) are stable
identifiers used across the tables. **File:line citations are as of 2026-07-31 and will
drift** — treat them as "start reading here", not addresses.

Every item is **in scope by construction**: none proposes a second engine, mocking,
contract testing, load testing, a dashboard/server, OpenTelemetry, or importing
hand-written hurl — all permanent non-goals (§3 below). The work is almost entirely
*surfacing data the sans-IO core already computes*, not new engine capability.

## 1. Headline finding

**proef's engine is already ahead of its cohort; the gaps are in reporting surfaces and
authoring/maintenance DX, not in HTTP power.** Because proef pins **hurl 8.0.1**, it
already ships hurl 8.0's full assertion arsenal (RFC 9535 JSONPath with filter functions,
type predicates `isUuid`/`isIsoDate`/`isString`/`isObject`/`isList`, response-time
`duration < ms`, the filter chain `split`/`count`/`toDate`/`base64Decode`/`daysAfterNow`).
Pack authors can write **Karate-grade assertions today**, inside raw `hurl:` blocks — they
are just undocumented and under-surfaced. The roadmap is therefore mostly *exposure and
tooling*, which is cheap, rather than engine work, which is done.

## 2. Where proef already wins (positioning to defend)

| proef strength | Competitor weakness it beats |
|---|---|
| One canonical way, raw-hurl-only, **no escape-hatch language** | Karate's most-cited flaw: a *three-language* model (Gherkin + DSL + embedded JS) the moment anything gets non-trivial |
| Deterministic **sans-IO core** | Karate/Tavern/Newman are non-deterministic; reproducibility is now a headline selling point |
| **Artifacts = executed bytes** (hash-locked, git-diffable, replayable `hurl --test`) | Postman's opaque JSON collections (unreviewable merges) — the reason Bruno is displacing it |
| **Finite retries + budgets + watchdog** (ADR-0007) | hurl itself has *no cancellation and unbounded retries* — proef fixes its own engine's biggest gap |
| Property-tested **secret masking** + typed **exit codes** (ADR-0009) | Most tools treat masking loosely and lack a stable exit-code contract |
| Dev-maintained macro packs = an **enforced "step dictionary"** | The AI-authoring trend is groping toward exactly this; proef has it structurally |
| No cloud, no account, plain text | Postman's 2026 pricing exodus is driving the whole git-native wave |

## 3. Scope guardrails — what this plan will NOT propose

**Permanent non-goals (PRD §3) — never revisit under "competitor parity":**
further engines (browser/gRPC/etc.), API mocking, contract testing (OpenAPI drift / Pact /
Schemathesis), load testing, a desktop dashboard or server mode, OpenTelemetry export,
dynamic plugin loading, importing hand-written hurl into Gherkin (artifacts flow outward
only), static musl/Windows binaries.

**Named anti-patterns (from the 2025–2026 trend research) to avoid:**
silent retries / green-on-attempt-2; retry ceilings > 3 or unbounded (proef's finite cap
is already correct); permanent quarantine without owner+expiry; treating masking as a
security *boundary*; LLM self-healing / non-deterministic test mutation; config sprawl.

**A Karate-style marker DSL (`#uuid`, `##optional`) is explicitly rejected** — it would be
a *second* assertion mechanism competing with raw hurl predicates. Achieve the same
readability by surfacing hurl's native predicates (item #2), not by inventing a layer.

**The overriding design gate is one-canonical-way** (see §6): four items must *replace or
augment* an existing mechanism, never add a parallel knob.

## 4. What code validation changed about the roadmap

Three deep code-validation passes (reporting, CLI/tags, authoring) reshaped the outside-in
list. Six meta-findings:

1. **"Surface, don't build" is confirmed at file:line.** The sans-IO core already computes
   and records the data behind most items: `attempts`/`duration_ms`/`detail` on
   `StepFinished` (`proef-core/src/event.rs`), the winning `macro_name` per bound step
   (`bind.rs:20`), deterministic seeded fakes, and hurl's own `curl_cmd` (already returned
   by `EntryResult`, currently discarded in engine-hurl `session.rs`).

2. **Two items are already ~90% built — validation downgraded them.**
   - **#14 seeded fakes:** fakes are already deterministic (hand-rolled SplitMix64, *no
     `rand` crate*, `fake.rs`) and **already seeded by the injected `run_id`, which is
     already recorded** in `RunStarted`. The feature collapses to "let `test` pin `run_id`
     the way `artifacts --run-id` already can."
   - **#9 stub-gen:** the "did you mean" fuzzy suggestion **already exists** (`bind.rs:214`,
     levenshtein); only the paste-ready stub template is missing.

3. **One item's premise is broken — #13 impacted-only re-run.** There is **no stored
   content hash anywhere**; ADR-0010 is enforced as a byte-identity `assert_eq!` on
   *outputs* (`crates/proef-cli/tests/execute.rs:188`), not a reusable digest. The only
   honest impact fingerprint — the emitted `.hurl` — is deliberately **not stable
   run-to-run** (`run_id` lives in runtime globals). #13 is gated behind a determinism
   prerequisite.

4. **Two plumbing gaps gate a cluster.** Scenario **tags stop at the CLI edge** — they
   never reach the runner (`ScenarioSpec`/`ScenarioOutcome` in `runner.rs` carry no tags)
   or the event stream — so **#15 (quarantine)** and part of **#8 (rerun)** need new
   plumbing. And **#4 (boolean tags)** is hard-blocked by `value_delimiter=','` on `--tags`
   (`main.rs:57`); it is a contract-changing *replace*, not an add.

5. **The recurring architectural rule is one-canonical-way.** Four items (#4, #9, #14, #15)
   each risk a *second* mechanism; each must replace or augment an existing one.

6. **The exit-code contract (ADR-0009) is a live wire for #15.** Quarantine changes *which*
   scenarios feed `RunSummary::exit_code` (fine — the fold stays pure in core) but must
   extend the pinned assert_cmd tests and **never let a quarantined *system* fault mask
   exit 3**.

## 5. The validated roadmap (master table)

Verdict legend: ✅ FITS · ⚠️ NEEDS-ADAPTATION · 🚫 premise broken. Effort: S ≤ ~1 day ·
M ~days · L ~weeks.

| # | Item | Verdict | Lives in | Effort | Architectural truth (as of 2026-07-31) |
|---|------|---------|----------|--------|----------------------------------------|
| 2 | Assertion cookbook (surface hurl-8.0 predicates/filters) | ✅ docs-only | `docs/` | S | Lowering copies all but `${…}` verbatim (`resolve.rs:163`); load runs the real `hurl_core` parser (`engine-hurl/src/lib.rs:95`). Predicates already work. Caveat: *grammar* validation, not JSONPath *semantics*. |
| 1 | GitHub `::error file=,line=,title=` annotations | ✅ | proef-cli `ci_reports.rs` | S | `file+line+detail` already flow to `write_github_summary` (`ci_reports.rs:98`). Gate stdout vs `--output json`; percent-encode multiline `detail`. Line-only (no byte-span at runtime). |
| 5 | `--curl` export per request | ✅ | engine-hurl `session.rs` | S | hurl's `EntryResult.curl_cmd` is already returned, just discarded (`session.rs:346`). **Must redact** (holds resolved secrets, ADR-0005). Fold into the existing `reproduce:` block (`exec.rs:230`). |
| 3a | "passed on attempt N" badge (JUnit/summary) | ✅ | proef-cli `ci_reports.rs` | S | `attempts:u32` already on `StepFinished` (`event.rs:72`) + `StepOutcome`; JUnit ignores it today (`ci_reports.rs:43`). |
| 9 | Stub-gen for unbound steps | ⚠️ | proef-core `bind.rs` | S | Augment the existing did-you-mean help (`bind.rs:89`), zero-match arm only — **not** a new command. Derive `{param}` from quoted tokens (matcher already sheds quotes, `matcher.rs:85`). |
| 10 | SARIF export of `--dry-run` diagnostics | ✅ | proef-cli new `sarif.rs` | S–M | `Diag` (`diag.rs:56`) → SARIF result ~1:1: `code`→ruleId, byte `span`→`region.byteOffset`. Pre-populate `rules[]` from the closed diagnostic-code set. A parallel serializer to `render.rs`. |
| 14 | `--seed` (reproducible fakes) | ⚠️ | proef-cli `main.rs`/`exec.rs` | S | Thread into `front::run`'s existing `run_id` param (`artifacts` already exposes `--run-id`, `main.rs:101`). Caveats: arbitrary seed breaks JUnit's UUID parse (`ci_reports.rs:22`); occurrence is **per-block, not per-run** (`resolve.rs:70`) — identical `${fake:X}` in two steps = same value. |
| 7 | Dead-macro / usage report | ✅ | proef-cli new `macros --usage` | S–M | `BoundStep.macro_name` (`bind.rs:20`) vs `packs.macros`. Count `use:`-only macros (`pattern:None`, `pack/mod.rs:165`) as reachable via the `use:` graph. Report the whole corpus, not a `--tags` subset. |
| 6 | Self-contained HTML report | ✅ | core `render_html(&[Event])` + cli write | M | Post-hoc `proef report --html <run-id>` replaying `events.jsonl` like `explain` (`explain.rs:12`). Bodies live in `artifacts/` — deep-link, don't inline. Derived view, never a second record (ADR-0008). |
| 12 | `proef diff` between two runs | ⚠️ | proef-cli `diff.rs` | M | Identity `(file,scenario)` (why ADR-0008 added `file`, `event.rs:86`); key step diffs on **`text` not `line`** (lines shift on edit). `attempts`+`duration_ms` → free flakiness/perf-regression detector. Pre-`file` records replay `file=""`. |
| 4 | Boolean tag expressions (`@a and not @b`) | ⚠️ (replace) | grammar in **core**, apply in cli `front.rs` | M | `value_delimiter=','` (`main.rs:57`) actively breaks `and/or/not`; must **replace** the CSV/OR contract (`front.rs:388`), keep empty-match=exit-2 (`front.rs:382`). Grammar/evaluator is deterministic → proptest/fuzz-shaped, belongs in core. |
| 8 | Rerun-only-failures (`--rerun`) | ⚠️ | proef-cli, reuse `explain` replay | M | `explain` already reads the latest record + failed `(file,name)` (`explain.rs:100`, `event.rs:83`). Needs a **multi-identity** predicate (today's `scenario`/`scenario-file` filters are single-valued, `exec.rs:301,321`). Factor a shared `record::failed_scenarios`. |
| 15 | `@quarantine` non-gating tag | ⚠️ (contract) | thread `gating:bool` core+cli | M | Tags must first reach `ScenarioSpec`/`ScenarioOutcome` (P1). `exit_code()` stays pure in core (`runner.rs:89`) and skips non-gating outcomes. Events still emit the scenario → **not hidden**. Extend the pinned assert_cmd tests; **never mask a `Fault::System` (exit 3)**. |
| 3b | True `<flakyFailure>` with earlier-attempt detail | ⚠️ | schema + engine-hurl | M | Needs an *additive* `attempt_details` field (ADR-0008 additive-only) + engine-hurl collecting per-retry bodies before the final one. Bigger than 3a. |
| 11 | `proef lsp` (feature/pack language server) | ✅ | new `proef-lsp` crate | L | All diagnostic substrate is headless/sans-IO already (`bind`, `pack::load`, `resolve` Probe mode, `matcher`). New: a sync `lsp-server` (tokio ban forbids async), a byte-offset→token API (not exposed), and a partial-results wrapper (bind/load are all-or-nothing today). Karate notably lacks good IDE support → differentiator. |
| 13 | Impacted-only re-run (content-hash) | 🚫 | — (gated) | L | No input hash exists; raw-input hashing is unsound (shared packs, `use:` nesting, config vars fan out). Honest fingerprint = per-scenario emitted `.hurl`, but it is not run-to-run stable (`run_id` in globals). Needs a determinism prerequisite first; silent-green risk. |

## 6. Prerequisites that unlock clusters

- **P1 — carry scenario tags + a `gating` flag past the CLI edge** into `ScenarioSpec` /
  `ScenarioOutcome` (`runner.rs:30,121`) and, additively, the event stream. Today tags die
  at `front.rs` (`bind.rs:35` → `lowered.tags`, no further). **Unlocks #15; simplifies #8.**
- **P2 — a shared `record::failed_scenarios(run_id)` + a multi-identity scenario
  predicate.** Reused by `explain`, `--rerun` (#8), and `proef diff` (#12).
- **P3 — a deterministic emitted-`.hurl` fingerprint** (stable run-to-run despite
  `run_id`). **Prerequisite for #13**; do not attempt #13 without it.

## 7. The one-canonical-way watch-list

Each of these must fold into an existing mechanism, never ship beside it:

| Item | Must replace / augment (not duplicate) |
|---|---|
| #4 boolean tags | **Replace** the CSV/OR `--tags` semantics — no second tag syntax |
| #9 stub-gen | **Augment** the existing did-you-mean `Diag.help` — no separate `proef stub` command |
| #14 `--seed` | Fold into the **existing `run_id`** determinism knob — no parallel seed unless it *replaces* run_id-keyed fakes |
| #15 quarantine | Exactly **one** non-gating tag name; must not spawn a second "skip" concept |
| #5 `--curl` | Attach to the **single** `reproduce:` mechanism, not a parallel debug path |

## 8. Recommended sequencing

1. **Batch A — free / small, all FITS, all reuse existing data.**
   #2 cookbook (docs) → #1 annotations → #5 `--curl` → #3a attempt badge → #9 stub.
2. **Batch B — small, high-leverage.** #10 SARIF · #7 dead-macro · #14 `--seed`.
3. **Prereqs → Batch C — medium, now unblocked.** Build **P1**+**P2**, then #6 HTML ·
   #12 diff · #8 rerun · #15 quarantine · #4 boolean tags · #3b flaky-detail.
4. **Batch D — strategic.** #11 LSP. And #13 **only** after committing to **P3**.

## 9. Per-item detail & competitor provenance

Each entry: *what it borrows from whom → the validated architectural note.* Numbers cross-
reference §5.

**#2 Assertion cookbook** — *from Karate's fuzzy markers + hurl's own docs.* Confirmed a
docs task: `resolve()` leaves everything but `${…}` byte-for-byte (`resolve.rs:163-208`,
test `runtime_tier_passes_through`), and pack load validates the full grammar via
`hurl_core::parser::parse_hurl_file` (`engine-hurl/src/lib.rs:95`), re-checked on the
emitted artifact (`front.rs:159`). *Extension:* ship a `tests/features/` reference feature
exercising each predicate so the cookbook is snapshot-locked against hurl upgrades (the
canary catches drift).

**#1 GitHub annotations** — *from the 2025–2026 CI-reporting shift (annotations displace
log-diving).* The failures loop already prints `` `{file}:{line}` — {detail} ``
(`ci_reports.rs:98`). Emit `::error` workflow commands as a sibling; `title` = scenario +
step text. *Risk:* stdout is owned by `--output json` (`exec.rs:114`) — gate it.

**#5 `--curl` export** — *from hurl's loved `--curl`; Bruno/Postman "copy as curl".* hurl
hands us `EntryResult.curl_cmd` already (session iterates `result.entries` at
`session.rs:346` but reads only captures/errors/duration). *Must* pass `Redactions`
(`session.rs:396`) before any sink — the curl line contains resolved secrets. Cannot be
derived pre-execution (needs runtime `{{…}}`).

**#3a "passed on attempt N"** — *from the flaky-test-honesty consensus (never hide a
retry).* `attempts` is first-class (`event.rs:72`, `step.rs:149`) and already printed on
the console (`report.rs:220`); JUnit simply drops it. Count-based badge is S.

**#9 Stub-gen** — *from Cucumber/Behave snippet generation.* The matcher already computes
the nearest macro via `closest_pattern`/`levenshtein` (`bind.rs:214`, `matcher.rs:248`);
add a `match:`+`hurl: |` skeleton to the help text for the **zero-match** arm only (an
ambiguous step, `bind.rs:99`, must not get a stub).

**#10 SARIF** — *from SARIF's rise for static/validation findings inline in PRs.* Dry-run
diags are a structured `Vec<Diag>` before miette (`diag.rs:123`, `front.rs:64`). `Diag`
maps ~1:1 to a SARIF result; the closed code set (one per `tests/errors/` dir) pre-fills
`rules[]`. cli-edge serializer, no core change.

**#14 `--seed`** — *from seeded-faker reproducibility.* Fakes already deterministic
(`fake.rs:12` SplitMix64/FNV, seeded `fnv1a(run_id) ^ …`), seed already recorded in
`RunStarted{run_id}` (`event.rs:26`). *Design fork:* alias `run_id` (zero core change, but
must stay uuid-parseable for JUnit) **vs** a dedicated recorded `seed` field (cleaner, but
a second knob — resolve per one-canonical-way). *Known limit:* `resolution.fakes` resets
per `resolve()` call (`resolve.rs:70,314`) → occurrence is per-block; cross-step uniqueness
does not hold. Document before advertising "unique fakes".

**#7 Dead-macro report** — *from Cucumber's `usage` formatter marking `UNUSED`.* Binding
records `BoundStep.macro_name` (`bind.rs:192`); iterate
`front.features[].scenarios[].bound.steps[].macro_name` vs `packs.macros.keys()`
(`pack/mod.rs:116`). `use:`-only macros (`pattern:None`) need reachability via the `use:`
graph (`pack/validate.rs:628`) to avoid false "unused". Report the whole corpus.

**#6 HTML report** — *from Cucumber/Karate/hurl HTML reports; the industry's convergence on
the Cucumber-Messages/JSONL stream.* Core `render_html(&[Event]) -> String`, cli writes;
best as post-hoc over `events.jsonl` (`explain.rs` already replays it) so historical runs
render. Events are pre-redacted at the sink (`report.rs:124`).

**#12 `proef diff`** — *from Allure history / test-observability-without-OTel.* Identity is
`(file, scenario)` (`report.rs:147` `ScenarioKey`; ADR-0008 added `file` for exactly this).
Key step diffs on `text`, not the volatile `line`. `attempts`+`duration_ms` make it a
flakiness/perf-regression detector. run_id is uuid-v7 → chronology recoverable.

**#4 Boolean tags** — *from Cucumber tag expressions (`and/or/not/()`).* Single filter fn
`tag_selected` (`front.rs:388`), three callers. `value_delimiter=','` (`main.rs:57`) blocks
the operator syntax → drop it, take one expression string, **replace** the CSV contract.
Grammar/evaluator → core (deterministic, fuzz-shaped). Preserve empty-match=exit-2.

**#8 Rerun-only-failures** — *from Cucumber's `rerun` formatter (`@rerun.txt`).* `explain`
already discovers + replays the latest record and extracts failed identities
(`explain.rs:63`). Add a multi-identity predicate reusing `build_specs` (`exec.rs:289`).
Empty failure set → reuse `no_scenarios_matched` (exit 2), never silent-pass.

**#15 Quarantine** — *from the flaky-quarantine-with-owner+expiry consensus.* Thread
`gating:bool` from CLI (which sees `scenario.lowered.tags`, `exec.rs:318`) into
`ScenarioSpec`/`ScenarioOutcome`; `exit_code()` (`runner.rs:89`) skips non-gating outcomes
and **stays pure in core**. Events unchanged → scenario still reported. Extend the
`cli.rs`/`execute.rs` exit-code assertions; a quarantined `Fault::System` still exits 3.

**#3b Flaky-failure detail** — *from JUnit `<flakyFailure>` / Allure retries.* Needs an
additive `attempt_details` on `StepFinished` (ADR-0008 additive-only) and engine-hurl
collecting per-retry messages (hurl retry is per-entry internal — verify the adapter isn't
already discarding earlier bodies).

**#11 `proef lsp`** — *from Cucumber's language server (unbound-step diagnostics,
go-to-def, completion); a gap Karate never closed.* Reuses `feature::parse`, `bind`,
`pack::load`, `resolve` Probe mode, `matcher` — all headless, all with stable codes +
byte-offset spans that already map to editor ranges. New work: sync `lsp-server` (tokio
banned), byte→token API, and a "collect diags, don't early-return" wrapper (bind/load are
all-or-nothing today).

**#13 Impacted-only re-run** — *from selective/affected-test re-run.* Premise broken: no
reusable input hash (ADR-0010 is a byte-identity `assert_eq!` on outputs,
`execute.rs:188`), and the honest fingerprint (emitted `.hurl`) is not run-to-run stable
because runtime globals include `run_id` (`execute.rs:304`). Gated on P3; a hash miss must
never skip a scenario that would fail (needs `--force`/first-run fallback).

## 10. Sources (competitive research, 2026-07-31)

- Karate — match keyword / fuzzy markers / reuse: <https://docs.karatelabs.io/assertions/match-keyword/>, <https://docs.karatelabs.io/reusability/calling-features/>
- Cucumber-JS formatters (usage / rerun / snippets / html): <https://github.com/cucumber/cucumber-js/blob/main/docs/formatters.md>
- Reqnroll HTML report + Cucumber Messages; SpecFlow EOL: <https://reqnroll.net/news/2025/06/roadmap-update-html-report/>, <https://reqnroll.net/news/2025/01/specflow-end-of-life-has-been-announced/>
- Hurl 8.0 (RFC 9535 JSONPath, `--curl`, TAP, secrets redaction) + release history: <https://hurl.dev/blog/2026/04/27/announcing-hurl-8.0.0.html>, <https://hurl.dev/docs/filters.html>
- Bruno vs Postman (git-native trajectory): <https://www.usebruno.com/compare/bruno-vs-postman>
- GitHub Actions workflow commands (annotations + job summaries): <https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-commands>
- Flaky-test quarantine/hardening; GitHub Actions OIDC/masking limits; Allure history: <https://pie.inc/blog/flaky-tests-cicd/>, <https://www.stepsecurity.io/blog/github-actions-security-best-practices>, <https://allurereport.org/docs/how-it-works-history-files/>
- Cucumber Language Server: <https://github.com/cucumber/language-server>
