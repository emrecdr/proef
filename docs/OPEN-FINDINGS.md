# proef — open findings

**Provenance:** an external review of v0.5.3, validated claim-by-claim against the tree
on **2026-08-06** (40 claims → 38 confirmed, 1 partial, 1 already fixed). Every item
below was reproduced live or verified in code at that time. The shipped/open split was
re-checked against `main` on **2026-08-10**.

**Companion docs:** [IMPROVEMENT-PLAN](IMPROVEMENT-PLAN.md) is the *feature* roadmap
(competitive analysis, N-items) — a different list with different numbering. This file
is the *defect* list. [CHANGELOG](CHANGELOG.md) records what shipped.

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

**Q7 partially addressed** (#16): `fuzz_tag_expr` is now compiled by the gates job and
its status is documented in [TESTING-STRATEGY](TESTING-STRATEGY.md) — but it is still
listed in neither fuzz loop (`ci.yml:127`, `nightly.yml:84` both name three targets), so
nothing fuzzes it. Verified 2026-08-10.

---

## Open — correctness

Two of these are the remaining Tier 1 branches. They are the highest-value work left.

### Q5 — Ctrl-C skips teardown silently *(blocked: needs an ADR-0014 decision)*

On SIGINT the teardown phase runs with the **already-cancelled** token, so every teardown
scenario resolves `Skipped`, and `phase_failed` ignores that — the operator gets **zero
message** and cleanup never ran. Live SIGINT repro.

This is a policy question before it is a code change: ADR-0014 says cleanup is reliable.
Either teardown gets a fresh child token so cleanup happens on cancel, or it stays
skipped-but-announced. **The ADR needs amending either way** — it is silent on
cancellation, so today's behaviour is unspecified rather than wrong-by-the-spec.

### Q4 — `--dry-run` validates neither phase

`--dry-run` validates neither `[run] setup` nor `[run] teardown`, contradicting ADR-0014
verbatim ("validated like any other feature but never executed"). Worse asymmetry: a bad
*teardown* path runs the whole suite first and then exits 3, where a bad *setup* path
fails in 0.03s. Live, both halves. Pairs naturally with Q5 as one branch.

### Q2 — LSP roots at process cwd *(Tier 1, unspecced)*

`proef lsp` ignores `initialize`/`rootUri` and roots at the process working directory;
`walk_dir` has no `target/`/`.git` excludes and the code comment admits a full walk per
request. So `nvim ~/proj/x.feature` launched from `$HOME` roots at `$HOME` and crawls the
home tree on every request. Live JSON-RPC repro. *Wrong answer + resource consumption.*

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

## Open — security-adjacent

### B7 — `secret set --value` is argv-visible

The value appears in the process list, and the error text steers users toward it rather
than stdin.

---

## Open — docs drift

All confirmed 2026-08-06; **A5, A4, A6 re-verified still open on 2026-08-10.**

| ID | Finding |
|---|---|
| A1 | `EDITORS.md` v1-limitations section is stale (match-line jump shipped in 0.5.1) and omits the real gap: builtin macros have no jump target and no hover |
| A2 | `CONFIG.md` claims `[env.<n>.run]` overrides any section; `RunOverride` is jobs-only with `deny_unknown_fields` (`config.rs:81-86`), pinned by `env_run_rejects_non_jobs_overrides` (`config.rs:423`). Re-verified 2026-08-10 |
| A4 | README's `test` row and TECH-SPEC §10 omit `--rerun`/`--sarif`/`--run-id`; `tap` is absent from `--output`; README says "ADR-0001…0011" against **17** actual ADRs |
| A5 | TECH-SPEC §2 still says `publish = false` (four crates publish); §11's run-dir inventory omits `report.html` and the phase features |
| A6 | TROUBLESHOOTING's exit table omits **130** (the second-Ctrl-C hard exit) — zero mentions in the file |
| A7 | `GETTING-STARTED.md:144` has a stale comment after `suite = "suite"` |
| B12 | The CHANGELOG's 0.5.2 entry lacks a line acknowledging the directory-setup hard error |
| P11 | `ScenarioFinished.worker` is always `None` by design; EVENTS.md is accurate, ADR-0015's text is stale |

---

## Open — maintainability and CI

| ID | Finding |
|---|---|
| B3 | `windows.yml` builds and tests without `--locked` |
| B5 | `explain`/`diff`/`report` each inline `ProjectConfig::load()` instead of the shared `load_config()` — three copies |
| B10 | The canary would chase a hurl **prerelease** (no semver filter) |
| B13 | The `justfile` gate list omits `public-api`, which CONTRIBUTING requires |
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
- **`fmt`'s tie-break** (equal CRLF/LF → LF) is correct but undocumented and unpinned;
  lone-`\r` files are unhandled by the `.lines()`-based design (pre-existing).
- **The stdout latch's single-reader test isolation** is safe under the mandated nextest
  (one process per test) but is a convention, not an enforced invariant.
- **A disk filling mid-run** still truncates the human console report without reaching the
  exit code: `ConsoleReporter` drops write errors. A disk already full at start *is*
  caught. Closing it needs a `note_stdout_failure()` in `Tee::write` when the console is
  stdout — no core change.
- **Absent-secret fallthrough** ("an unset `PROEF_SECRET_<NAME>` still reads the store")
  is load-bearing and pinned only by an integration test, not a unit test.
