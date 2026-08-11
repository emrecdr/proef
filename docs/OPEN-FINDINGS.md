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

The review documents themselves were removed once their open items landed here; their
full text, transcripts and citations are in git history (`git log --diff-filter=D
-- docs/FIRST-RUN-UX-REVIEW.md docs/NON-TECHNICAL-UX-REVIEW.md`). The shipped/open split
was re-checked against `main` on **2026-08-10**.

**Read the citations as "start reading here", not as addresses.** They were accurate on
2026-08-06 and files have moved since; locate symbols with `rg`, not line numbers.

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

**Q7 is now closed** (#30): `fuzz_tag_expr` is in both fuzz loops as well as the
compile gate.

---

## Open — round-7 residue (ingested 2026-08-10)

The round-7 pre-merge review of PR #13 never entered any worklist; a round-8
revalidation re-reproduced its findings against v0.8.0. §2.2, §2.3, §2.4 and the
`diff` item shipped in #30/#31. What remains, carried on that report's evidence
rather than re-reproduced here:

- **The early-error record** writes `Completed 0/0/0` and a `summary:` line after
  the error.
- **The dry-run nudge** drops `--env`/`--tags` from the command it echoes, so the
  printed command is not the one that was run.
- **`Warned` scenarios** are dropped from truncated-record fallback totals.

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

### R3 — the scaffold default is still the dev fixture's port

`init.rs` writes `base = "${env:PROEF_BASE_URL:-http://127.0.0.1:8787}"`, and 8787 is
proef's own dev fixture port (`xtask fixture`, ADR-0011 amendment) — so to anyone who
installed a binary and has no fixture, the value *looks* configured and is not. A
failing run now says the suite is still the untouched scaffold (#24), which covers
**recovery**; an obvious placeholder such as `https://api.example.com` would cover
**prevention**. The cost is not the literal but its documentation blast radius:
`GETTING-STARTED` ×2, `CONFIG`, `TROUBLESHOOTING` ×2, and the documented dev loop.

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

Q2 is the remaining Tier 1 branch. Q5 and Q4 shipped in #26.

### Q2 — the walk still happens twice per request *(remainder)*

**Shipped in #27:** the walk skips `target/`, `node_modules/`, `vendor/` and
dot-directories, is depth-bounded, and no longer aborts the whole discovery on
one unreadable subdirectory (which `analyze.rs` swallowed into a silently empty
analysis). **Shipped in #32:** the server adopts the workspace root the client
announces — `workspaceFolders`, else `rootUri`, else the previous
config-then-cwd resolution — so an editor launched outside the project no longer
analyses the wrong tree.

**Still open.** `analyze.rs` discovers packs and features with two independent
walks, on every completion/definition/references request, and completion
requests are not debounced. The excludes cut what each walk costs; they do not
stop it happening twice. Halving it means either caching per analysis pass or a
combined discovery call — both need a way to invalidate, which the
`SourceProvider` trait has no hook for today. Purely a cost item now that the
root is correct: no wrong answers depend on it.

### B2 — templated `retry:`/`delay:` under-count the batch budget

Code-verified. A templated value is not counted toward the budget the way a literal is,
so a pack can exceed the intended ceiling.

### B4 — `--output json` serializes `exit_code` before the JUnit escalation

The JSON body can carry a verdict the process then escalates past, so the embedded
`exit_code` and the real exit disagree.

### P3 — `--sarif` emits no `startLine`

Only `byteOffset`/`byteLength`, which defeats inline GitHub annotations. Repro'd.
*Feature-defeating.*

### P5 — watch gaps *(partial)*

`proef.toml` is unwatched, and the runs-dir self-triggers — both reproduced. The
"single file dies after an atomic save" half did **not** reproduce on macOS/FSEvents;
`notify`'s docs confirm it is real but platform-dependent, worst on inotify.

### B6 — LSP completion snippets do not escape `$` / `\`

A macro name containing either produces a malformed snippet.

### B9 — GitHub annotation `file=` unescaped; `|` unescaped in the job-summary table

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
| B10 | The canary would chase a hurl **prerelease** (no semver filter) |
| P12 | The matcher re-tokenizes per `(step, pattern)` pair on every bind *(performance)* |
| P13 | No World snapshot/restore proptest; no CI workflow runs `llvm-cov` |
| Q1 | `EngineLowering` seam: structured payloads are **unreachable** today (pack validation rejects them first) — design debt, zero live misbehaviour |
| Q6 | `html.rs` re-derives the emitter slug (the event schema carries no slug field); `exec.rs`'s own comment forbids exactly this. Four `file_stem()` sites — correct today, future-drift risk |

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
- **The stdout latch's single-reader test isolation** is safe under the mandated nextest
  (one process per test) but is a convention, not an enforced invariant.
- **A disk filling mid-run** still truncates the human console report without reaching the
  exit code: `ConsoleReporter` drops write errors. A disk already full at start *is*
  caught. Closing it needs a `note_stdout_failure()` in `Tee::write` when the console is
  stdout — no core change.
- **Absent-secret fallthrough** ("an unset `PROEF_SECRET_<NAME>` still reads the store")
  is load-bearing and pinned only by an integration test, not a unit test.
