# proef v0.5.2 — CLI correctness fix pass (design)

**Goal:** Fix the six validated §3 findings from the external v0.5.0 review — four
genuine correctness bugs in the run-diff / suite-setup / diagnostic-render paths,
one trivial overflow-hardening, and one exit-code documentation gap — so
`proef diff`, `[run] setup`, and diagnostic rendering behave correctly under
partial runs, duplicate steps, directory misconfiguration, and closed pipes.

**Architecture:** Bug-fix pass entirely in `proef-cli` (`record.rs`, `diff.rs`,
`exec.rs`, `watch.rs`, `render.rs`) plus docs. `proef-core` is untouched — its
sans-IO purity and the event schema (ADR-0008, already carries `RunFinished` +
`cancelled`) are unchanged. No new dependencies. Each fix ships a regression test
that genuinely fails without the fix (the v0.5.1 pass caught a vacuous test — that
bar holds here).

**Tech stack:** Rust 2024; existing `assert_cmd`/`tempfile` test harnesses; the
`proef_core::event::Event` schema (unchanged).

**Branch:** `fix/cli-correctness-p1` off `main` (5c36a5e = v0.5.0). Independent of
PR #4 (goto-def gaps) and PR #5 (v0.5.1 LSP fixes) — different files, merges in any
order. Ships as **v0.5.2** after v0.5.1 lands (own PR now; tag/publish is the
separate RELEASING step).

---

## Verified facts (re-confirmed against the current tree before design)

Per the standing directive, every finding was re-read at its current file:line and
re-confirmed — line numbers below are from the live code, not the pre-compaction
review.

- **§3.1 collision — in-memory only, no schema change.** `record.rs:87-96`:
  `read_record` folds each `StepFinished` into `pending[(file,scenario)]` as a
  `BTreeMap<String, StepRun>` keyed by `step.text` — `.insert(step.text, …)` is
  last-write-wins (doc comment at `record.rs:53` even says so). The persisted
  `events.jsonl` carries every step individually; the collision is purely in this
  folded structure. `diff.rs` consumes it by text (`note_flaky` :150, `note_slower`
  :166). Fix touches `record.rs` + `diff.rs`, not the event schema.
- **§3.2 truncated/cancelled — schema already supports the fix.** `Event::RunFinished`
  is the tail event and carries `cancelled: bool` (additive, default false). A
  *complete* run ends with `RunFinished{cancelled:false}`; a *cancelled* run with
  `cancelled:true`; a *truncated/died* run has **no `RunFinished` at all**.
  `read_record` (`record.rs:78-110`) only reads `StepFinished`/`ScenarioFinished`
  — it never inspects `RunFinished`, so `diff` cannot tell a partial run from a
  complete one. `diff.rs:138-142` classifies base-keys-missing-from-new as
  `removed` (benign); `fail_on_regression` (`diff.rs:56`) only checks
  `report.regressed`. So a truncated/cancelled new run → its missing scenarios look
  `removed` → **exit 0**. No schema change needed; `diff` just has to read
  completion.
- **§3.3 overflow — trivial.** `note_slower` (`diff.rs:164-179`) sums durations with
  raw `+=` (`:167-168`) and compares with raw `*` (`:173`). `saturating_add`/
  `saturating_mul`. Low severity; harden while in the file.
- **§3.4 setup double-run — contract vs. code mismatch.** ADR-0014 defines
  `[run] setup`/`teardown` as "**a feature file**" (singular, throughout — never a
  directory). But `run_phase` → `front::run` → `discover_features`
  (`front.rs:214-235`) accepts a directory (recurses for `.feature` files). So a
  directory-valued setup runs every feature under it as the setup phase; then
  `exclude_phase_features` (`exec.rs:613-631`) — which excludes by exact canonical
  *file* path — cannot exclude any of them (a directory path never equals a feature
  file path), so each also runs in the pool → **twice**. No validation rejects a
  directory (`front.rs` only errors on neither-file-nor-dir, or a dir with zero
  features).
- **§3.5 EPIPE — stderr path unguarded.** `render.rs:40-48` `print_all` renders
  diagnostics with raw `eprintln!("{report:?}")`, which panics on `BrokenPipe`
  (exit 101, reproduced). The `outln!` macro (`render.rs:13-23`) already guards
  `BrokenPipe` — but only for **stdout**. The diagnostic path writes stderr and
  has no guard.
- **exit-130 — outside the typed contract, undocumented, duplicated.** The
  `ExitCode` enum (`proef-core/src/error.rs`) is only `Success=0 / TestFailure=1 /
  UserError=2 / SystemError=3`. `130` is a raw `std::process::exit(130)` at **two**
  sites — `exec.rs:179-180` and `watch.rs:60-61` (identical literals, not a shared
  helper) — on the *second* Ctrl-C (128+SIGINT convention). TECH-SPEC §10
  (`TECH-SPEC.md:266-267`) documents only 0/1/2/3; `cli.rs` asserts 0/1/2/3, none
  pins 130; ADR-0009 never mentions signal codes.

---

## Design

### Fix 1 — §3.1 disambiguate same-text steps (record.rs + diff.rs)

The step identity for diffing becomes `(text, occurrence-ordinal-within-scenario)`
— the ordinal counts prior same-text steps as they stream in, so the Nth `"GET /x"`
in the base run pairs with the Nth `"GET /x"` in the new run. Deterministic, stable
across line edits (still text-based), and lossless.

`ScenarioRun.steps` changes from `BTreeMap<String, StepRun>` to
`BTreeMap<(String, usize), StepRun>` (key = `(text, ordinal)`). In `read_record`,
maintain a per-scenario per-text occurrence counter while folding `StepFinished`:

```rust
// occurrence ordinal per (scenario, text) as steps stream in
let mut seen: BTreeMap<(String, String), usize> = BTreeMap::new();
// ... in the StepFinished arm:
let key = (step.file.to_string(), scenario.to_string());
let ord = { let c = seen.entry((scenario.to_string(), step.text.to_string())).or_insert(0);
            let n = *c; *c += 1; n };
pending.entry(key).or_default().insert((step.text.to_string(), ord), StepRun { attempts, duration_ms });
```

`diff.rs` `note_flaky`/`note_slower` iterate `new.steps` (now `(text, ord)` keys)
and look up `base.steps.get(&(text.clone(), ord))`; the rendered message still shows
just `text` (the ordinal is identity, not display). Test: a scenario with two
identical-text steps whose `attempts`/`duration_ms` differ → both are retained and
diffed independently (pre-fix the second overwrites the first).

### Fix 2 — §3.2 don't pass the gate on an incomplete run (record.rs + diff.rs)

`read_record` returns a richer type so its callers learn completion in the same
single pass:

```rust
pub enum RunCompletion { Completed, Cancelled, Incomplete } // Incomplete = no RunFinished
pub struct Record {
    pub scenarios: BTreeMap<(String, String), ScenarioRun>,
    pub completion: RunCompletion,
}
pub fn read_record(dir: &Path) -> Result<Record, String>;
```

While folding events, track completion: default `Incomplete`; on `RunFinished{cancelled}`
set `Cancelled` if `cancelled` else `Completed`. `failed_scenarios` (the other
caller, used by `--rerun`) switches to `read_record(dir)?.scenarios`.

`diff`:
- **Always** (with or without `--fail-on-regression`) prints a visible banner when
  either record is not `Completed`, e.g.
  `⚠ new run <id> is INCOMPLETE (no RunFinished) — results may be partial` /
  `⚠ … was CANCELLED`. So a human reading the diff is never misled.
- Under **`--fail-on-regression`**: if the **new** run's completion is not
  `Completed`, return `ExitCode::TestFailure` (exit 1) with a message that
  distinguishes "incomplete/cancelled run — cannot certify no regressions" from an
  actual regression. (An incomplete/cancelled run must never pass a CI gate.) The
  base run being incomplete is banner-only (you can still detect regressions against
  a partial baseline; the gate protects the *new* result).

Test: a new-run record whose `events.jsonl` omits `RunFinished` (or has
`cancelled:true`) → `diff --fail-on-regression` exits 1 and prints the banner;
without the flag it prints the banner and exits 0.

### Fix 3 — §3.3 saturating duration math (diff.rs)

`note_slower`: `base_ms = base_ms.saturating_add(...)`, `new_ms.saturating_add(...)`,
and the ratio comparison uses `saturating_mul`. Behavior-identical for real
durations; overflow-safe for a corrupted `events.jsonl`. Covered incidentally by the
existing slower-detection tests; no dedicated test (it's hardening, not a behavior
change).

### Fix 4 — §3.4 reject a directory-valued setup/teardown (exec.rs)

In `run_phase`, before running the phase, reject a directory-valued path as a
`UserError` (ADR-0014: setup/teardown is exactly one feature file). This resolves
the runner/exclusion inconsistency at the contract boundary — loudly, instead of
silently double-running — and preserves every single-file case:

```rust
// ADR-0014: [run] setup/teardown names one feature file, not a directory.
// A directory would run every feature under it as the phase AND leave them in
// the pool (exclude_phase_features matches one file path), running each twice.
if path.is_dir() {
    eprintln!("error: [run] {phase} must be a feature file, not a directory ({})", path.display());
    return Err(ExitCode::UserError);
}
```

(`phase` is the "setup"/"teardown" label `run_phase` already carries.) Test:
`proef test` with `[run] setup = "<a dir>"` → exit 2 with that message; a single-file
setup still runs once and is excluded from the pool (existing behavior, pinned).

### Fix 5 — §3.5 guard the diagnostic stderr write (render.rs)

Add an `errln!` macro mirroring `outln!` (BrokenPipe swallowed, other errors are
best-effort) and use it in `print_all`:

```rust
macro_rules! errln {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        // Diagnostics go to stderr; a closed reader (`proef … |& head`) must end
        // the pipeline quietly, never a 101 panic — mirror outln!'s stdout guard.
        let _ = writeln!(::std::io::stderr(), $($arg)*);
    }};
}
// print_all: errln!("{report:?}") instead of eprintln!
```

(`writeln!` to `stderr` returns the same `io::Result`; swallow `BrokenPipe`, and —
matching `outln!` — a genuinely-broken stderr has nowhere useful left to report to,
so drop quietly.) Test (assert_cmd): a command that emits diagnostics, piped to a
reader that closes early, exits with the normal error code, never 101.

### Fix 6 — exit-130 documentation (docs + optional dedupe)

Document the second-Ctrl-C hard exit as a deliberate escape hatch *outside* the
typed 0/1/2/3 taxonomy (the 128+SIGINT shell convention), covering both `proef test`
and `proef watch`:

- **TECH-SPEC.md §10** (extend the exit-codes sentence at ~266-267): note that a
  second interrupt (Ctrl-C) forces an immediate hard exit with code **130**
  (128+SIGINT), by convention, bypassing the graceful 0/1/2/3 taxonomy.
- **ADR-0009**: a one-line note that 130 is the sanctioned OS-signal hard-abort code,
  intentionally not an `ExitCode` variant (it is not a graceful outcome).
- **CHANGELOG** `[Unreleased]`.
- **Optional dedupe:** replace the two `std::process::exit(130)` literals
  (`exec.rs`, `watch.rs`) with a shared `const INTERRUPT_EXIT_CODE: i32 = 130;` (and
  a one-line doc) so the documented code and the literal can't drift. Keep it a
  `const`, not an `ExitCode` variant — 130 is a signal convention, not a taxonomy
  member.

---

## Testing strategy

Per TESTING-STRATEGY (assert observable outcomes / normalized order, never
wall-clock): every code fix ships a test that fails without it.

- **§3.1:** a synthetic `events.jsonl` (or a fixture run) with a scenario carrying
  two identical-text steps with differing `attempts`/`duration_ms`; assert both are
  present in the diffed `ScenarioRun` and both surface in flaky/slower detection.
  (Unit test over `read_record`/`diff` — no network.)
- **§3.2:** two synthetic records — a complete base and a new one missing
  `RunFinished` (and a second case with `cancelled:true`); assert
  `diff --fail-on-regression` exits 1 + banner (via assert_cmd), and the plain
  `diff` exits 0 + banner.
- **§3.4:** assert_cmd `proef test` in a tempdir with a `proef.toml` whose
  `[run] setup` is a directory → exit 2 + the message; a companion single-file setup
  case still runs once (guard the fix doesn't break the file path).
- **§3.5:** assert_cmd — a diagnostic-emitting invocation with stdout/stderr piped to
  a reader closed early; assert exit ≠ 101 (the normal error code) and no panic.
- **§3.3 / exit-130:** §3.3 is covered incidentally (hardening); exit-130 is docs
  (no test, but note it stays deliberately un-pinned in assert_cmd since forcing a
  double-SIGINT in a test is fragile — the docs are the deliverable).

---

## Global constraints

- `proef-core` untouched; sans-IO preserved; event schema (ADR-0008) unchanged
  (`RunFinished`/`cancelled` already exist — we only *read* them).
- Exit codes are a contract (ADR-0009, assert_cmd-pinned): the §3.2 gate returns the
  existing `TestFailure=1`; §3.4 returns the existing `UserError=2`; 130 stays a
  documented signal-convention escape hatch, not a new enum variant.
- hurl pins `=8.0.1` untouched; no new dependencies.
- No task ids / plan numbers in code comments (changelog only); no AI-attribution
  commit trailers.
- Each fix ships a genuinely-discriminating regression test.
- Ships as **v0.5.2** (patch — bug fixes + docs).
- Gate every task: `cargo fmt --all --check`; `cargo clippy --all-targets
  --all-features -- -D warnings`; `cargo nextest run --profile ci`;
  `cargo test --doc`; `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
  --all-features --workspace`; `cargo run -p xtask -- docs-check`.

---

## Task breakdown (preview for the plan)

1. **§3.4 reject directory setup/teardown** (exec.rs) — smallest, self-contained;
   establishes the `run_phase` guard + test.
2. **§3.5 errln! stderr guard** (render.rs) — self-contained macro + assert_cmd test.
3. **§3.1 (text, ordinal) step key** (record.rs + diff.rs) — the folded-map + both
   diff lookups + test.
4. **§3.2 run-completion gate** (record.rs `Record`/`RunCompletion` + diff.rs banner
   + exit-1 gate + `failed_scenarios` caller update + test). Depends on Task 3's
   `record.rs` touch — sequence 3 → 4 to avoid churn.
5. **§3.3 saturating math** (diff.rs) — fold into Task 4's diff.rs touch, or its own
   tiny commit.
6. **exit-130 docs (+ optional const dedupe)** (TECH-SPEC §10, ADR-0009, CHANGELOG;
   exec.rs/watch.rs const) — docs sweep, closes with docs-check.

Order: 1 → 2 → 3 → 4 (→ 5 folded) → 6. Tasks 1/2 are independent single-file fixes;
3 → 4 share `record.rs` so run sequentially; 6 is docs.
