# proef — open findings

**This is the worklist.** Every open defect and gap lives here, whichever review found
it. Each entry is self-contained: the evidence, the reasoning, and — where something was
declined — why.

**Companion:** [IMPROVEMENT-PLAN](IMPROVEMENT-PLAN.md) is the *feature* roadmap (own
numbering, 13 of 16 shipped) and stays separate because five ADRs cite it by section
number. [CHANGELOG](CHANGELOG.md) records what shipped, per release.

**Provenance.** Three reviews fed this list, each validated claim-by-claim against the
tree and then retired into it:

| Review | Scope | Contributed |
|---|---|---|
| v0.5.3 external (2026-08-06) | 40 claims → 38 confirmed, 1 partial, 1 already fixed | the A/B/P/Q items below |
| first-run UX (0.5.3, engineer's first 30 min) | F1–F4 | **R1–R2** |
| non-technical UX (0.8.0, PRD §4 P1 calibration) | N1–N5 | **R3** |
| round-7 pre-merge review of PR #13 | 4 defects + residue | **§2.1–§2.4 below**; three shipped in #31 |
| corpus-port report (0.8.0, a real 844-line hurl suite ported) | 12 items → 3 shipped (#41, #43), 2 premises corrected | **M/E/D items below** |

The review documents themselves were removed once their open items landed here; their
full text, transcripts and citations are in git history (`git log --diff-filter=D
-- docs/FIRST-RUN-UX-REVIEW.md docs/NON-TECHNICAL-UX-REVIEW.md`). The shipped/open split
was re-checked against `main` on **2026-08-10**.

**Read the citations as "start reading here", not as addresses.** They were accurate on
2026-08-06 and files have moved since; locate symbols with `rg`, not line numbers.

---

## Ingested — the 2026-09-02 survey (internal), validated then implemented

A deliberate check-the-world round: repo state against upstream releases,
standards movement, and the open list itself. Recorded like every external
round so its verdicts are not re-derived.

**Validated as needing nothing — do not re-raise without new evidence:**

- **The hurl pin is current.** 8.0.1 *is* the latest upstream stable
  (2026-04-28); the canary covers the next one.
- **The Rust pin is correct per the written policy.** 1.98.0 landed
  2026-08-20; no 1.98.1 exists yet, and policy adopts at `x.y.1` — a calendar
  item (~mid-September 2026), not a drift.
- **`notify` 9.0 is still a release candidate** (rc.4, 2026-05); `=8.2.0`
  stands.
- **Release engineering already ships the modern supply-chain story** —
  Sigstore attestations (`attest-build-provenance@v4`), `.sha256` sidecars,
  Homebrew tap, binstall metadata, SHA-pinned actions gated by pinned zizmor.
  The survey's own candidate ("add attestations") died against the tree.
- **Competitor movement is OpenAPI-generative testing** (Schemathesis et
  al.) — a different product shape (generated negative tests vs. declared
  business scenarios); no charter-fit gap. The hurl-fidelity niche is
  uncontested.

**Shipped from the survey (this series):** `[http] cookie-store = false`
(hurl 8.0's env-shaped option; the one `[http]` key with no per-entry
spelling at all); `--ctrf` (CTRF report off the JUnit fold, quarantine
parity per ADR-0019, real `retryAttempts`); the mid-run console write
failure latch (the deferred v0.6–v0.8 item, to its own written design);
`emit::feature_stem`/`emit::artifact_slug` closing Q6 structurally; Q2
re-verdicted closed (the #146 cache had already closed it).

**Still open from the survey, dispositions unchanged:** `[source-links]`
(*build* verdict of 2026-09-01, unscheduled); P13 (nightly `llvm-cov` job —
`cargo-llvm-cov` is already in the documented toolchain); the text-scan
honesty bundle (capture-name charset / ≤2-char methods / `key_line_spans`
flag — see the deferred list); P12 (measure first, alone, per the
complexity-guard lesson). Decision items untouched: E2's split-invocation
remainder (trigger not fired), E3 (wants an ADR), R1 (wants its own spec),
the shipped-changelog duplicate headers (maintainer's call).

---

## Shipped since validation

Kept here so the list reads as live rather than stale, and so a finding is not
re-reported after it is fixed.

| ID | Finding | Shipped in |
|---|---|---|
| P10 | Abandoned-scenario events appended after `RunFinished` — worse than reported: past the *run's* terminal event, not just the scenario's | #15 |
| B11 | `${fake:*}` collided across a scenario's steps | #15 |
| P9 | `.map.json` gained phantom capture rows (fence-unaware scan, unrecognised custom methods) | #15 |
| B1 | Whitespace-only `expect:` produced an inverted sidecar span `[9,8]` | #15 |
| P2 | Non-UTF-8 `PROEF_KEY`/`PROEF_ENV`/`PROEF_SECRET_<NAME>` read as *absent* | #18 |
| P6 | Full disk: `--output json` exited 0 with truncated JSON | #18 |
| P7 | No stdout-side pipe-close test (both existing ones closed stderr) | #18 |
| P1 | Tee re-wrote the full slice on every `write_all` retry, duplicating `run.log` tail bytes | #18 |
| P8 | `proef fmt` rewrote CRLF → LF wholesale | #18 |
| Q3 | `report -o` outside the run dir shipped dead relative artifact hrefs | #18 |
| B8 | `diff` flagged a brand-new retried step as flaky | #18 |
| A3 | `CLAUDE.md` status stopped at post-M5 | #21 |
| N1 | First run reported `system error` with no explanation (NON-TECHNICAL) | #24 |
| N2 | `proef macros` printed identifiers, never the `match:` sentence | #24 |
| N3 | `macros` refused to list when any step failed to bind | #24 |
| N4 | `unbound_step`'s help led with the pack maintainer's action | #24 |
| N5 | No document described the scenario author's workflow | #24 |
| §8 | `init` announced four files and reported five | #24 |
| Q5 | Ctrl-C skipped teardown silently — cleanup never ran, nothing said so | #26 |
| Q4 | `--dry-run` validated neither `[run] setup` nor `[run] teardown` | #26 |
| R2 | `doctor` did not report a missing pack schema (FIRST-RUN F4b's second half) | #29 |
| B7 | `secret set --value` put the secret in argv, and the error text steered to it | #29 |
| §2.2 | `init` destroyed an authored `proef-pack.schema.json` (round 7) | #30 |
| §2.3 | a mixed suite+phase failure lost the phase label exactly when it disambiguated | #31 |
| §2.4 | `--rerun` after a phase-only failure blamed filters never passed | #31 |
| §2.1 | pre-0.6.0 records reported the wrong verdict with confidence | #31 |
| — | `diff` counted a failing teardown as a test regression | #31 |
| P4 | `fmt` homogenized mixed-endings files beyond its hurl-blocks-only promise | #33 |
| P4 | `proef --help` described `macros` with pre-prose wording | #33 |
| P4 | `WRITING-SCENARIOS`' two sample outputs drifted from the binary | #33 |
| — | `init` destroyed an authored `proef-pack.schema.json` (round-7 §2.2) | #30 |
| Q7 | `fuzz_tag_expr` compiled but was in neither fuzz loop | #30 |
| B3 | `windows.yml` built and tested without `--locked` | #30 |
| B13 | `justfile` gate list omitted `public-api` (and the fuzz gate) | #30 |
| B5 | `explain`/`diff`/`report` each inlined `ProjectConfig::load()` | #30 |
| A6 | TROUBLESHOOTING's exit table omitted `130` | #30 |
| A4 | README's ADR range and flag rows, and TECH-SPEC §10's command surface, were stale | #30, #34 |
| A5 | TECH-SPEC's `publish` claim and its run-dir inventory were stale | #30 |
| A1 | `EDITORS.md` claimed go-to-definition cannot land on a `match:` line | #34 |
| A7 | `GETTING-STARTED`'s copy of the scaffold comment had a word the scaffold does not | #34 |
| P11 | ADR-0015 described a `worker` on `ScenarioFinished` that is always `None` | #34 |
| B2 | a templated `retry:`/`delay:` under-counted the batch budget, abandoning healthy scenarios | #35 |
| B4 | `--output json`'s `exit_code` disagreed with the real exit after a JUnit failure | #35 |
| B6 | LSP completion snippets did not escape `$`/`}`/`\` | #36 |
| B9 | GitHub annotation `file=` and job-summary table cells were unescaped | #36 |
| — | the `--dry-run` nudge echoed a command that was not the run validated (round-7) | #37 |
| P3 | `--sarif` emitted no `startLine`, so it annotated nothing | #37 |
| P5 | `--watch` did not retrigger on `proef.toml` | #37 |
| — | a run against untouched scaffold *routes* got no coaching (round-8 §5) | #38 |
| — | truncated-record fallback totals dropped `Warned` scenarios (round-7) | #39 |
| — | `fmt` rewrote any file handed to it, not just a pack | #40 |
| — | `fmt` trimmed the YAML skeleton, turning `--check` red outside its scope | #40 |
| C1 | negative-case authoring had no signposted catalogue form | #43 |
| C3 | `expect:` composition documented as a mechanism, never shown as the pattern | #43 |
| R9-1 | no `proef fragments` listing — neither way a fragment dies had a denominator | (branch) |
| §2.1 | a `bind:` key nothing reads passed silently — the one authoring mistake with no signal | (branch) |
| §2.2 | `duplicate_fragment` said "in both `x` and `x`" and offered a remedy that cannot work | (branch) |
| §2.3 | `unbound_placeholder` named two of ADR-0018's three supply routes | (branch) |
| §3.1 | `doctor` did not know fragments exist — a path error surfaced as a name error | (branch) |
| §3.2 | config discovery searches only up, undocumented; no way to name the file | (branch) |
| §3.3 | `init` scaffolded only `hurl: |`, so `ref:` was invisible to the persona built for it | (branch) |
| — | ADR-0007 value caps never crossed to fragments: `retry: -1` validated clean | (branch) |

**Q7 is now closed** (#30): `fuzz_tag_expr` is in both fuzz loops as well as the
compile gate.

---

## Ingested — round 19 (2026-08-31), validated claim-by-claim

Five confirmed defects, all shipped; five checks that cleared; four external
triggers re-tested. The round's shape: the heavily-audited paths (scheduler,
record gate, outline expansion, the shard×shuffle×rerun composition) were
probed and found correctly defended, so the yield came from **what the output
surfaces contain** rather than from what the core computes.

### R19-1 — a step's `name:` reached the artifact and nothing else *(shipped)*

A macro with more than one step turns one feature sentence into several engine
steps sharing a `StepRef` exactly. The emitter always wrote the authored
`name:` into the artifact's entry comment; `StepRef` never carried it, so the
console, HTML report, `JUnit`, TAP, the job summary and `explain` printed the
same sentence once per step with only the status glyph between a warning and
the failure beside it. In a fresh reference run, **17 of 44 step identities
were duplicates**, and the pinned event snapshot was *encoding* the defect —
three byte-identical `step_finished` for `the cookie session is exercised`.

Fixed by mirroring `fragment` (`StepOutcome` + `step_finished`), not by
extending `StepRef`: several engine steps share one `StepRef`, so the label
belongs to the engine step. Additive on the wire; schema stays 1. Retires two
untrue claims — `AUTHORING.md`'s "they anchor artifacts, events, and failure
output" and `LoweredStep::label`'s own "(events/console)". Same class as
`reproduce_hint` in the R18 wave: computed all along, printed all along,
dropped by the record.

### R19-2 — `report -o` wrote the machine into the shared file *(shipped)*

Absolute artifact hrefs, 12 per report, naming the author's home directory —
in the one output built to be uploaded. 0.13.0 scrubbed machine identity from
the *record* (R12-1) and the record is clean; the HTML put it back. The
absolute path was deliberate and pinned by a test, but it resolves only on the
machine that produced it, which is exactly where `-o` output is not read. A
relative href strictly dominates. **Windows CI then caught a second half the
local gate could not**: the href was built with `Path::display`, and `\` is
not a separator in a URL, so a Windows-generated report's links were dead
either way — it is now built from components joined with `/`. The known
macOS-only-gate hazard, paid again.

### R19-4 — one palette token failed WCAG AA, and every dark pill did *(shipped)*

`--skip` was the single token the dark block does not redefine: a grey chosen
against `#0d1117` left carrying white text on white at 3.45:1. Writing the
*guard* rather than the fix found the larger one — `.pill` painted `color:#fff`
on status colours the dark palette tunes as text on a dark ground, so all four
dark pills sat between 2.52:1 and 3.45:1. The pill foreground is a token now.
Tests assert the *ratio*, not the hex, and that both palettes define the same
token set (the absence that caused it).

### R19-5 — the report had one heading and no outline *(shipped, narrower than filed)*

Filed as "no headings at all"; **the timeline already had an `<h2>`** — the
first inventory ran against a record with no timing, so the timeline never
rendered. Corrected before implementing: only the tag table and the scenario
list lacked one. Both gained one, sharing the class the timeline already used.

### R19-3 — the three post-run commands had no machine output *(shipped)*

`explain`, `diff` and `doctor`. A run directory carries no structured summary,
so anything analysing a run it did not launch had to fold `events.jsonl`
itself — the fold proef's own two copies disagreed on three ways. Each object
mirrors its prose field for field; `doctor` had to start collecting its checks
before rendering them, so JSON is a second rendering rather than a second walk.

### Cleared — checked, not defects (do not re-raise as omissions)

- `RecordGate`'s `(file, name)` identity is safe: `feature.rs`'s `dedup_names`
  guarantees uniqueness feature-wide and its doc names this consumer.
- Duplicate step rows are not a counting bug — the record keys steps by
  `(text, occurrence ordinal)`. Only the surfaces were blind (R19-1).
- Report keyboard focus is intact: no `:focus` rules, but no `outline:none`
  either, so native rings survive on button/anchor/summary.
- The tag table is a real `<table>`; it renders no rows only when a run
  carries no tags.
- crates.io showing no `homepage` for 0.14.0 is publish lag — the field landed
  after that release was cut (verified by ancestry), and appears on the next
  publish. `documentation` is still unset: for a binary crate that falls back
  to a docs.rs library page rather than the book, worth setting deliberately.

### External triggers re-tested 2026-08-31 — all four hold

- **OpenTelemetry export stays a non-goal.** OTel graduated CNCF (2026-05), so
  the umbrella argument weakened, but the attributes that would carry a test
  run — `test.case.name`, `test.case.result.status`, `test.suite.name`,
  `test.suite.run.status` — are **all still Development** stability. PRD §3's
  stated reason is current as written; only the re-check date moves.
- **CTRF stays declined.** Still community-adoption phase; Microsoft's test
  platform has a discussion issue, not an implementation. Trigger unfired.
- **Both sacred pins are correct.** hurl 8.0.1 is the latest release
  (2026-04-29) — no 8.1, no 9.0. Rust 1.97.1 is right under the written
  policy: stable is 1.98.0 (2026-08-18) and `channel-rust-1.98.1.toml` 404s,
  so the point release the policy waits for does not exist yet. R18-2's
  refutation survives contact with the calendar.
- **An MCP server is declined, with a named trigger.** The largest ecosystem
  shift since the last research round — Playwright, Cypress, BrowserStack,
  Maestro and ReportPortal all ship one, and Claude Code / Cursor / Windsurf
  consume them natively. They shipped MCP because their primary surface is a
  GUI or a cloud API and an agent had no other way in. proef is CLI-first with
  `--format json` and a pinned four-code exit contract: an agent already has a
  complete interface, and a second one is a second way to do one thing. The
  only real gap an agent hit was R19-3, now closed. **Trigger:** a concrete
  agent workflow that `--format json` plus exit codes cannot express. Recorded
  so `proef lsp`'s precedent is not read as an open door.

### Noted, not filed

`source_guards`' malformed-plural scan matches the literal `(y)` anywhere in
a non-comment source line, so any code with a single-character `y` parameter
false-positives (`x.max(y)` did). The guard's intent is user-facing strings;
scanning all code is broader than that. Left alone — it is working as a guard
and tightening it to string literals is more risk than the trap is worth — but
the next author to trip it should know why.

## Ingested — the deep improvement report (2026-08-25), validated claim-by-claim

A twelve-stream self-audit plus competitive/ecosystem research (five code
audits, three UX audits, four research streams; ~125 findings), every
load-bearing claim re-verified against the tree before acceptance and three
proved empirically (measured stack-overflow abort and exponential
backtracking in the tag glob; observed nondeterministic gherkin error
ordering). The full report is the session artifact "proef — deep improvement
report"; this section records the verdicts and what remains open.

### Wave 1 — shipped (#112–#116)

- **#112** — a comment on a section header no longer blinds any scan
  (`[Options] # tuning` + `retry: -1` dry-ran clean — ADR-0007's named hole;
  `[Captures] # ids` dropped sidecar rows; `[Asserts] # note` doubled a
  section); the delay cap learned hurl's `h` unit (`delay: 5h` validated
  clean at 5× the cap); pack-scope `bind:` resolves arg-free instead of in
  whichever macro ran first; the tag glob is the two-pointer match
  (oracle-property-tested — the metachar branch previously had zero
  generated coverage); `multiline_bind` refuses `\r`/controls; the
  `expect:` merge shares the emitter's hardened response-line check.
- **#113** — a Ctrl-C in `--watch`'s debounce window no longer launches one
  more full run; a delivered watcher error or rescan burst (queue overflow)
  retriggers instead of leaving the watch permanently deaf. Punctured and
  re-closed 0.12.0's "staleness class closed for good" claim.
- **#114** — eleven silent-failure sites gained voices (store-poison save,
  suite walker, doctor/fmt over unreadable trees, non-UTF-8 env values,
  `.map.json`, LSP config, `flaky` degrade, `docs-check` vacuous pass,
  `Sinks` severity filter); `fmt` recognizes every literal-block spelling.
- **#115** — a travelling record can no longer lie (`scenario_finished.file
  = ""` key mismatch silently emptied every step map — `flaky`'s `Latent`
  verdict was unreachable and `diff --fail-on-regression` certified green),
  crash (256 MiB read ceiling; saturating sums; saturating `Span::len`),
  or steer (`rerun_of`/`--run-id` single-component validation; `[tag-links]`
  URL percent-encoding + http(s)-only in both sinks).
- **#116** — `saveAs: global` refuses a secret it can *find* (needle set,
  in core's `World`, every engine covered) rather than one it can *equal*
  (engine-side, raw values only); the invariant is now property-tested as
  CLAUDE.md had claimed. The SLA gate applies the same `@quarantine`
  non-gating list as the exit code.

### Waves 2–5 — shipped (#118–#128), audited 2026-08-31

**This section said these were open for a week after they landed.** The list
exists so a finding is not re-reported once it is fixed, and it failed at
exactly that: a re-read sent one round toward rebuilding wave 2, and repeated
two of its claims to a reader as open work. Corrected by checking the tree for
each item rather than trusting the entry.

| Wave | Shipped in | Spot-checked by |
|---|---|---|
| 2 — CI-sink conformance | #118 | `failure_detail_reaches_attribute_and_text_node_alike`, `illegal_bytes_and_ansi_never_reach_the_xml`, `an_oversized_summary_truncates_and_says_so`, `annotations_cap_at_ten_with_an_honest_notice`, `composed_identities_form_a_set`, `times_are_three_decimal_seconds` |
| 3 — UX | #119–#122 | console `is_terminal` colour, `clap_complete`/`clap_mangen` in the archives, `doctor`'s project block, the report's `jump` nav + `data-f` filter, the `--format` / `-o` split |
| 4 — diagnostics | #123–#125 | `suggest_or_enumerate`, `code_description`, `proef::config::*` codes, `match_span` in use |
| 5 — docs & distribution | #126–#128 | `docs/INSTALL.md`, `.sha256` sidecars, the README comparison |

Two wave items did **not** ship, and one of them should not:

- **`[[ATTACHMENT|path]]` in a testcase's `system-out` — declined, with a
  trigger.** It is a Jenkins-plugin convention: GitLab and GitHub ignore it, so
  it buys a link for one vendor's users who also installed the JUnit
  Attachments plugin. It would put a filesystem path inside an artifact built
  to travel — the class of defect R19-2 had just finished removing from the
  HTML report — and the reader's need is already met twice over, by the
  reproduce-hint `curl` in the failure content and by the HTML report's own
  artifact deep-links. **Trigger:** a user on Jenkins reporting that neither
  reaches the artifact for them.
- **`llms-full.txt`** — still unshipped, and the entry that proposed it already
  records that the SEO case for it is empirically dead. Left as-is.

### Corrections to this list's own claims *(all four were stale)*

- *"the exclusive-tags scheduler and `RecordGate` have no direct tests"* —
  **false.** `proef-core/tests/runner.rs` carries
  `an_exclusive_scenario_never_shares_the_pool`,
  `back_to_back_exclusive_scenarios_each_get_the_pool_alone`,
  `cancelling_during_an_exclusive_drain_still_completes_the_run`, and — for
  the gate — `abandoned_scenario_emits_nothing_after_run_finished`.
- *"`bake_entry_options` deserves a proptest"* — **it has one**, in `lower.rs`.
- *"the `--format`/`-o` split is open"* — shipped in #122.
- *"`match_span` is computed and unused"* — it is used; the diagnostics wave
  wired it.

### Verified against the tree (each entry says whether it is open or closed)

- ~~`--shard` balances by hash while the timing data to balance by duration is
  already retained.~~ **Closed (2026-09-02) by `--shard-weights`, after
  validation found the obvious design silently wrong.**

  `shard_bucket(file, name, count)` took identity only, so a 4-way split was
  balanced by count and not by time — and a CI matrix finishes when its slowest
  shard finishes. The weight now shipped is the **sum of a scenario's step
  durations** (`record::StepRun::duration_ms`), which measures work rather than
  queue wait; the wall-clock span would have been the wrong number and the
  record reader does not retain it anyway.

  **The hazard this entry existed to record.** The natural implementation —
  "weight by the latest record in `runs-dir`" — is silently incorrect for the
  only case sharding exists to serve. Each shard of a CI matrix runs on a
  **separate machine** with its own (usually empty) `runs-dir`, so every job
  would compute a *different* weight table and therefore a different assignment.
  Scenarios would run twice or not at all, and the suite would still report
  green. Nothing about that failure announces itself.

  Shipped shape: every run that reaches its suite writes a small `timings.json`
  into its run directory (an aborted setup has no suite to weigh, and writing
  its own scenarios would skew the next split with identities that never run),
  CI archives that one file, and each matrix job points `--shard-weights` at the
  same copy — so the split is a pure function of (selected scenarios, that
  file). Weighted scenarios are placed longest-first; unweighted ones fall back
  to the frozen hash, and the two rules **partition** rather than compete, so a
  test added after the timings were captured still runs exactly once. A
  three-way matrix test asserts set equality both ways; mutating placement by
  one bucket drops two scenarios and the test names them.

  What it gives up is what hash mode was chosen for — a balanced split is not
  stable under insertion — which is why the flag is opt-in. See `CONFIG.md`.

- ~~`lower.rs` threads the same mutable trio through twelve functions.~~
  **Closed (2026-09-02), and the premise it was filed under was wrong.**

  The original filing said "mechanical, no behaviour change: introduce a context
  struct and make them methods". That would have broken the code. The closures
  (`resolve_in`, `resolve_pack_scope`) take `refs` and `sinks` as *explicit
  parameters* rather than capturing them, precisely so they remain callable while
  other state is mutably borrowed — and a method on `&mut self` cannot be called
  while `self` is borrowed elsewhere. Threading was not an oversight; it was
  load-bearing, and validating that is what turned a rename into a design.

  Shipped: three bundles, each a type the code already implied —
  `Emit { out, refs, sinks }` (the mutable outputs, always passed together),
  `StepScope { step_ref, ctx, at }` (what stays fixed for one authored step
  however deep expansion recurses), and `Finished` for the four values
  describing a completed step. The threading discipline is unchanged; only the
  arity is. **Arity suppressions workspace-wide: 13 → 6, `lower.rs` at zero.**

- ~~Hurl grammar in `proef-core` vs ADR-0002's diff-empty claim.~~ **Closed
  (2026-09-01) by an ADR-0002 amendment plus a guard — and this entry was wrong
  three times over.** "~290 lines" counted `#[cfg(test)]` fixtures; the
  correction to "19 lines, all in `lower.rs`, four concerns" fixed the count and
  kept two errors. It is 19 lines across **three** files — `lower.rs`, `emit.rs`
  and `pack/validate.rs` — and the four "concerns" mostly are not concerns:
  `is_method_line`, `is_section_header`, `is_response_line` and `is_header_line`
  are already one canonical `pub(crate)` set that all three files share.

  What the entry missed entirely is the finding: **the seam already solved this
  once, on the reading side.** `StepKindSpec::options` exists, in its own words,
  as "the seam that keeps option *spellings* out of `proef-core`" — added
  because matching `"retry-interval:"` as a literal meant "one rule lived at two
  altitudes." It covers recognising options. The core still *writes* `retry:`,
  `retry-interval:`, `delay:` and `variable:` as literals, so the same rule
  still lives at two altitudes, in the other direction. That, not the line
  count, is the actual asymmetry.

  Resolved as: the thirteen-token vocabulary is **sanctioned and closed** on the
  ADR record, pinned by `source_guards::hurl_grammar_in_core_is_the_closed_set_the_adr_names`
  (which fails on growth *and* on an existing token spreading to another core
  module, and names both remedies in the failure). Moving the written half
  behind the seam is deferred with a named trigger — a second engine being
  scheduled — because until then it relocates seven literals that exactly one
  implementation will ever supply, at the cost of a public-API break.

  The meta-lesson, and the fifth instance of it this programme: **a claim that
  lives only in prose decays, and decays in whichever direction makes the
  writer's point.** Every wrong version of this entry overstated the problem.
- ~~Reading `events.jsonl` is spread across seven files.~~ **Closed (#150),
  and it was not the duplication it was filed as.** Chasing it found that the
  256 MiB record ceiling reached two of its four readers: `explain` and
  `report` each opened the file with a bare `read_to_string`, so neither had
  it — `report` even used the guarded reader for the *base* record two dozen
  lines below the raw read of the primary one. Both go through
  `record::read_events` now, and a source scan in `source_guards` makes the
  next reader use the same door. The folds that could disagree were already
  unified (`record::parse_record`, `report::suite_totals`), and
  `explain --format json` hands consumers the canonical answer rather than
  inviting an eighth reader.
- **`captures_before` is O(steps²)** — deliberately left. It runs only when a
  `ref:`/`bind:` consumes it and is bounded by scenario size, so threading a
  running set through lowering is churn against a bound that is not tight.
- ~~Redaction runs inside the reporter mutex.~~ **Fixed (#149).** The
  allocation cost had already been addressed (the miss path no longer
  allocates, and a clean field keeps its `Arc`); the structural point stood
  until the masking simply moved *above* the `lock()`. It reads the event and
  the needle set and writes neither, so it never needed the lock at all.

### Analysed 2026-09-01 — none of the three was an ADR question

This section carried three items as charter questions needing a new or
amended ADR. Checked against the tree, **none of them needs one**, and two
had the wrong governing principle attached. Each entry below states the
verdict and the evidence; the decision to act is a one-word answer, not a
design exercise.

- **GitHub-summary permalinks to failing lines** — *build it; ADR-0020 is
  untouched.* The entry assumed the commit must come from `GITHUB_SHA`,
  which §1 forbids by name. It need not, on two counts. First, a link to
  the failing line **already ships**: `github_annotations`
  (`ci_reports.rs:363`, live at `exec.rs:1444`) emits `::error
  file=…,line=…::` per failing scenario, and `--sarif` (`sarif.rs`) carries
  `startLine`. GitHub resolves both against the commit the job checked out —
  proef never reads a SHA to make that work. The residual gap is narrow: the
  *job-summary tables* are inert text where the annotations are linked.
  Second, closing that gap needs only two mechanisms that are already
  accepted and already shipped — `[tag-links]` (`config.rs`, glob → URL
  template with `{tag}` substituted, applied by the HTML tag table and the
  GitHub summary, documented as *"base config only — a link is a project
  fact, not an environment one"*), and ADR-0020 §1's own worked example,
  `--meta commit=$(git rev-parse HEAD)`. A `[source-links]` table over
  `{file}`/`{line}`/`{commit}`, with the commit **handed over** as metadata,
  sits inside both rules unchanged. §1 forbids proef *harvesting* the
  variable; it does not forbid a user handing it over — that is precisely
  the distinction the ADR was written to draw. It also serves GitLab,
  Bitbucket and self-hosted forges, which a `GITHUB_SHA` read never would.

- **Chrome-trace export of the scheduling timeline** — *decline; the filed
  reason is false and the conclusion survives on a different one.* "A second
  rendering of what the HTML timeline already shows" is wrong: `render_timeline`
  (`html.rs`) draws one bar per **scenario** per worker lane, while a trace's
  whole value is step-level nesting and zoom, which the report does not have.
  (Cited without a line number on purpose — the first version of this entry
  named one, and adding the neighbouring `render_slowest` moved it.) The
  *adjacent* gap that entry implied — that the page could not say which
  scenarios cost the most — is closed separately by the report's ranked
  **Slowest** section; the trace question is unaffected, because that section
  ranks scenarios and a trace nests steps. The correct reason to decline is that the JSONL
  record already carries every step's start and end (ADR-0015 injected
  timestamps), so a trace is a short transform of data proef publishes in
  full — and a second export format for already-published data is what one
  canonical mechanism forbids. No consumer has asked, which is the same
  CTRF/TAP-14 discipline applied above. **Action:** document the conversion
  recipe instead of building an exporter. **Trigger:** someone who has run
  the transform and hit something it cannot express.

- **Templated report output paths** — *decline; the one-path rule was the
  wrong lens.* That rule governs the resolution **base** (a path in
  `proef.toml` resolves against the config's directory, a flag against the
  cwd); templating touches neither, so the two never conflicted. The
  governing principle is ADR-0020 §1's axis again: `-o
  "reports/$RUN_ID/index.html"` is shell interpolation the caller already
  controls, and asking proef to interpolate it is asking proef to own a value
  that is already handed over. Per-run *records* also already have their
  mechanism — `[run] runs-dir` plus `keep-runs` rotation (`config.rs:101-111`),
  deliberately project-wide so there is one record store and one policy — so a
  second per-run path scheme would be the duplicate, not the gap.

The pattern is now consistent enough to be worth stating: **an item filed as
a charter question is usually an item whose governing principle was guessed.**
The only-failed console sat here until #147 shipped it as `--console failed`,
a fourth mode on the existing flag — exactly what the entry asked for and no
ADR at all. Two more had already been decided. Check the tree and name the
actual principle before filing the next one.

### Declined — do not re-raise

- **In-run scenario `@retry`** (cucumber-rs's headline feature, ranked
  first by one research stream): the CI-standards stream independently
  established retry-until-green as the anti-pattern (a 25%-failure bug
  passes 99.6% of the time under three retries) and proef's
  detect-then-quarantine shape as the consensus architecture; per-step
  `retry:` already covers polling. Document the stance in
  TESTING-STRATEGY instead — it reads as a gap until stated.
- **CTRF** — the deferred trigger was checked and has **not** fired: no CI
  platform ingests it natively (GitLab/CircleCI are JUnit-only, GitHub has
  no format at all, Buildkite has its own JSON). If a consumer ever
  materializes, Buildkite JSON is the higher-yield target (its span model
  maps 1:1 onto step outcomes) — same trigger discipline.
- TAP 14 (unratified branch, zero declared consumers) · Bruno-style
  granular exit codes (ADR-0009 is a contract) · an OS-keychain secret
  backend (second storage mechanism) · a user-level personal config file
  (proef.toml is the one channel; `NO_COLOR` covers terminal taste) ·
  Karate `match within` sugar (hurl predicates cover it).

### Environment note (machine-side, not repo-side)

Homebrew's Rust (1.98.0) shadows rustup on this machine's PATH
(`/opt/homebrew/bin/cargo` first), which breaks `cargo +nightly` and the
public-api gate and silently un-pins builds; Wave 1 gates were re-run
under the pinned 1.97.1 explicitly. Owner action: `brew uninstall rust`
or reorder PATH.

---

## Ingested — Robot Framework capability audit (2026-08-24)

A deliberate mining of Robot Framework 7.x for transferable ideas, run as
five extended-context investigations (one per adoption candidate, plus a
counter-audit attacking the first-pass verdicts), every load-bearing claim
reproduced against the tree before anything shipped.

### RF wave 1 *(shipped, #96–#101)*

Detail cap at the engine boundary (RF's 40-line rule) · tag-atom globs
(`*`/`?`, anchored; the silent-no-match became the intended selection) ·
`flows` feature descriptions (parsed, was dropped) · `--shuffle` seeded by
the run id (R3-9, one determinism knob) · `reproduce_hint` into the record
(the console knew more than `explain` did). The shard parity fix (#96) was
round 18's, not RF's, but shipped in the same wave.

### RF wave 2 — the schema wave *(all three shipped, #103–#105)*

- **RF-W2-skip** *(shipped — ADR-0019, #103)* — `@skip` / `@skip:reason-token` (prefix verified to parse
  as one tag); `reason` on `ScenarioFinished`+`ScenarioOutcome`; one
  reserved-tag module (quarantine moves in); the two mapped collisions are
  the point of the work: `--rerun` re-queues Skipped-on-cancelled (authored
  reasons start with `@`, mechanical never do), and `diff` buckets
  Failed→Skipped as *fixed* — three-way bucketing required. All-skipped →
  exit 0 (ADR-0009 argument recorded); harness → libtest-mimic ignored.
- **RF-W2-tags** *(shipped — #104)* — `tags` on `ScenarioFinished` only (the cancel-skip path
  emits no `Started`), `exclusive` on `ScenarioStarted` (closes R11-6),
  schema stays 1; HTML + GH-summary per-tag tables, suite-only per
  ADR-0014; the quarantine `non_gating` list re-derivation collapses into
  the new one owner; D1 becomes its predicted recipe. NOT building RF's
  tagstat combine/link/doc knobs.
- **RF-W2-meta** *(shipped — ADR-0020, #105)* — `--meta k=v` + `[meta]`/`[env.<name>.meta]` on the
  existing precedence chain; `RunStarted.env` auto-recorded (handed-over,
  not harvested — R12-1's real axis); values through the one sink-boundary
  mask; JUnit `<properties>` deferred by the named-consumer method; never
  in artifacts; ADR codifying explicit-injection-only ships with it. A
  `shuffled: bool` marker rides the same `RunStarted` change (deferred out
  of #100 for one wire change instead of two).

**Hazard both schema items must clear:** `stamp_scenario_timing` and
`phase_sink` (exec.rs) rebuild scenario events field-by-field — a new field
compiles clean and is silently stripped from every stamped stream unless
threaded there, with an integration test per field.

### RF wave 3 *(shipped, #106–#108)*

- **Rerun merge-at-report** *(shipped — #106)* — the record-composition half
  of E2: a rerun's JUnit carries the base's not-re-run suite scenarios as
  ordinary testcases, and `report` overlays the base for one whole-suite
  page. Composition over records — the record files themselves never merge
  (ADR-0008); totals and the exit code stay the rerun's own (ADR-0014).
- **Console modes** *(shipped — #107, extended #147)* — `--console
  full|failed|dotted|quiet`. The
  OSC-8 hyperlink half did **not** ship: printed paths stay plain until a
  terminal consumer asks (the same named-consumer method as JUnit
  `<properties>`).
- **Quarantine in JUnit** *(decision taken; shipped with ADR-0019, #103)* —
  a quarantined test-failure maps to `<skipped message="quarantined failure
  (non-gating): …">`, so Jenkins, the dashboards and the exit code agree.

### Deferred with named triggers

Report-size mechanism (first >1k-scenario record; failures-only render, one
mechanism not three knobs) · `--runemptysuite` (first CI consumer; settle
the early-error record first) · JUnit `<properties>` (a
Jenkins-`keepProperties` user). The `--tagstatlink` analogue left this list:
it shipped as `[tag-links]` (#108).

### Rejected, and where the basis actually lives

`--nostatusrc` (ADR-0009 is a contract) · argfiles/`ROBOT_OPTIONS`
(proef.toml is the one channel) · pre-run modifiers, custom parsers,
listener API (PRD §3 non-goals + product identity) · GROUP · `Set Test
Message` · `--exitonerror` (`--max-fail` is the one early stop) ·
`robot:private` (the macro listing exists to *show* the vocabulary).
**Two stances the counter-audit showed are held but unwritten** — control
flow lives in packs (`when:`/`optional:`/`retry:`), prose stays
declarative; and proef has no runtime extension surface, the record is the
observation API — both belong in AUTHORING or a short ADR when wave 2's
ADR is written anyway.

### Counter-audit corrections *(for the record)*

"`--rerun` ahead of RF" was half wrong — ahead on selection (cancelled-tail
union), missing the merge half entirely; see E2. "HTML report ahead of
log.html" — ahead on visualization, was behind on forensics (the
`reproduce_hint` gap, now closed by #101; request/response excerpts remain
a deliberate non-goal until asked). "@quarantine ≈ `--skiponfailure`" holds
only for exit-code CI (see wave 3). RF 7.4 added a `Secret` type — proef's
redaction invariant predates it; banked as an ahead.

## Ingested — round 18 (2026-08-24), validated claim-by-claim

### R18-1 — the shard hash collapse, round two *(confirmed — shipped)*

Round 18 tested this registry's R17-2.1 refutation instead of restating the
round-17 claim, and won the half that matters. The mechanism is arithmetic,
not statistics: FNV-1a's multiplier is odd, so the accumulator's low bit is
exactly the XOR-parity of the input bytes' low bits; a scenario named after
its feature file — the commonest Gherkin convention — duplicates content
across the `(file, name)` identity, whose parity contributions cancel,
leaving a corpus-constant bit: `N=2 → [20,0]`, odd buckets empty at `N=4`
(reproduced against the real `shard_bucket`, then pinned red in the balance
test before the fix). The R17 balance test could not see it **by
construction** — all three corpora held the file constant, the one condition
under which raw FNV behaves. Shipped: Murmur3 `fmix64` finalizer on
`shard_bucket` (Breaking: every matrix re-deals), the `mirrored` corpus in
`natural_corpora_spread_across_shards`, and bounds recalibrated to what a
well-mixed hash yields (no empty shard at any `N`; the 3× skew bound at
`N=2` only — a fair deal of 20 over 4 buckets legitimately produces
`[2,7,5,6]`). The reviewer's own concession stands for the record: fmix64 is
mildly worse on constant-file corpora (`[7,13]` vs `[10,10]`), which is
randomness, not structure — no empties.

### R18-2 — Rust pin "four days overdue" *(refuted — and the policy is now written)*

The pin follows the practiced policy — adopt a new stable at its `x.y.1`
point release, ~3–4 weeks after `x.y.0` (1.98.1 expected mid-September) —
but the reviewer read CLAUDE.md's "always latest stable Rust", which said
otherwise. An unwritten policy that contradicts the written one is a docs
defect on our side: the policy now lives in CLAUDE.md and RELEASING.md, and
the pin bump lands on 1.98.1, as it always would have.

### R18 closures

Eleven round-17 closures re-verified by the reviewer against `cc75129` with
original repros; nothing reopened. The review singles out the machine-body
funnel and the flags-direction docs gate as the durable forms of their fixes.

## Ingested — round 17 (2026-08-23), validated claim-by-claim

Two P1s filed; one confirmed both ways it can be read, one refuted by
measurement. Every confirmed item reproduced against `b5b320a` before any fix.

### R17-2.1 — `--shard` hash collapse at power-of-two counts *(refuted for constant-file corpora; corrected by R18-1)*

The filed claim: FNV-1a's unmixed low bits collapse the distribution at
`N=2/4/8` ("100% of scenarios land in one shard"), fix with an fmix64
finalizer. **Measured with a model calibrated against the frozen-literal test
(exact match on every pinned value), the claim inverts.** Natural corpus
shapes — numbered scenarios, prose names, outline `#N` instances, camelCase,
verb templates, multi-file — are near-uniform under the current
`fnv % count`: `[10,10]`, `[11,9]`, `[4,5,6,5]` at their widths. The proposed
fmix64 is *worse* on the same corpora (`[7,13]` where FNV gives `[10,10]`,
empty shards at `N=8` that FNV does not produce): FNV's parity-structured low
bit behaves like round-robin on templated names, which real suites are full
of. Collapse requires a degenerate corpus — every name an even-length run of
one character — which no suite exhibits. The filed measurement tables do not
reproduce from the calibrated function. What survives: **no test asserted
balance** — shipped as a distribution test over natural name shapes, so a
future hash change that *does* skew fails loudly.

**Round-18 correction:** the refutation above held only where its evidence
did — every corpus it measured kept the file path constant. Round 18 showed
the varying-file half was real (see R18-1): the low bit of raw FNV is byte
parity, and a scenario named after its feature file cancels to a
corpus-constant parity — `[20,0]` at `N=2`. "Degenerate corpus only" was this
registry's error, not the reviewer's.

### R17-2.2 — `bind:` validation refused input the engine accepts *(shipped)*

Confirmed, both halves, plus a third the round missed:

- `{{newUuid}}` in a bind value was refused as an unbound variable; it is a
  hurl **function** (`ExprKind::Function`), and stock hurl 8.0.1 runs the
  equivalent line (reproduced both directions). "What does this text read" is
  now the engine's answer — `FragmentSupport::template_reads`, the same AST
  walk the fragment scanner uses — so the tree holds one answer, not two
  disagreeing ones.
- A sibling literal bind sorting before the bound key is a real supplier
  (injected lines are written and evaluated in name order) and is now
  accepted; a later-sorting sibling stays refused, with the ordering named in
  the help.
- **The round's fix list missed the ordering half:** injection landed at the
  *head* of an author `[Options]` section, so the fragment-supplies-it route
  the check accepts was assigned too late to be read at run time. Injection
  now lands at the section's end; pinned by
  `a_fragments_own_variable_evaluates_first`.

### R17-2.3 / 2.4 / 2.5 — machine output and phase reporting *(shipped)*

An empty shard wrote prose (plus a stray-space run) where a `--output
json`/TAP body belongs; a setup abort wrote JUnit but zero machine-stdout
bytes; a failed teardown reached no report at all. **Shipped as one
mechanism each way:** `emit_machine_body` is called by every terminating
path (pool, empty shard, both setup aborts) with ADR-0014 suite-only totals
and the path's own exit code — the note moved to stderr under machine
output — and a *failed* teardown's outcomes ride into `write_junit` as their
own suite (#78's rule made symmetric; a green phase stays out). Deliberate
scope *as recorded then*: the GitHub summary keeps pool-only totals. The
second audit pass showed the code does not hold to it — a setup abort passes
the *setup* summary as the primary, so setup failures render in the GitHub
summary while teardown failures do not. Queued: unify all three CI sinks on
"a phase appears when it fails" at the `write_ci_reports` boundary, with
totals staying suite-only everywhere (ADR-0014).

### R17-2.6 — batch *(shipped)*

README omitted `--shard`/`--max-fail` (and the #73 gate was blind to the
flags direction) — closed with a reverse-flags gate whose measured burden was
exactly three flags; `explain`'s truncated-record fallback now filters
through `is_suite()` (the fourth consumer #72's helper was built for); the
canary refuses a backport older than the pin by semver ordering, not
equality; identical warnings collapse to one with a repeat count (every
class, at the front-end aggregation — `bind_shadows_capture` was the
motivating fifty-warning wall); `quick-xml` rides at quick-junit 0.7's
in-tree copy again, one generation in the lock. P4s *(all shipped in #87)*: the pages workflow comment now states that
`upstream/`'s `.patch` files are served; `docs/runbooks/` entered both
`living_docs` scanners; outline identity's positional `#N` is documented in
AUTHORING with the column-placeholder remedy; the #79 comment stopped
claiming `file:line` survives in the failure detail.

### Standards note

Rust 1.98.0 released 2026-08-20 (verified against the channel manifest). The
round calls the pin overdue; house policy waits 3–4 weeks after `x.y.0` and
targets `x.y.1` — the window opens ~2026-09-10.

## Open — round-9 residue (ingested 2026-08-12)

The review's P1/P2 and three P3s shipped in #48 and #50. What follows is what was
verified and deliberately not built, so none of it depends on remembering.

### R9-1 — `proef fragments` has no listing command

`flows` lists scenarios and `macros` lists the vocabulary; nothing lists the
corpus. There is no way to ask which fragments exist, which are referenced, or
which `.hurl` entries carry no annotation — and an unannotated entry is *dropped
at scan time by design*, so the tool structurally cannot report what it never
built.

Raised by a consumer migration whose coverage gate ("every `@proef` name is
referenced, every entry is annotated") had to become a script that repo owns.
**Not built for 0.10.0 on purpose:** new public surface, and the migration was
unblocked by correcting its own gate instead.

**Shipped.** A second migration report (`ADOPTION-REQUEST.md`, 97 entries)
supplied the field evidence this entry was waiting for and ranked it first of
seven. `proef fragments` now names both death modes apart, lists unannotated
entries by line, and gates CI with `--check`; `--require-annotated` is opt-in
because an unannotated entry is inert *by design* (ADR-0018), so "not done yet"
is a porting team's reading of that signal and not every adopter's.

### R9-2 — fuzz coverage does not reach the fragment surfaces *(shipped)*

`fuzz_pack_load` runs with an empty corpus, so `ref:`/`bind:` clash logic never
executes under fuzzing; the annotation scanner's entry-boundary arithmetic —
proef's own code, not hurl's — and `bake_entry_options`' textual injection are
unfuzzed entirely. Split the fuzz input into pack and corpus halves, and consider
a `fuzz_fragment_scan` target (nightly, accepting the native-libs cost).

**Shipped, and the prescription was half wrong — measurably.** Splitting the
input into pack and corpus halves was tried first and *did not work*: a
byte-oriented target never resolved a single `ref:` in **1.45 million runs**,
because reaching the rules means discovering valid YAML and a matching corpus
name simultaneously. Verified by probe (panic on a resolving `ref:`, run the
fuzzer, see whether it fires) rather than assumed from coverage numbers — which
is the same mistake this finding is about, one level up.

What shipped instead is `fuzz_fragment_binding`, **structure-aware**: it builds a
well-formed pack and corpus from the input and spends the budget on the name
space, so every run reaches the rules. The probe fires in seconds.
`fuzz_pack_load` stays byte-oriented and unchanged — parser totality is a real
job and the split would only have diluted it.

The `fuzz_fragment_scan` half was declined for a concrete reason, not on cost
alone: cargo dependencies are **package-level**, so adding `proef-engine-hurl` to
the fuzz crate compiles hurl for all five targets and drags native libraries into
a job that has none. Hurl's scanner is instead **property-tested in
`proef-engine-hurl`**, where those libraries already are — pinning that every
reported line lies inside the file, that entries are accounted for exactly once
in order, and that no fragment's text runs into the entry after it. The last
assertion was added after mutation testing: the first draft passed with the
boundary deliberately broken.

Still open from this entry: **`bake_entry_options`' textual injection is
unfuzzed.** It is lower-time, not load-time, so it sits behind lowering rather
than `pack::load` and needs its own target.

### R9-3 — no resource bounds on the corpus read *(shipped)*

No per-file or file-count cap: a multi-GB `.hurl` is read whole on every command
that loads packs. Pairs with the read-resilience work in #48, which made the read
*survivable* but not *bounded*.

**Shipped, and worse than filed by one word:** not "a multi-GB file" — a 279 MB
file cost **601 MB of resident memory** on `proef flows`, a command that never
looks at a fragment, over a file carrying no `# @proef` annotation at all. The
doubling is `read_to_string` into a `String` and then `Arc::from(&str)`, which
copies.

Bounded now at 8 MiB per file and 64 MiB per corpus, measured from the directory
entry so an oversized file is never allocated (601 MB → **15 MB** on the same
input). Reported through the per-file diagnostic channel `unreadable_file`
already established — skipped, never fatal — and applied in `proef lsp` too,
where the corpus is *held between requests* rather than for the length of one
command. The laziness promise is intact: a corpus nothing `ref:`s still reports
nothing and exits 0, pinned by a test.

The `Arc<str>` copy itself was left alone. Removing it means changing
`PackSource`'s type across every reader, which is a wider change than a bound
and buys a constant factor on an input that is now capped anyway.

### R9-4 — a bind that shadows a capture is silent *(shipped)*

hurl's `variable:` assigns into one shared set, so a pack- or macro-scope `bind:`
re-assigning a name an earlier entry captured overrides it for every later entry,
with no diagnostic. A warning shaped like `option_declared_twice` fits — the
difference is that this one is only decidable where the capture set is known, at
lower time.

**Shipped** as `proef::lower::bind_shadows_capture`, a warning per the verdict
above — a fixed value over a live session is sometimes deliberate. Only a
*literal* bind warns: a secret bind skips the `[Options]` path entirely, so the
earlier capture's assignment stands and there is nothing to warn about (pinned
by a unit test). En route it was validated that `unread_bind_key` already
narrows the surface to binds a fragment in scope reads — the live gap was
exactly the capture-shadow shape.

### R9-5 — `{{x}}` inside a bind value is unvalidated at lower time *(shipped)*

It fails at run time instead of at `--dry-run`: loud, but late, and the late half
is what `--dry-run` exists to prevent.

**Shipped** as the same `proef::lower::unbound_placeholder` the fragment check
uses — one code for one defect class — naming both the placeholder and the bind
key, anchored on the feature step (pack-line anchoring from lower time is R1's
recorded deferral). The accepted suppliers, each pinned: an earlier step's
capture, the fragment's own `[Options] variable:` (authored lines precede the
injected ones), and a secret in scope — a run-time `{{secret}}` reference never
puts the value in an artifact, unlike the `${secret:…}` splice that
`secret_in_composite_bind` refuses.

### R9-6 — provenance is cwd-dependent *(shipped)*

Run from a subdirectory and `step_finished.fragment`, explain's `via`, JUnit and
the diagnostics carry an absolute machine path; the record-portability claim holds
only from the project root. Relativize against the config root rather than cwd —
the same boundary `[run] fragments` already resolves against.

**Shipped** as part of R12-1, which found the same defect reaching further than
this entry describes — the safe case it names, running from the project root, had
stopped being safe. The prescription here was the right one and is what landed:
one anchor, the config directory, for every input kind.

### R9-7 — smaller edges, verified and recorded

Artifacts written *inside* a fragments root poison the corpus with proef's own
output (loud, but the remedies misdirect — skip files carrying the artifact
header, or document it); a step-scope `bind:` key the fragment never reads is
silently baked as a run-level `variable:` and can shadow a later capture, and an
unused `${secret:}` bind silently widens the required-secret set (warnable at step
scope, where it is decidable); a `# @proef` annotation placed mid-entry is
silently ignored and the resulting `unknown_ref` does not hint at misplacement;
`proef macros` prints a corpus error twice on the degraded path; same-file
duplicate annotations read as "declared in both f.hurl and f.hurl".

**The double print is broader than filed** (verified 2026-08-14 while adding the
corpus bound, which inherits it). It is not specific to `macros`: `proef
fragments` does it too, and to *any* corpus diagnostic — `unreadable_fragment_file`
and the new `oversized_fragment_file` alike. The mechanism is that
`commands::fragments` renders `corpus.diagnostics()` itself and then loads the
suite, whose failure path renders the same diagnostics again. Both land on
stderr, so the count line reads `1 error(s)` under two rendered copies. Left
here rather than folded into the bound: it is a rendering decision about which
of the two sites owns corpus diagnostics, not a property of any one diagnostic.

## Open — round-10 residue (ingested 2026-08-12)

Found by a cleanup review over the fragments branch, after its own gates were
green. All three are consequences of what that branch added; none is a defect in
what shipped before it. Recorded rather than fixed in place because each is a
behaviour change, and the branch was already carrying two correctness fixes.

### R10-1 — `--config` is honoured by the runner and ignored by the editor *(shipped)*

`--config <path>` bypasses the upward search so a `proef.toml` beside the suite
becomes usable. `proef lsp` never sees it (it re-discovers via
`ProjectConfig::load_from`), and `--watch` watches the config found by its own
fresh upward search, not the one the run was given.

So in **exactly the layout the flag exists for**, `proef test --config …` runs
green while the editor gets no `[run] fragments` and reports every `ref:` as
unknown — diagnostics disagreeing with the runner, which is the drift that makes
an editor untrustworthy.

**Shipped.** `ProjectConfig` now keeps the *file* it was read from and derives
`root` from it, rather than storing the directory and leaving every consumer that
needed the file to search again. `--watch` watches the config the run resolved
through; `proef lsp` takes the flag and lets it outrank even the client-announced
workspace root, since a named file is not a guess to be improved on. The free
`config::config_path()` — the fresh upward search both bugs went through — is
gone, which is what stops the class recurring. `proef lsp` still starts when a
named config is missing (an editor offering less beats one that will not boot),
where the runner exits 2; the asymmetry is deliberate and documented.

### R10-2 — `proef fragments` judges reachability over a smaller universe than the runner *(shipped)*

`[run] setup` / `[run] teardown` are not loaded, so a fragment used **only** by a
phase feature counts as never run and fails `--check` — a false CI failure in the
workflow `--check` was asked for, unless the phase feature happens to sit inside
the suite directory. `exec::execute` already threads one corpus through both
phase validations and both phase runs; the listing needs the same universe.

### R10-3 — three predicates answer "is this a fragment file?", and they disagree *(shipped)*

`front::fragment_extensions` (exact match, and its doc claims to be "the one
place that answers this"), `pack::scan_fragments` (exact), and the LSP's own
`is_fragment` (case-insensitive). `api.HURL` therefore invalidates the editor's
corpus but is never scanned by core or discovered by the CLI.

The shared home is `proef_core::engine`, beside `StepKindSpec` — it is pure logic
over the registry, so it is sans-IO-legal, and `proef-lsp` cannot reach
`proef-cli`'s copy. Worth pairing with the deeper question the LSP predicate
raises: membership in `discover_fragments()` is the real test, and an extension
match also claims emitted artifacts that happen to end in `.hurl`.

### R11-1 — `proef.toml` resolved its paths against two different roots *(shipped)*

`[run] fragments` resolved against the config file's directory and `suite`,
`setup`, `teardown` and `runs-dir` resolved against the working directory, so the
same relative spelling meant two directories depending on which key it sat under.
`.proef-state.json` and `.proef-secrets.json` were cwd-anchored too and appeared
in no inventory, making two shells in one project two Worlds and two secret
stores. One rule now: written paths resolve against the config, typed paths
against the working directory.

### R11-2 — `--watch` retriggered on a config it then ignored *(shipped)*

The loop watched `proef.toml` and reran on an edit while the rerun used the
startup snapshot, so changing `[url] base` produced a rerun that called the old
host. Fixed by re-reading per rerun — and by moving the startup config out of
scope, which makes the stale value unreachable from the rerun closure and the
invariant a compile error rather than a habit. Which *directories* are watched is
still fixed at startup, so `[run] fragments` and `[run] suite` need a restart to
be watched. `runs-dir` was in that list until R11-8 showed it did not belong
there: it is not a watched root but an *excluded* one, and freezing it was the
bug rather than the limitation.

### R11-3 — `--config` was honoured, swallowed, or ignored depending on the command *(shipped)*

`doctor` printed the error for a missing named file and then reported on
defaults, exit 0; `fmt`, `init`, `schema` and `secret` accepted a nonexistent
path silently. Three documents called the flag global to every subcommand. A
named-but-missing file is exit 2 everywhere now; `doctor` stays lenient about
discovery, which is a different claim.

### R11-4 / R11-5 — `[run] exclusive-tags` did not validate itself *(shipped)*

`--dry-run` never parsed the expression, and a well-formed expression matching
nothing was silent — both defeat the reason the setting is a config expression
rather than a reserved tag name.

### R11-6 — exclusivity is invisible in the run record *(shipped — RF wave 2)*

`Event::ScenarioStarted` carries no field saying a scenario ran exclusively, so a
post-mortem cannot tell a deliberate drain from a stall: the record shows
parallelism dropping to one and nothing explaining why. An additive field is
permitted by ADR-0008, and the reporters would need to decide whether to surface
it. Filed rather than built — it is a design question about what the record
should say, not a defect, and the run behaves correctly either way.

**Closed (2026-08-24):** `scenario_started` carries additive `exclusive` —
the very bool the scheduler read, never re-evaluated. Surfaced in the HTML
timeline title only; every other reporter deliberately ignores it.

### R11-7 — the corpus-read rule is shared, its *discovery* is not *(closed 2026-08-23 — discovery unified: one walker, one `claims` predicate; the surviving asymmetry is size measurement — `fs::metadata` vs text length — deliberate and documented, an unsaved buffer has no file to stat)*

`FragmentCorpus::unreadable_file` now gives both readers one diagnostic, but the
CLI walks the fragment root with `std::fs` while the LSP reads through its
overlay provider. That difference is real — the editor must see unsaved buffers —
so the readers stay separate. What is worth watching is that "which files are in
the corpus" is still answered twice, and only the *meaning* of a failed read was
unified here.

### R11-8 — a `runs-dir` edited mid-`--watch` fed the loop its own output *(shipped)*

R11-2 made each rerun re-read the config, so records went to the *new* runs dir
while the watcher's exclusion still named the one frozen at startup. Every
rerun's `artifacts/*.hurl`, now under an unexcluded directory, requeued the next
run: 39 runs in 12 seconds, firing real traffic, from one edit. The third outing
for this class, so the fix removes the second answer rather than resynchronising
it — each rerun registers where it is about to write, *before* it writes, and the
exclusion is derived from the same config the run is. Deliberately not a
uuid-shaped exclusion: `--run-id` names a run directory that is not uuid-shaped.

### R11-9 — a relative `--config` was never the file `--watch` matched *(shipped)*

The watcher compared the config by exact path while `notify` reports events under
the spelling the OS resolved them to, so `--config proef.toml` matched nothing and
config edits produced no rerun — silently, because feature edits kept firing and
the loop looked alive. Two questions had been conflated: where a path *points*
(answered once, lexically, when the flag is stored) and whether two paths *are the
same file* (answered by comparing canonical forms, since absolute is not enough —
macOS's `/var` → `/private/var` aliasing and symlinks both survive it). The same
relative path had been costing `proef lsp --config` go-to-definition across the
whole corpus, because `documents::name_to_url` refuses a relative name.

### R11-10 — `doctor` reported on defaults over a `proef.toml` that would not parse *(shipped)*

R11-3's discovery arm became a silent `unwrap_or_default`, dropping the parse
error the previous code printed: a malformed config left `doctor` reporting on
invented defaults and printing "all checks passed", exit 0. A `project:` row now,
so it reaches `worst` and the exit code CI reads. Leniency still means *absent* —
`doctor` must run outside a project — not *broken*.

## Ingested — competitive research v2 (2026-08-16), validated claim-by-claim

An external research pass (prototyped against the built 0.12.0 binary) plus its
round-14 companion review. Each actionable claim was re-reproduced here before
anything was written down. Disposition:

### S1 — an encoded reflection of a secret defeated redaction *(shipped)*

**The one defect in the set, confirmed by live reproduction**: a server
reflecting the bearer token base64-encoded put `dG9r…` (trivially decodable)
into an assert-failure detail; the raw needle never fired; the encoded
credential reached the console and `events.jsonl`. The raw-form invariant was
intact — this violated its *intent*. Shipped as derived needles inside
`Redactions::new` (see the changelog and the ADR-0005 amendment); property- and
mutation-tested, pinned end-to-end against a fixture introspection route.

Not covered, on purpose: hashed/split/re-encrypted reflections (not needle-
matchable), double encodings (an unbounded tower; echo endpoints produce one
level). The research doc's companion ideas — a redaction-verifying scan over a
finished run record, GitHub `::add-mask::` for captured secret-typed values,
RF-style secret-typed macro *arguments* — are enhancements, not part of the
defect, and await triage.

### Corrections to the research set, so they are not re-litigated

- **S4 (Trusted Publishing plan) rests on a false premise**: it plans a
  *first* publish with a classic token, but all four crates have been live on
  crates.io since 0.5.1 (0.12.0 current). Trusted Publishing can be configured
  directly against the existing crates; the token sequence is unnecessary.
- **S2's exposure check is right and already satisfied**: `Cargo.lock` carries
  `curl-sys 0.4.90+curl-8.21.0`, past the June-2026 CVE batch. The *detection
  blind spot* (RUSTSEC carries no advisories for `*-sys`-bundled C libraries)
  is real; the proposed libcurl-version print in release artifacts awaits
  triage with the rest.
- **R3-16/R3-17 (browser and Android engines) are foreclosed**, not deferred:
  proef is API-testing-with-hurl only — a standing decision, not a gap the
  research reopens. The M6 line in CLAUDE.md is architectural readiness, with
  nothing scheduled. The seam-hygiene half of R3-15 stands on its own merits
  and awaits triage like the rest of the registry.
- The round-14 review audited `214a39d` (a pre-amend commit never pushed;
  what merged is `c3ac752`, differing by one deliberately-removed proptest
  seed), counted 464 tests where 462 exist, and credited #63 with the LSP
  corpus-holding change that shipped earlier — recorded here because review
  counts have now drifted by +2 for three consecutive rounds.

### The R3 registry — triaged 2026-08-17

Triaged as a set against the PRD, the ADRs, and current industry practice,
with each seam re-validated against the tree first. The v1 research document
was confirmed absent (only v2 exists on disk), so items defined only there are
one-line summaries with no spec — that fact drives several verdicts below.

**Built:**

- **R3-1 `--max-fail N`** *(shipped with this triage)*. The convention is
  universal — Playwright `--max-failures`, pytest `--maxfail`, nextest
  `--max-fail` — with one shared semantics: stop after N failures, un-run
  tests report as not-run rather than passed. proef's seams made it a CLI-only
  change: a sink wrapper counts suite-scenario failures (the `phase` field
  keeps setup/teardown out of the count) and cancels the run token, which is
  the tested Ctrl-C drain path — in-flight batches finish, the rest record as
  skipped, teardown still runs on its own token, and the record is a complete
  *cancelled* run. That last part is free correctness: `diff
  --fail-on-regression` already refuses to certify a cancelled run, which is
  exactly right for a deliberately-partial one.
- **R3-4 `diff` takes a record path** — shipped earlier (#65), with the
  research's `--baseline` flag spelling declined as a second name for the
  same positional.

**Build next (validated, in order):**

- **R3-2 a flakiness verdict** — *(shipped as `proef flaky`)*. The 2026
  pipeline is detect → quarantine → resolve, and proef already owned the
  middle step (`@quarantine` runs-but-does-not-gate); `flaky` is the missing
  detect, a fold over the records `runs-dir` already retains, so the history
  window is `[run] keep-runs` and no new state exists. Transition-counting
  separates flaky from broken (a mutation test proved the test suite could
  not initially tell that apart from a naive fail-rate — the F,F,P,P case
  now pins it), per-step attempt counts surface the pass-only-on-retry
  latent class, and a cancellation-skipped row is not evidence. **No
  `--check` gate, deliberately** — its sibling `fragments` has one, but a
  flakiness verdict is advisory by nature and `@quarantine` owns the gating
  decision; the asymmetry is a choice, not an omission, and the thresholds
  become contract (and move to `proef.toml`) only if a gating mode ever
  exists.
- **R3-3 sharding, hash-mode only** — *(shipped as `--shard I/N`)*. The
  measured stability argument held end to end: the mutation test swapped
  index-slicing back in and the insertion case (prepend, which shifts every
  position) caught it — the *append* case did not, which is itself the
  finding's point. The assignment is frozen by literal-pinned tests; changing
  the hash is a breaking change to every sharded matrix. Filter→shard order
  pinned; an empty shard of a non-empty selection exits 0 with a note.
- **R3-6 JUnit attributes** — *(shipped, from the fresh spec the triage
  required)*. The spec was written from what the two consumers actually parse,
  at source level: GitLab's docs enumerate testcase `classname`/`name`/`file`/
  `time` plus suite and root `time` — and explicitly ignore the count
  attributes and `timestamp`; Jenkins' `SuiteResult.java` reads suite
  `name`/`package`/`id`/`time`/`timestamp` and case `classname`, and never
  reads `hostname`. What shipped, and why:
  - **Identity became `classname` + `name`** — Jenkins keys test history on
    the pair, GitLab's MR widget diffs head against base by it, and the old
    single `name` embedded `file:line`, so an edit above a scenario
    re-identified every test below it (a fleet of "new" tests on both tools).
    `classname` carries the feature file, `name` the scenario alone — unique
    per file by construction (outline instances are `#N`-disambiguated).
    **Breaking** for anything keyed on the old names.
  - **`file` on the testcase** (GitLab source linking), **`time` on suite and
    root** (both consumers), and the suite `skipped` count spelled `skipped`
    (quick-junit 0.5 → 0.7; 0.5 wrote `disabled`, which neither consumer
    reads).
  - **`timestamp` and `hostname` deliberately absent** — GitLab ignores both,
    Jenkins substitutes its own build clock and never reads `hostname`, and
    naming the machine would undo R12-1. Additive later if a consumer asks.

**Deferred, with the trigger named:**

- **R3-5 CTRF output** — on the first concrete consumer request. The format
  has momentum but proef already emits JUnit, TAP, JSONL, a GH summary, SARIF
  and HTML; a seventh format needs a consumer, not a trend.
- **R3-18 generated pack documentation** — when pack-vocabulary discovery becomes a
  reported adoption pain; the LSP currently serves that need interactively.
- **R3-15 pre-M6 seam refactors** — when a second engine is actually
  scheduled (M6 has nothing scheduled; the snapshot corpus already provides
  the golden artifact-diff prerequisite).
- **R3-7 `--affected-by`, R3-10 fake variants** — defined only in the absent
  v1 document; need the source or a fresh spec before any verdict.
- **R3-9 seeded shuffle** — **shipped** as `--shuffle` (RF-audit wave 1).
  The old pointer here was dangling: IMPROVEMENT-PLAN #14 is the *fakes*
  seed and never mentioned order. The shipped form honors #14's actual
  rule anyway — the permutation is seeded by the run id, no parallel seed.

**Declined — do not re-raise** (moved to the standing section's rules):

- **OTel trace export (R3-11) and Cucumber Messages (R3-12).** ADR-0008: the
  JSONL event stream *is* the record, no second record format. Both are
  re-encodings of the record for ecosystems that can convert from JSONL
  outside proef; building them in creates permanent format-tracking
  obligations against moving upstream schemas.
- **Browser/Android engines (R3-16/R3-17)** — foreclosed by the standing
  hurl-only decision, not deferred.
- **S4's first-publish token sequence** — false premise; the crates have been
  live since 0.5.1. The worthwhile residue (crates.io Trusted Publishing for
  the *existing* crates, then the token-delete) is an owner-side dashboard
  action, recommended to the user rather than something the repo can do.

## Open — adoption report on 0.12.0 (ingested 2026-08-14)

From a suite that ported to `ref:` at scale — 15 hurl files, 112 fragments, 21
scenarios — and ran 0.12.0 as an installed release. Three items, each reproduced
here against the tree before being written down. Two shipped in the same change;
the third is recorded because the report's *diagnosis* was wrong even though its
*observation* was right, and that distinction is the finding.

### R12-1 — provenance named the machine that produced the record *(shipped)*

`[run] suite` resolves against the config directory (R11-1), so a path-less
`proef test` handed the front end an **absolute** path and every emitter printed
it: the `.hurl` `# source:` header, `.map.json`'s `feature.file`, every
`step_finished` event, the console, and pack diagnostics. Two checkouts of one
suite stopped producing equal artifacts, which is exactly the property ADR-0010
exists to guarantee.

**Worse than R9-6 filed it.** R9-6 says the portability claim "holds only from
the project root"; this reproduces *from* the project root with the config in it.
R11-1 was the right fix — one resolution rule — but resolution produces absolute
paths, and nothing was named at the other end.

**Shipped, and R9-6 with it.** `front::SourceNaming` is the one naming boundary:
resolve against the project, then name against the project again. A relative path
is left exactly as it arrived (machine-independent already, and the caller's own
spelling, which their terminal can open); an absolute one is spelled relative to
the config directory when it lies inside it. This also replaced the fragment
corpus's cwd-relative strip, which was a second anchor for the same question —
the drift R9-6 predicted. The four ways to name one suite (derived, typed, typed
absolute, from a subdirectory) now emit one artifact byte-for-byte, pinned by
`crates/proef-cli/tests/provenance.rs`.

Two limits, deliberate: a corpus genuinely outside the project keeps its absolute
name, because no project-relative one exists; and `DiskSourceProvider`
(`proef lsp`) still yields absolute names, because it keys document identity on
them.

### R12-2 — the run-record ceiling was a constant no project could reach *(shipped)*

Retention was `const RUN_RETENTION = 200` with only `runs-dir` configurable, and
artifacts are byte-identical across runs of an unchanged suite — so a suite
re-run on every save accumulated identical bytes for a day before anything
signalled a ceiling existed. `[run] keep-runs` makes the policy expressible; `0`
keeps none but the run in flight.

**The report's inference that artifacts should therefore not be stored is wrong,
and it said so itself:** an old record's artifacts are what *that run executed*,
and once the corpus changes `proef artifacts` no longer reproduces them. Bound
the cost, do not drop the evidence.

**Not closed by this, and not reported:** rotation only ever deletes directories
named by a *generated* run id, so `--run-id <name>` records sit outside the
budget entirely. A CI minting a fresh id per build accumulates without bound.
Guessing at user-named directories is the worse failure — `runs-dir` may be `.`
— so this stays, documented in CONFIG.md rather than fixed.

### R12-3 — a `[run] setup` test failure is invisible to JUnit *(shipped)*

Reproduced: a setup feature whose *assertion* fails exits **2** with
`summary: 0 passed · 0 failed · 0 skipped`, and `--output junit` writes an empty
report, because the abort precedes the reporter. A CI reading JUnit sees nothing
at all.

**The exit code is not the defect.** ADR-0014 decided it explicitly — a setup
failure maps to a user (2) or system (3) fault, never a test failure, "the same
distinction Playwright draws between a clear setup error and a cryptic test
failure". Changing it needs a superseding ADR, not a bug fix.

**Three of the report's supporting claims do not survive checking**, recorded so
they are not re-litigated:

- *"teardown already has a distinct code; setup collapses both into one"* —
  false. A teardown **assertion** failure exits 3, not 1: both phases map a test
  failure onto a non-test code (`phase_failed(…, UserError)` / `…, SystemError`).
  Neither distinguishes, by design.
- *"appears in nothing `explain`/`diff` consume"* — false for `explain`, which
  prints `failed (setup — excluded from the totals above)` with the assertion
  detail and the artifact reference; the events are in the record with
  `phase: setup`.
- *"previously raised, still open"* — no entry in this file matches it.

So the open item is narrow: **the phase reporters run only for the pool.** Worth
fixing at the reporter, not the exit code.

**Shipped** at exactly that boundary: the CI-report block (JUnit, GitHub job
summary, PR annotations) is one function both enders call, so a setup abort now
writes the reports from the setup phase's own summary — one testcase, failed,
suite named by the setup feature file. On `main` the gap was worse than filed:
no JUnit file was written *at all* (the finding said "empty"). Exit codes are
untouched, per ADR-0014. Nothing is fabricated for the pool that never ran —
the test pins that too.

---

## Open — round-7 residue (ingested 2026-08-10)

The round-7 pre-merge review of PR #13 never entered any worklist; a round-8
revalidation re-reproduced its findings against v0.8.0. §2.2, §2.3, §2.4 and the
`diff` item shipped in #30/#31. What remains, carried on that report's evidence
rather than re-reproduced here:

- **The early-error record** — *reproduced 2026-08-11, needs a decision.*
  `proef test --tags <nothing-matches>` prints the error and then a
  `summary: 0 passed · 0 failed · 0 skipped` line, and the record it leaves is
  `run_started` + `run_finished 0/0/0` — byte-indistinguishable from a clean run
  of an empty suite. A post-mortem reader cannot tell "errored before dispatch"
  from "ran nothing successfully".

  The fix is a design call, not a patch. Suppressing the tail on this path would
  leave the record *incomplete*, which the tooling already banners correctly —
  but `RunRecord` emits its tail structurally, on `Drop`, precisely so no return
  path has to remember it, and adding an exception reintroduces the fragility
  that design removed. Opening the record later is blocked by setup, whose
  scenario events need it. The third option is an additive event carrying the
  early error (ADR-0008 permits it) — the most honest and the most work.

## Open — residue of the two UX reviews

Verified against `main` on 2026-08-10. Everything else those reviews raised has shipped
(first-run: F1, F3, F4a and F2's did-you-mean in 0.6.0 · non-technical: N1–N5 and the
`init` count in #24).

### R1 — `missing_config_var`'s span points at the sentence, not the pack line

The diagnostic reports at the *feature* step that used the variable, e.g.
`suite/case.feature:3:5`, rather than the pack line where `${url:bse}` actually appears —
so the reader goes hunting. The did-you-mean half shipped in 0.6.0; this half did not,
**deliberately**.

*Why it was deferred, in full — this is the whole reasoning, do not re-derive it:*
`ResolveError` carries no position, and `resolve()` is documented "pure and total".
The comparable diagnostic that *does* land on a pack line (`pack::invalid_hurl`) gets
its position from hurl's own parser reporting a line/column, which feeds
`locate::payload_line_span(…, rel_line)`; nothing computes a `rel_line` for a resolve
failure. Supplying one means threading an offset out of a deliberately position-free
pure function and carrying pack identity to the diagnostic site. That is a design
change, not a fix — it wants its own spec.

Two sibling extensions were **declined** at the same time: `resolve::missing_env` must
not suggest from the injected environment snapshot (it would surface unrelated
environment variable names in diagnostics, against the secret-masking posture), and
`resolve::unknown_namespace` already enumerates all seven valid namespaces. Sibling
codes share a *shape*, not a *candidate set*.

### R3 — the scaffold default is the dev fixture's port *(declined 2026-08-11)*

`init.rs` writes `base = "${env:PROEF_BASE_URL:-http://127.0.0.1:8787}"`, which is
proef's own dev fixture port — so to someone who installed a binary and has no
fixture, the value *looks* configured and is not. The proposal was an obvious
placeholder (`https://api.example.com`) to cover **prevention**, since a failing run
already covers recovery.

**Declined, with the reasoning recorded rather than a silent skip.** Recovery is now
covered on *both* halves: an unreachable target and untouched routes each get their
own note (#28, #38). The remaining benefit is that the config file would read as
obviously unfilled. Against that, `init.rs`'s module doc states the scaffold
deliberately mirrors what `GETTING-STARTED` teaches — so changing the literal changes
the tutorial too, and the tutorial's "run it against `xtask fixture` with no
`PROEF_BASE_URL`" flow stops working. That flow is a real onboarding asset for
contributors. Trading a working tutorial for a more obviously-fake string is not worth
it once the failure itself explains both halves.

Revisit if first-run drop-off is ever measured rather than reasoned about.

### Decided against — do not re-raise

Recorded as decisions, so they are not rediscovered as fresh ideas.

- **Re-classify the unconfigured-scaffold failure from exit 3 to exit 2.** Not a
  CLI-edge change: the verdict is set in `proef-engine-hurl` (`classify_error`'s
  `_ => Infra` arm), `Fault::System(String)` carries no kind to match on, and the exit
  derives in `proef-core` (`RunSummary::exit_code_excluding`). Both routes — string-
  matching the engine's opaque message, or adding a structured kind to core's public
  surface — cost more than the value, which is *vocabulary*. The note delivers that, and
  fires on the exit-1 placeholder-route path a re-classification would have missed.
- **Degrade `proef flows` the way `macros` degrades.** `flows` promises *every*
  scenario; a list silently omitting the feature that failed to parse is a wrong answer,
  not a degraded one. `macros` degrades safely only because pack loading precedes
  binding and does not depend on it.
- **Ship `proef-fixture` in the binary so the scaffold's first run passes.** Needs a new
  ADR (it is dev-only today), enlarges the binary and the security posture of a test
  runner with a listening server — and R3 plus #24's note remove the need.
- **A GUI, web UI, or "no-terminal" mode.** PRD §3 forecloses dashboard/server mode. The
  P1 gap was always about *vocabulary and error text*, never a second interface.
- **Importing or round-tripping hand-written hurl**, and **anything OpenAPI-shaped as a
  recurring oracle.** PRD §3 and ADR-0016 permanent non-goals.

---

## Open — correctness

Q2 was the remaining Tier 1 branch (Q5 and Q4 shipped in #26); it closed with the #146 analysis cache — see below.

### Q2 — the walk still happens twice per request *(closed 2026-09-02)*

**Shipped in #27:** the walk skips `target/`, `node_modules/`, `vendor/` and
dot-directories, is depth-bounded, and no longer aborts the whole discovery on
one unreadable subdirectory (which `analyze.rs` swallowed into a silently empty
analysis). **Shipped in #32:** the server adopts the workspace root the client
announces — `workspaceFolders`, else `rootUri`, else the previous
config-then-cwd resolution — so an editor launched outside the project no longer
analyses the wrong tree.

**Closed 2026-09-02 — by the #146 analysis cache, which this entry predated.**
The premise ("on every completion/definition/references request") is no longer
true: every request handler reads one cached `Analysis` through the single
read path (`server.rs` — "the debounced diagnostics publisher and every
on-demand feature go through here; they share one recompute per edit rather
than one each"), edits mark the suite dirty behind a debounce, and the
fragment corpus is held across recomputes ("called when a fragment file
changes, never per request" — `analysis.rs`). The invalidation hook this entry
said `SourceProvider` lacked turned out not to be needed: the whole analysis
is invalidated on any edit, which at this suite scale (tens of small files,
milliseconds per recompute) beats maintaining an incremental index — the
module doc says so in as many words. The *two* walks inside one recompute
remain, and are now a per-edit cost too small to file.

### P5 — watch: the atomic-save half *(remainder)*

**Shipped in #37:** `--watch` now also watches `proef.toml`, matched by exact path.

**~~Closed by inspection~~ — the inspection was invalidated by a later change, and
the bug shipped.** The original argument was: the retrigger filter is an allowlist
of `.feature`/`.yaml`/`.yml`, and no run-record file (`.jsonl`, `.log`, `.hurl`,
`.vars`, `.json`, `.xml`, `.html`) matches it. ADR-0018 then added the engines'
fragment extensions to that allowlist — `.hurl`, named in this very paragraph as
the thing that could not match — while every run writes
`.proef-runs/<id>/artifacts/*.hurl`. A watched tree containing its own runs dir
fed itself: **49 runs in 15 seconds**, firing real traffic in a tight loop.

**Now closed by construction, not inspection.** The retrigger filter excludes
generated trees **by directory name**, reusing discovery's own `skipped_dir`, so
there is one rule with two consumers rather than a second list to drift; the
configured `[run] runs-dir` is passed in for the case where it is not a
dot-directory. `watch::tests` pins both halves — that an emitted artifact never
requeues, and that a fragment edit still does.

The lesson is the general one: a "closed by inspection" note records a conclusion
whose premise nothing watches. This one even enumerated the fact that later became
false. Prefer a test that would fail when the premise changes.

**Still open:** "a single watched file dies after an atomic save". It did **not**
reproduce on macOS/FSEvents; `notify`'s own docs say it is real but
platform-dependent and worst on inotify. Do not chase it on a Mac — that is how it
gets "fixed" by coincidence. It needs a Linux reproduction first.

---

## Open — adoption and execution model (ingested 2026-08-11)

Source: a report written while porting a real 844-line hurl corpus onto proef —
field evidence rather than inspection, which is why it found a different class
from the review rounds. Every claim below was re-checked against `main` before
filing; where the report was wrong, the correction is recorded with the item.

**Already closed from it:** the docstring-placeholder documentation gap (#41).
Two of its claims did not survive checking, and both are noted in place (M1, D2).

**The through-line.** These are *adoption*, not correctness. The first-run path
is finished and the correctness series closed its bug class; the next constraint
is whether a team with an existing hurl suite can move onto proef and
**demonstrate** they lost nothing. M1 and M2 are that story. E1 is the first wall
a real suite hits afterwards.

### F1 — `proef.toml` now has two path-resolution rules *(closed — duplicate of shipped R11-1)*

`[run] fragments` resolves relative to the **config file's directory** (ADR-0018);
`suite`, `setup`, `teardown` and `runs-dir` stay relative to the **working
directory**. The reasoning that produced the new rule — the config is found by
walking *up*, so a path in a config three levels above must mean "relative to the
project" — applies verbatim to all five keys, and `setup`/`teardown`/`runs-dir`
are consulted on every run rather than only when a path was omitted.

Cost: one file with two semantics and no marker distinguishing them. A user with
`setup` and `fragments` in the same `proef.toml` gets one working from a
subdirectory and one not, and every future path key re-litigates the choice
against four precedents for the older rule.

**Not fixed here on purpose.** Changing the four existing keys is a behaviour
change for every project that already relies on cwd-relative resolution, which
is out of scope for the change that introduced the fifth. The fix is a single
`ProjectConfig::resolve_path` used by every path accessor, shipped deliberately
with a changelog note — recorded so it is a decision rather than an oversight.

**ADR-0018 (named hurl fragments) lands into this section — read it against these
items before assuming what it closes.** It lets a pack `ref:` a named entry in a real
`.hurl` file, so a corpus file is annotated once instead of transcribed, and stays
runnable under stock `hurl`. Item by item:

- **M1** is *not* closed and must not be built concurrently — both touch `fmt`
  discovery. ADR-0018 requires the opposite of M1 at one entry point (directory
  discovery must never sweep `.hurl` into the pack formatter) while leaving M1's actual
  ask untouched (an explicitly named `.hurl` may be canonicalized). Sequence them,
  either order, never at once.
- **M2** is *not* closed. ADR-0018's integration test runs one fragment both ways,
  which proves a file is dual-runnable; it does not compare two suites' result sets.
- **M3** is *unanswered and now overtaken*: the charter re-examination M3 asked for has
  happened (PRD §3 amendment) without the measurement it asked it to rest on. The
  amendment argues from the non-goal's own rationale instead, and says so. Measuring
  the port cost is still worth doing — it now informs priority rather than permission.

**Closed 2026-08-23 (premise false — the "not fixed here" above HAS since been
fixed, as shipped R11-1).** The exact fix this entry prescribed exists as
`ProjectConfig::resolve` (`config.rs:328-338`): every path-valued key routes
through it, its doc comment narrates this entry's story, and `CONFIG.md`
documents the one rule. This entry and R11-1 were the same finding filed twice.

### M1 — `fmt` cannot canonicalize a standalone `.hurl` *(closed — foreclosed by ADR-0018)*

**The report had this backwards** and it is worth recording why. It claimed `fmt`
*refuses* a file outside a pack, and proposed teaching it to accept `.hurl` as a
small plumbing change. `fmt` in fact accepted any file and rewrote it — two
defects fixed in #40, which now makes it refuse `.hurl` **correctly**, since
applying YAML block-location logic to hurl syntax would be nonsense.

So the item survives but changes shape: making it real means teaching `fmt` to
recognize a hurl file and run the block canonicaliser over the whole thing, with
no `hurl:` key to locate. That is a feature, not a flag.

**Why it still ranks first.** It is what converts M2 from clerical to mechanical,
and it is the cheapest unlock for the most valuable capability.

**Closed 2026-08-23, without building it.** This entry predates ADR-0018, which
was accepted with the opposite principle: *proef reads files it does not own,
so it must never write them — `fmt` refuses fragment files* (ADR-0018,
"proef never writes"; carried as a hard constraint in `CLAUDE.md`). Building M1
would diverge from an accepted ADR without a superseding one. And the goal M1
served no longer needs it: it existed to make M2 mechanical — canonicalize both
corpora, diff the text — but fragments removed the transcription M2 was
guarding, so there is no ported copy whose equivalence needs proving. The file
the backend team owns *is* what proef runs, pinned per-file by the both-runners
test. Reopening this requires a superseding ADR, not a feature request.

### M2 — no mechanical equivalence check between a hurl corpus and its proef port *(deferred — trigger named below)*

**Verified when filed** (diff now also accepts record dirs and `.jsonl` paths — R3-4/#65 — but still reads no hurl report); no
path reads a `hurl --report-json`, which the pinned hurl 8.0.1 does emit.

**Why it matters.** The safe way to adopt proef is to run both suites until the
new one is trusted. During that window nothing *proves* the two assert the same
things, so the equivalence gate degrades to a hand-maintained mapping table
reviewed once by a human — and that table is what a team's decision to delete
their old suite rests on.

**Scope.** Not the hurl-import non-goal in disguise (`PRD.md:42`). Import means
reading `.hurl` and *generating* Gherkin. This compares two **result** sets,
which is `diff`'s existing job with one more input format. The non-goal
forecloses a direction of data flow, not the ability to check your own work.

**Deferred 2026-08-23.** The urgency rested on transcription drift — a port
that could silently assert less than its original. ADR-0018 removed the
transcription: a migrating team annotates the corpus it already has, and the
same bytes run under stock hurl and under proef (`fragments.rs` pins it
per-file against the fixture). What remains defensible is a *results* diff for
the trust-building window when both runners run in CI side by side —
`diff`'s job with `hurl --report-json` as one more input. **Trigger:** the
first concrete migration that runs both runners and asks to compare outcomes
mechanically. Building a seventh input format ahead of a consumer is the same
mistake the CTRF deferral records.

### M3 — the port cost has never been measured *(closed — overtaken by ADR-0018)*

`PRD.md:42` makes hurl import a **permanent** non-goal, and that rests on
persona P3's "pastes between corpus and packs" (`PRD.md:57`) being cheap —
which nobody has measured. A 14-file, 844-line port is the first real datum
available. Recording the hours settles a recurring argument in one direction or
the other: cheap vindicates the non-goal with evidence instead of assertion,
expensive earns the charter a re-examination with numbers rather than opinion.

**Closed 2026-08-23.** The re-examination this measurement was meant to trigger
happened: ADR-0018 narrowed the non-goal to *generation* and rewrote P3's job
from "pastes between corpus and packs" to "annotates once" — the exact charter
change M3 said the numbers should decide. The two field data points stand
recorded (an 844-line/14-file corpus ported by raw paste at 100% coverage; a
97-entry corpus that chose annotation and stopped the paste port deliberately),
and no third answer would change a decision that has already been made and
shipped.

### E1 — no intra-run serialization primitive *(report B1)*

**Verified.** `TECH-SPEC.md:313` — scenario ordering is "preserved for artifact
naming, not execution order." No `serial` tag or config key exists anywhere in
core, cli, `CONFIG.md` or `AUTHORING.md`.

**Why it matters.** Real suites contain scenarios that mutate global state — the
reporting corpus has two, one needing an empty database for absolute `items[N]`
assertions and one installing a workflow definition governing everything created
afterwards. Neither can run in a parallel pool, and proef offers no way to say
so; the workaround is several CLI invocations driven by tag discipline in a
Makefile.

**Charter fit.** Scheduling, not a new engine or execution mode — the
orchestrator already decides what runs when, and `[run] setup`/`teardown` prove
the surrounding concept is in charter. Those cover *before* and *after* the pool
and nothing *inside* it.

**Options.** A reserved `@serial` tag, or `[run] serial-tags = [...]`. The config
form is more explicit and keeps runner semantics out of the feature files — and
E4 is an argument for it.

**Shipped** as `[run] exclusive-tags`, a tag *expression* rather than a list —
the same language `--tags` takes, so group membership is answered exactly as
selection is. Two corrections to this entry, both from checking before building:

1. The filing describes one axis; the mature shape has two. `cargo-nextest`
   separates a group concurrency limit (`max-threads`, which bounds members
   against each other and leaves the rest of the pool running) from per-test
   weight (`threads-required`, which is what buys global exclusivity — they
   redefined it in 2024 precisely so limits "are never exceeded", enabling
   mutual exclusion against *all* tests). Only the second is what was missing
   here, so only that shipped; a group table can be added later without
   breaking this key.
2. Of the two motivating scenarios, only the first is a serialization problem.
   "Installs a workflow definition governing everything created afterwards" is
   **ordering**, which `[run] setup` already provides — a feature run once
   before the pool exists. Recorded so an ordering primitive is not built on
   the assumption that it was needed.

### E2 — N invocations produce N run records, with no merge *(report B2; consequence of E1)*

**Verified.** Each run writes its own `.proef-runs/<run-id>/` (`TECH-SPEC.md:299`).

E1's workaround therefore yields N records, N JUnit files, N HTML reports, and
pass/fail aggregation pushed onto the caller's shell, while `explain`/`diff`
operate per-run so a post-mortem reader must know which to open. **Recorded as a
consequence, not an independent item** — solve E1 and this largely evaporates;
solving it alone (a `proef merge`) treats the symptom.

**Largely closed by E1 shipping:** a suite whose isolation needs are expressed
as `exclusive-tags` runs in one invocation, so it produces one record, one JUnit
file, one report and one exit code. Kept open rather than closed outright
because a suite may still split invocations for reasons E1 does not address
(different environments, different `--tags` in separate CI jobs), and nothing
merges those.

**Shipped (2026-08-25) — the rerun half:** `run_started.rerun_of` names the
base; the rerun's JUnit carries the base's not-re-run scenarios
(reconstructed from its record, exit code and totals untouched), and
`report` overlays the base into a whole-suite page with a merged-view
banner, degrading loudly when rotation ate the base. What remains of E2 is
the original split-invocation case (different `--tags` in separate CI
jobs), still open on its trigger.

**Widened by the RF audit (2026-08-24):** the class includes `--rerun`'s own
CI story, which this entry never named — a rerun writes a new record whose
JUnit/report contain only the re-run subset, so "the one JUnit at the end"
of the standard retry workflow describes 3 scenarios of a 300-scenario
suite. RF's answer is `rebot --merge`. The proef shape, when built: overlay
a rerun record onto its base at `report`/JUnit emission — composition over
records, never a merged record file (ADR-0008); an additive
`RunStarted.rerun_of` field would make records self-describing for it.

### E3 — no per-scenario state reset hook *(report B3)*

**Verified.** `[run] setup`/`teardown` are whole-suite only, run once around the
pool (`CONFIG.md:63-64`, `120-141`).

Any suite against a real database wants before-each; today isolation is
convention (title prefixes so scenarios do not see each other's rows) and
convention has no guardrail. proef knows nothing about databases, so "reset the
DB" cannot be a proef feature — but framed as *a feature file run before each
scenario* it is the same primitive as `setup` at a different scope, which is
engine-agnostic by construction. The cost is real: it multiplies run time by
scenario count and interacts with parallelism. **This needs an ADR against
ADR-0014, not a patch**, and it may well be declined — deliberately rather than
never asked.

### E4 — nothing enforces tag-group discipline *(report B4; record, do not build)*

If E1 ships as a tag convention, a scenario added six months later lands untagged
in the parallel pool and breaks isolation intermittently — the worst failure
mode, because it reads as flakiness. A lint would have to guess which endpoints
are global, which proef cannot know. Its value is as a marker: this is the
follow-on cost of the tag form of E1, and therefore an argument for the config
form.

### D1 — no first-class requirement traceability

**Verified.** `flows --output json` prints one object per scenario
(`main.rs:128-137`), which with tags like `@FRD-3.1-create` gets most of the way.
Almost certainly a **documented recipe rather than a feature** — proef should not
learn what a requirement is — but the recipe does not exist, so every team
reinvents it and the capability is not advertised for this use.

### D2 — report generation across N runs *(premise partly corrected)*

**The report overstated this.** It claimed a Makefile must capture the run id
because proef needs `proef report <run-id>`; in fact `run_id` is optional and
defaults to the latest run (`main.rs:199-201`), so the ordinary single-run case
needs nothing captured.

What survives is the compounding with E2: with N invocations, "the latest" is one
of N. Minor on its own, and listed because report-generation friction is felt by
every CI integration rather than by one team.

### Positive evidence — recorded so it is not undone

- **The raw-hurl paste path covered 100% of a real corpus.** All 844 lines used
  only `[Asserts]` (75) and `[Captures]` (15) — no `[Options]`, `[Query]`,
  `[FormParams]` or `[Cookies]` — with seven ordinary predicates (`==`, `exists`,
  `not exists`, `matches`, `count ==`, `>=`, `isString`), every one passing
  through untouched. **The strongest evidence yet for ADR-0004**, and the kind of
  claim that gets doubted later.
- **`proef macros` printing sentences (#29) is load-bearing.** The porting plan
  gated its prerequisite phase on it, purely to author 14 files of new prose.
- **`--rerun`** (`main.rs:120-122`, re-run only the last run's failures) fits
  conversion iteration exactly.

### Suggested order *(historical — every item now resolved or parked)*

The order was M1 → M2 (adoption becomes provable) → E1 (dissolves E2). E1
shipped as `[run] exclusive-tags`; M1 closed against ADR-0018; M2 is deferred
on a named trigger; M3 closed as overtaken. The two documentation items, C1
and C3, shipped in #43. E3, E4, D1 and D2 remain record-only — none blocks
anyone today.

---

## Closed — docs drift (2026-08-11)

Every item in this section shipped; the table above records which PR each landed in.
Two did not reproduce when re-checked, and are recorded here rather than dropped, so
the next reader does not spend the same time on them:

- **A2** — `CONFIG.md` was said to claim `[env.<name>.run]` overrides any section. It
  carries no such claim today: its precedence text names `jobs` specifically, which is
  what `RunOverride` actually allows.
- **B12** — the CHANGELOG's 0.5.2 entry was said to lack a line about the
  directory-valued-phase hard error. It has one, first bullet under **Fixed**.

One half of **A5** was deliberately not acted on: TECH-SPEC §11's run-dir inventory
lists the files a run *generates*, and the `[run] setup`/`teardown` features are inputs
named by config, not run-dir output. The reviewer called this half "defensible-but-
interpretive" and it is; `report.html`, which the inventory genuinely omitted, was added.

---

## Open — maintainability and CI

| ID | Finding |
|---|---|
| B10 | The canary would chase a hurl **prerelease** (no semver filter) — *shipped: the index parse (`latest_stable_in_index`) skips `-` versions, unit-pinned; build metadata needs no rule, crates.io refuses versions differing only by `+meta`* |
| P12 | The matcher re-tokenizes per `(step, pattern)` pair on every bind *(performance)* |
| P13 | No World snapshot/restore proptest; no CI workflow runs `llvm-cov` |
| Q1 | ~~structured payloads unreachable~~ *(premise false 2026-08-23: they parse, validate through the engine seam, lower and skip the hurl emitter — pinned by tests; `EngineLowering` was a review's name, never a symbol)* — what survives: no *registered* engine claims a structured kind, so the path runs only under test fixtures |
| Q6 | ~~`html.rs` re-derives the emitter slug; four `file_stem()` sites~~ *(closed 2026-09-02 — the count was exactly right, four production sites, and the fix is structural rather than descriptive: `emit::feature_stem` and `emit::artifact_slug` are now the one definition of each, called by the emitter's own caller, the dispatcher's spec naming, the report's anchors/artifact links, and the editor analysis. The other premise had gone stale the other way: `ScenarioOutcome.artifact_slug` has carried the emitter's naming to runtime consumers since round 19, so "the schema carries no slug" no longer forced anyone to re-derive)* |

---

## Open — deferred during the v0.6.0–v0.8.0 correctness series

Found while fixing the above; each was validated and consciously left out of scope.

- **`proef-harness` `PROEF_BIN`/`PROEF_HARNESS_SUITE`** — fixed in #19, but the same
  reader is now duplicated in `proef-cli` and `proef-harness`. Justified today (a binary
  crate cannot be depended on; these are the only two `env::var` callers in the tree).
  **Tripwire: at a third caller, promote it to a shared crate.**
- **Capture-name charset** is narrower than hurl's grammar, so an out-of-charset name is
  silently omitted from `.map.json`.
- **A `#` comment inside a `[Captures]` run** — fixed in #16; the one/two-letter-method
  gap it exposed remains (`is_method_line` requires three characters, hurl's grammar does
  not).
- **`key_line_spans`' flow-style undercount** is guarded by convention, not types. Two
  callers guard it independently; a third would have to remember. Cheap hardening: have
  the primitive return a reliability flag.
- **Cross-scenario `${fake:*}` coincidence** — two scenarios can still draw the same
  value. Documented as a known limitation in AUTHORING/CHANGELOG/TECH-SPEC.
- **No corpus tier for engineered robustness fixtures.** `tests/` has zero custom-method
  entries and zero fenced blocks, so that bug class is pinned only by unit tests on
  private functions.
- **`fmt`'s tie-break** (equal CRLF/LF → LF) now applies only to the trailing newline
  of a file that lacked one — per-line endings are preserved (#33). Lone-`\r` files are
  still unhandled: the splitter keys on `\n`, so a classic-Mac file is one long line.
- **`normalize_pack` keeps the skeleton verbatim by construction at each `push`, not
  by the algorithm's shape.** The "hurl blocks only" promise has broken three times
  (#18 line endings, #33 mixed endings, #40 trailing whitespace), each caught by an
  example pinning that one instance. #44 added properties — skeleton-only text round-trips
  byte-for-byte, and formatting is a fixed point — so a fourth over-reach now fails CI
  instead of shipping. The structural version would locate each block's byte span and
  splice the canonicalized body back into the original text, making "bytes outside a
  span are never visited" a property of the shape. **Not worth the rewrite for a small
  textual formatter; revisit if a fourth normalization rule is ever added to that loop.**
- **The stdout latch's single-reader test isolation** is safe under the mandated nextest
  (one process per test) but is a convention, not an enforced invariant.
- **A disk filling mid-run** still truncates the human console report without reaching the
  exit code: `ConsoleReporter` drops write errors. A disk already full at start *is*
  caught. Closing it needs a `note_stdout_failure()` in `Tee::write` when the console is
  stdout — no core change.
- **Absent-secret fallthrough** ("an unset `PROEF_SECRET_<NAME>` still reads the store")
  is load-bearing and pinned only by an integration test, not a unit test.
