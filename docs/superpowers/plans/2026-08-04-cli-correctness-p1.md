# proef v0.5.2 — CLI correctness fix pass — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the six validated §3 CLI-correctness findings from the external v0.5.0 review — the run-diff step-collision, the truncated/cancelled `--fail-on-regression` gate, a duration overflow, the directory-valued-setup double-run, the EPIPE panic in the diagnostic renderer, and the undocumented exit-130 — all in `proef-cli`, each with a genuinely-discriminating regression test.

**Architecture:** Bug-fix pass entirely in `proef-cli` (`exec.rs`, `render.rs`, `record.rs`, `diff.rs`, `watch.rs`) plus docs. `proef-core` is untouched; the event schema (ADR-0008 — `RunFinished`/`cancelled` already exist) is only *read*, never changed. No new dependencies. Full re-verified mechanisms with live file:line are in `docs/superpowers/specs/2026-08-04-cli-correctness-p1-design.md` "Verified facts".

**Tech Stack:** Rust 2024; `assert_cmd` + `tempfile` (existing proef-cli dev-deps); `proef_core::event::Event` / `proef_core::step::{Status, StepRef}` for synthetic records.

**Branch:** `fix/cli-correctness-p1` off `main` (5c36a5e = v0.5.0). Ships as **v0.5.2**.

## Global Constraints

- `proef-core` untouched (this is `proef-cli` only); sans-IO preserved; event schema (ADR-0008) unchanged — `RunFinished`/`cancelled` already exist, we only READ them.
- Exit codes are a contract (ADR-0009, assert_cmd-pinned): the §3.2 gate returns the existing `ExitCode::TestFailure` (1); §3.4 returns the existing `ExitCode::UserError` (2); **130 stays a documented signal-convention escape hatch, NOT a new `ExitCode` variant**.
- hurl pins `=8.0.1` untouched; **no new dependencies**.
- No task ids / plan numbers in code comments (changelog only). No AI-attribution commit trailers.
- Each fix ships a regression test that genuinely FAILS without the fix (the v0.5.1 pass caught a vacuous test — hold that bar; demonstrate RED before GREEN).
- The workspace package/binary is named **`proef`** (use `cargo … -p proef`, `assert_cmd::cargo::cargo_bin("proef")` — NOT `proef-cli`).
- Ships as **v0.5.2** (patch — bug fixes + docs).
- **Gate every task** (all must pass before commit): `cargo fmt --all --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo nextest run --profile ci`; `cargo test --doc`; `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace`; `cargo run -p xtask -- docs-check`.

**Task order:** 1 → 2 → 3 → 4 → 5. Tasks 1 and 2 are independent single-file fixes. Task 3 (record.rs `(text,ordinal)` key) and Task 4 (record.rs `Record`/completion + diff.rs gate + §3.3) both touch `record.rs` and `diff.rs`, so **Task 4 builds on Task 3** — run them in order. Task 5 is docs.

## File map

- `crates/proef-cli/src/exec.rs` — Task 1: `run_phase` directory guard. Task 5: exit-130 const (optional).
- `crates/proef-cli/src/render.rs` — Task 2: `errln!` macro + `print_all`.
- `crates/proef-cli/src/record.rs` — Task 3: `(text,ordinal)` step key. Task 4: `Record`/`RunCompletion`.
- `crates/proef-cli/src/diff.rs` — Task 3: step lookups. Task 4: completion banner + gate + saturating math.
- `crates/proef-cli/src/watch.rs` — Task 5: exit-130 const (optional).
- `crates/proef-cli/tests/execute.rs` — Tasks 1, 4: assert_cmd integration tests.
- `crates/proef-cli/tests/cli.rs` — Task 2: EPIPE assert_cmd test.
- `docs/TECH-SPEC.md`, `docs/adr/ADR-0009-*.md`, `docs/CHANGELOG.md` — Task 5.

---

### Task 1: §3.4 — reject a directory-valued `[run] setup`/`teardown`

**Files:**
- Modify: `crates/proef-cli/src/exec.rs` — `run_phase` (starts line 521; guard goes right after the signature, before `front::run` at 533)
- Test: `crates/proef-cli/tests/execute.rs`

**Interfaces:**
- `run_phase(label: &str, path: &Path, …) -> Result<runner::RunSummary, ExitCode>` — `label` is `"setup"`/`"teardown"`; `path` is the resolved `[run] setup`/`teardown` path. ADR-0014 defines each as exactly one feature file.

- [ ] **Step 1: Write the failing test.** In `crates/proef-cli/tests/execute.rs`, add (follow the file's existing tempdir + `assert_cmd` pattern — it already has 31 setup/teardown/diff tests to mirror for fixture layout, run-dir wiring, and `proef.toml` writing):

```rust
#[test]
fn directory_valued_setup_is_rejected_not_double_run() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // A suite with one ordinary feature.
    std::fs::create_dir_all(root.join("suite")).unwrap();
    std::fs::write(
        root.join("suite/main.feature"),
        "Feature: M\n  Scenario: S\n    When I noop\n",
    )
    .unwrap();
    // A DIRECTORY of setup features (the misconfiguration).
    std::fs::create_dir_all(root.join("setup")).unwrap();
    std::fs::write(
        root.join("setup/a.feature"),
        "Feature: A\n  Scenario: SA\n    When I noop\n",
    )
    .unwrap();
    // Minimal pack so `I noop` binds (mirror execute.rs's existing fixture packs).
    std::fs::create_dir_all(root.join("suite/packs")).unwrap();
    std::fs::write(
        root.join("suite/packs/p.yaml"),
        "macros:\n  noop:\n    match: \"I noop\"\n    steps:\n      - hurl: |\n          GET http://x\n",
    )
    .unwrap();
    std::fs::write(
        root.join("proef.toml"),
        "[run]\nsuite = \"suite\"\nsetup = \"setup\"\n",
    )
    .unwrap();

    assert_cmd::Command::cargo_bin("proef")
        .unwrap()
        .current_dir(root)
        .args(["test", "--dry-run"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "[run] setup must be a feature file, not a directory",
        ));
}
```

- [ ] **Step 2: Run it (verify RED).** `cargo nextest run -p proef directory_valued_setup_is_rejected_not_double_run`. Expected: FAIL — today a directory setup runs each feature twice and does NOT exit 2 (no such error message).

- [ ] **Step 3: Add the guard.** In `exec.rs`, at the very top of `run_phase`'s body (immediately after the `) -> Result<runner::RunSummary, ExitCode> {` on line 532, before `let front = front::run(` on 533):

```rust
    // ADR-0014: `[run] setup`/`teardown` names exactly one feature file. A
    // directory would run every feature under it as the phase AND leave them in
    // the pool (exclude_phase_features matches a single file path), running each
    // scenario twice. Reject it loudly instead of silently double-running.
    if path.is_dir() {
        eprintln!(
            "error: [run] {label} must be a feature file, not a directory ({})",
            path.display()
        );
        return Err(ExitCode::UserError);
    }
```

- [ ] **Step 4: Run it (verify GREEN) + a single-file guard test.** `cargo nextest run -p proef directory_valued_setup_is_rejected_not_double_run` → PASS. Then confirm a single-FILE setup still works (the good path isn't broken): add this test and run it:

```rust
#[test]
fn single_file_setup_still_runs_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("suite/packs")).unwrap();
    std::fs::write(root.join("suite/main.feature"),
        "Feature: M\n  Scenario: S\n    When I noop\n").unwrap();
    std::fs::write(root.join("suite/packs/p.yaml"),
        "macros:\n  noop:\n    match: \"I noop\"\n    steps:\n      - hurl: |\n          GET http://x\n").unwrap();
    std::fs::write(root.join("setup.feature"),
        "Feature: Setup\n  Scenario: SU\n    When I noop\n").unwrap();
    std::fs::write(root.join("proef.toml"),
        "[run]\nsuite = \"suite\"\nsetup = \"setup.feature\"\n").unwrap();

    // --dry-run validates + excludes the setup file from the pool; a single-file
    // setup must NOT be rejected (exit 0, not 2).
    assert_cmd::Command::cargo_bin("proef").unwrap()
        .current_dir(root).args(["test", "--dry-run"])
        .assert().code(0);
}
```

Expected: PASS (the guard only fires for directories). If the existing `execute.rs` fixtures need the setup feature to live inside vs outside the suite dir, mirror an existing passing setup test's layout.

- [ ] **Step 5: Full gate + commit.**

```bash
git add crates/proef-cli/src/exec.rs crates/proef-cli/tests/execute.rs
git commit -m "fix(cli): reject a directory-valued [run] setup/teardown

ADR-0014 defines setup/teardown as one feature file. A directory ran every
feature under it as the phase and again in the pool (double-run); now it is
a loud user error instead."
```

---

### Task 2: §3.5 — guard the diagnostic stderr write against EPIPE

**Files:**
- Modify: `crates/proef-cli/src/render.rs` (add `errln!`; `print_all` at line 40-48)
- Test: `crates/proef-cli/tests/cli.rs`

**Interfaces:**
- Produces: `macro_rules! errln` + `pub(crate) use errln;` (mirrors the existing `outln!` at render.rs:13-23).

- [ ] **Step 1: Write the failing test.** In `crates/proef-cli/tests/cli.rs`, add a test that renders diagnostics into a closed pipe and asserts no 101 panic. Use `std::process::Command` piping the child's stderr into a `head -c0` (which reads nothing and exits, closing the pipe); assert the child does not exit 101. Trigger diagnostics deterministically by dry-running the seeded broken corpus (a `tests/errors/<code>` dir fails validation and renders `Diag`s to stderr):

```rust
#[test]
fn diagnostics_do_not_panic_on_a_closed_stderr_pipe() {
    use std::process::{Command, Stdio};
    // `head -c0` reads nothing then exits, closing the read end of the pipe so
    // the next stderr write from proef gets EPIPE. The diagnostic renderer must
    // swallow it (exit with the normal error code), never panic with 101.
    let bin = assert_cmd::cargo::cargo_bin("proef");
    // Point at a broken corpus dir so validation emits diagnostics to stderr.
    // (Repo-relative: the seeded tests/errors/ corpus fails dry-run by design.)
    let repo_root = env!("CARGO_MANIFEST_DIR"); // crates/proef-cli
    let errors_dir = std::path::Path::new(repo_root)
        .join("../../tests/errors");
    // Pick any one broken code dir deterministically; the harness lists them.
    let one = std::fs::read_dir(&errors_dir).unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.is_dir())
        .expect("at least one seeded error corpus dir");

    let mut proef = Command::new(&bin)
        .args(["test", "--dry-run"])
        .arg(&one)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Consume a token amount then drop the reader to close the pipe early.
    let mut head = Command::new("head")
        .args(["-c", "0"])
        .stdin(proef.stderr.take().unwrap())
        .spawn()
        .unwrap();
    let _ = head.wait();
    let status = proef.wait().unwrap();
    // The exact non-zero code doesn't matter; 101 (panic) must NOT occur.
    assert_ne!(status.code(), Some(101), "diagnostic render panicked on EPIPE");
}
```

(If `head` is unavailable on the CI image, an equivalent is a tiny Rust reader that reads 0 bytes and drops the handle; prefer `head` since the repo's CI runs on Linux/macOS/Windows — on Windows `head` may be absent, so **guard this test with `#[cfg(unix)]`**: EPIPE is a POSIX signal-pipe behavior, and the fix is what matters cross-platform even if the test is unix-only. Note this in the test with a comment.)

- [ ] **Step 2: Run it (verify RED).** `cargo nextest run -p proef diagnostics_do_not_panic_on_a_closed_stderr_pipe`. Expected: FAIL — `print_all`'s `eprintln!` panics on `BrokenPipe` → the child exits 101.

- [ ] **Step 3: Add `errln!` and use it.** In `render.rs`, add after the `outln!` macro (after line 23):

```rust
/// Print a line to stderr, tolerating a closed pipe. Diagnostics go to stderr,
/// so `proef … |& head` must end the pipeline quietly (exit contract, never a
/// 101 panic). `BrokenPipe` is swallowed; any other stderr error is also
/// dropped — stderr is the only diagnostic channel, so a broken stderr has
/// nowhere left to report to (writing the failure to stdout would corrupt
/// program output).
macro_rules! errln {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let _ = writeln!(::std::io::stderr(), $($arg)*);
    }};
}
pub(crate) use errln;
```

Then change `print_all` (line 46) from `eprintln!("{report:?}");` to:

```rust
        errln!("{report:?}");
```

- [ ] **Step 4: Run it (verify GREEN) + full suite.** `cargo nextest run -p proef` → the EPIPE test passes; existing diagnostic/snapshot tests still pass (the `{report:?}` formatting is unchanged, only the write path is guarded).

- [ ] **Step 5: Full gate + commit.**

```bash
git add crates/proef-cli/src/render.rs crates/proef-cli/tests/cli.rs
git commit -m "fix(cli): swallow BrokenPipe when rendering diagnostics

print_all used raw eprintln!, panicking (exit 101) when stderr is a closed
pipe. Route it through an errln! guard mirroring outln!'s stdout guard."
```

---

### Task 3: §3.1 — disambiguate same-text steps by `(text, ordinal)`

**Files:**
- Modify: `crates/proef-cli/src/record.rs` (`ScenarioRun.steps` type; `read_record` folding at 78-110)
- Modify: `crates/proef-cli/src/diff.rs` (`note_flaky` 148-159; `note_slower` 163-180)
- Test: `crates/proef-cli/src/record.rs` (new `#[cfg(test)]` module — the file has none today)

**Interfaces:**
- `ScenarioRun.steps: BTreeMap<(String, usize), StepRun>` — key `(step text, 0-based occurrence ordinal of that text within the scenario)`. Consumed by `diff.rs`.
- `StepRef { file: Arc<str>, line: usize, text: Arc<str> }` (from `proef_core::step`) — the `step` field of `Event::StepFinished`.

- [ ] **Step 1: Write the failing test.** Add a `#[cfg(test)] mod tests` at the end of `record.rs`. It writes a synthetic `events.jsonl` by serializing real `Event` values (robust to the serde wire format), then asserts `read_record` keeps BOTH identical-text steps:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use proef_core::event::Event;
    use proef_core::step::{Status, StepRef};
    use std::sync::Arc;

    /// Write `events` as one JSON object per line into `<dir>/events.jsonl`.
    fn write_events(dir: &Path, events: &[Event]) {
        let body: String = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.join("events.jsonl"), body).unwrap();
    }

    fn step(file: &str, text: &str) -> StepRef {
        StepRef { file: Arc::from(file), line: 1, text: Arc::from(text) }
    }

    fn step_finished(scenario: &str, s: StepRef, attempts: u32, duration_ms: u64) -> Event {
        Event::StepFinished {
            scenario: Arc::from(scenario),
            engine: Arc::from("hurl"),
            step: s,
            status: Status::Passed,
            attempts,
            duration_ms,
            captures: Vec::new(),
            detail: None,
            attempt_details: Vec::new(),
        }
    }

    fn scenario_finished(scenario: &str, file: &str, status: Status) -> Event {
        Event::ScenarioFinished {
            scenario: Arc::from(scenario),
            file: Arc::from(file),
            status,
            timestamp_ms: None,
            worker: None,
        }
    }

    #[test]
    fn duplicate_text_steps_are_kept_distinct() {
        let tmp = tempfile::tempdir().unwrap();
        // One scenario, two steps with IDENTICAL text but different metrics.
        write_events(tmp.path(), &[
            step_finished("S", step("f.feature", "GET /x"), 1, 10),
            step_finished("S", step("f.feature", "GET /x"), 3, 40),
            scenario_finished("S", "f.feature", Status::Passed),
        ]);
        let record = read_record(tmp.path()).unwrap();
        let run = record.get(&("f.feature".to_string(), "S".to_string())).unwrap();
        // Both occurrences survive (pre-fix the second overwrote the first → len 1).
        assert_eq!(run.steps.len(), 2, "both same-text steps must be retained");
        assert_eq!(run.steps.get(&("GET /x".to_string(), 0)).unwrap().attempts, 1);
        assert_eq!(run.steps.get(&("GET /x".to_string(), 1)).unwrap().attempts, 3);
    }
}
```

(NOTE: this test calls `read_record(dir).unwrap().get(&key)` — under Task 3 `read_record` still returns the `BTreeMap` directly. Task 4 changes it to return `Record`; when Task 4 lands, this test updates to `read_record(dir).unwrap().scenarios.get(...)`. That caller update is part of Task 4's work.)

- [ ] **Step 2: Run it (verify RED).** `cargo nextest run -p proef duplicate_text_steps_are_kept_distinct`. Expected: FAIL — today `steps` is `BTreeMap<String, StepRun>`; the second `insert("GET /x", …)` overwrites the first, so `steps.len()` is 1 and the `(text, ordinal)` keys don't exist (compile error on the `.get(&(.., 0))` calls, or a length assertion failure once the key type is changed). Expect a compile failure first (the test uses the new key shape) — that IS the red for a type change.

- [ ] **Step 3: Change the step key in `record.rs`.** Update the struct doc + type (lines 49-55):

```rust
/// One scenario's outcome in a run record: aggregate status plus its steps.
/// Steps are keyed `(text, ordinal)` for diffing — the authored `line` shifts
/// when a file is edited above it, so text is the stable identity, and the
/// 0-based occurrence ordinal disambiguates two steps that share text
/// (macro-expanded steps commonly do) so neither is lost (`proef diff`).
#[derive(Debug, Clone)]
pub struct ScenarioRun {
    /// Aggregate scenario status.
    pub status: Status,
    /// Steps keyed by `(authored text, 0-based occurrence ordinal within the scenario)`.
    pub steps: BTreeMap<(String, usize), StepRun>,
}
```

Then in `read_record`, change `pending` to hold the new key and count occurrences per `(scenario, text)`. Replace the `pending` type (line 76) and the `StepFinished` arm (80-96):

```rust
    let mut pending: BTreeMap<(String, String), BTreeMap<(String, usize), StepRun>> =
        BTreeMap::new();
    // Occurrence ordinal per (scenario, step text), so identical-text steps get
    // distinct keys instead of overwriting each other.
    let mut seen: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut record: BTreeMap<(String, String), ScenarioRun> = BTreeMap::new();
    for line in text.lines() {
        match serde_json::from_str::<Event>(line) {
            Ok(Event::StepFinished { scenario, step, attempts, duration_ms, .. }) => {
                let text_key = step.text.to_string();
                let ord = {
                    let counter = seen
                        .entry((scenario.to_string(), text_key.clone()))
                        .or_insert(0);
                    let n = *counter;
                    *counter += 1;
                    n
                };
                pending
                    .entry((step.file.to_string(), scenario.to_string()))
                    .or_default()
                    .insert((text_key, ord), StepRun { attempts, duration_ms });
            }
```

(The `ScenarioFinished` arm and the rest are unchanged: `pending.remove(&key)` still yields the per-scenario step map, now `(text,ord)`-keyed.)

- [ ] **Step 4: Update `diff.rs` step lookups.** `note_flaky` (148-159) and `note_slower` (163-180) iterate `&new.steps`; the iteration item is now `((text, ord), new_step)`. Update:

```rust
    fn note_flaky(&mut self, key: &Key, base: &ScenarioRun, new: &ScenarioRun) {
        for ((text, ord), new_step) in &new.steps {
            let base_attempts = base.steps.get(&(text.clone(), *ord)).map_or(1, |s| s.attempts);
            if new_step.attempts > base_attempts {
                self.flaky.push(format!(
                    "    ⚠ {} — step \"{text}\" {base_attempts}→{} attempt(s)",
                    label(key),
                    new_step.attempts
                ));
            }
        }
    }
```

```rust
    fn note_slower(&mut self, key: &Key, base: &ScenarioRun, new: &ScenarioRun) {
        let (mut base_ms, mut new_ms) = (0u64, 0u64);
        for ((text, ord), new_step) in &new.steps {
            if let Some(base_step) = base.steps.get(&(text.clone(), *ord)) {
                base_ms += base_step.duration_ms;
                new_ms += new_step.duration_ms;
            }
        }
        // (§3.3 saturating math lands here in Task 4.)
        let delta = new_ms.saturating_sub(base_ms);
        if delta >= SLOWER_MIN_DELTA_MS
            && new_ms * SLOWER_MIN_RATIO_DEN >= base_ms * SLOWER_MIN_RATIO_NUM
        {
            self.slower.push(format!(
                "    ⏱ {}  {base_ms}ms → {new_ms}ms (+{delta}ms)",
                label(key)
            ));
        }
    }
```

The rendered `"{text}"` still shows only the text (the ordinal is identity, not display).

- [ ] **Step 5: Run tests (verify GREEN).** `cargo nextest run -p proef` → `duplicate_text_steps_are_kept_distinct` passes; existing diff behavior (single-occurrence texts) is unchanged (ordinal 0 for the common case). If `execute.rs` has diff integration tests, they stay green.

- [ ] **Step 6: Full gate + commit.**

```bash
git add crates/proef-cli/src/record.rs crates/proef-cli/src/diff.rs
git commit -m "fix(cli): key diff step records by (text, ordinal)

Macro-expanded steps that share text collided in the last-write-wins text
map, silently dropping steps from the diff. Key by (text, occurrence
ordinal) so the Nth occurrence pairs across runs and none is lost."
```

---

### Task 4: §3.2 — don't pass the gate on an incomplete run (+ §3.3 saturating math)

**Files:**
- Modify: `crates/proef-cli/src/record.rs` (add `RunCompletion`/`Record`; `read_record` return type; `failed_scenarios` 117-123; the Task-3 test's `.get` → `.scenarios.get`)
- Modify: `crates/proef-cli/src/diff.rs` (`diff()` 25-60; `Report::compute` call; banner; gate; `note_slower` saturating)
- Test: `crates/proef-cli/tests/execute.rs`

**Interfaces:**
- Produces: `pub enum RunCompletion { Completed, Cancelled, Incomplete }`; `pub struct Record { pub scenarios: BTreeMap<(String, String), ScenarioRun>, pub completion: RunCompletion }`; `pub fn read_record(dir: &Path) -> Result<Record, String>`.
- Consumes (Task 3): `ScenarioRun.steps: BTreeMap<(String, usize), StepRun>`.

- [ ] **Step 1: Write the failing tests.** In `crates/proef-cli/tests/execute.rs`, add three assert_cmd tests. They build two run dirs under a `.proef-runs` root by writing synthetic `events.jsonl` (reuse the `Event`-serializing approach; a small local `write_events` helper mirroring Task 3's, or a shared test util). A COMPLETE base run and a NEW run that is (a) missing `RunFinished`, (b) `cancelled:true`:

```rust
// Helper (top of execute.rs test module, or a shared util): write events.jsonl.
fn write_run(runs_root: &std::path::Path, id: &str, body: &str) {
    let dir = runs_root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("events.jsonl"), body).unwrap();
}

// Two JSONL bodies constructed from serialized Events (see record.rs test helpers
// for exact Event shapes). A COMPLETE run ends with RunFinished{cancelled:false};
// an INCOMPLETE run omits RunFinished; a CANCELLED run has cancelled:true.
```

Provide the bodies by serializing `Event` values in a small helper the test calls (the plan's Task 3 `mod tests` shows the exact `Event` construction — replicate `step_finished`/`scenario_finished` and add a `run_finished(passed, failed, skipped, cancelled)` builder and a `run_started` builder). The three tests:

```rust
#[test]
fn fail_on_regression_fails_when_new_run_is_incomplete() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join(".proef-runs");
    // base: one passing scenario, COMPLETE (ends with RunFinished).
    write_run(&runs, "00000000000000000000000001", &complete_pass_body());
    // new: same scenario passing but NO RunFinished (truncated/died).
    write_run(&runs, "00000000000000000000000002", &incomplete_pass_body());
    assert_cmd::Command::cargo_bin("proef").unwrap()
        .current_dir(tmp.path())
        .args(["diff", "--fail-on-regression"])
        .assert()
        .code(1)
        .stderr(predicates::str::contains("INCOMPLETE").or(
            predicates::str::contains("cannot certify")));
}

#[test]
fn fail_on_regression_fails_when_new_run_was_cancelled() {
    // identical to above but new run body ends with RunFinished{cancelled:true};
    // assert .code(1) and a CANCELLED banner.
}

#[test]
fn plain_diff_reports_incomplete_but_exits_zero() {
    // same records, `proef diff` (no --fail-on-regression) → exit 0 + INCOMPLETE banner.
    // assert .code(0) and stderr/stdout contains the banner word.
}
```

(The run-id dir names must satisfy `fsutil::is_run_id` so `all_runs` picks them up — use valid uuid-v7-shaped ids; mirror whatever existing `execute.rs` diff tests use for run-dir names. VERIFY `is_run_id`'s exact acceptance and copy a known-good id from an existing test.)

- [ ] **Step 2: Run them (verify RED).** `cargo nextest run -p proef fail_on_regression_fails_when_new_run_is_incomplete plain_diff_reports_incomplete_but_exits_zero fail_on_regression_fails_when_new_run_was_cancelled`. Expected: FAIL — today `diff` ignores completion; the incomplete new run's missing scenarios (if any) are `removed`, `--fail-on-regression` exits 0, and there is no banner.

- [ ] **Step 3: Add `Record`/`RunCompletion` and change `read_record`.** In `record.rs`:

```rust
/// Whether a run record represents a complete run — read from the tail
/// `RunFinished` event (ADR-0008). A truncated/died run has no `RunFinished`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunCompletion {
    /// Ended with `RunFinished { cancelled: false }`.
    Completed,
    /// Ended with `RunFinished { cancelled: true }`.
    Cancelled,
    /// No `RunFinished` — the run was truncated or the process died.
    Incomplete,
}

/// A full run record: every scenario outcome plus whether the run completed.
#[derive(Debug, Clone)]
pub struct Record {
    /// `(file, scenario) -> outcome`.
    pub scenarios: BTreeMap<(String, String), ScenarioRun>,
    /// Whether the run reached its tail `RunFinished`.
    pub completion: RunCompletion,
}
```

Change `read_record` to return `Result<Record, String>`. Track completion in the same pass: initialise `let mut completion = RunCompletion::Incomplete;`, add a match arm:

```rust
            Ok(Event::RunFinished { cancelled, .. }) => {
                completion = if cancelled { RunCompletion::Cancelled } else { RunCompletion::Completed };
            }
```

and return `Ok(Record { scenarios: record, completion })`.

- [ ] **Step 4: Update the callers.** `failed_scenarios` (117-123): `Ok(read_record(record_dir)?.scenarios.into_iter()...`. The Task-3 record.rs test: change `read_record(tmp.path()).unwrap().get(...)` → `read_record(tmp.path()).unwrap().scenarios.get(...)`.

- [ ] **Step 5: Wire the banner + gate in `diff.rs`.** In `diff()` (25-60), `base_rec`/`new_rec` are now `Record`. Before computing, emit a banner when either is not `Completed`; after computing, gate on the NEW run's completion:

```rust
    let base_rec = match record::read_record(&base_dir) { Ok(r) => r, Err(err) => { eprintln!("error: {err}"); return ExitCode::UserError; } };
    let new_rec = match record::read_record(&new_dir) { Ok(r) => r, Err(err) => { eprintln!("error: {err}"); return ExitCode::UserError; } };

    incomplete_banner("base", base, &base_dir, base_rec.completion);
    incomplete_banner("new", new, &new_dir, new_rec.completion);

    let report = Report::compute(&base_rec.scenarios, &new_rec.scenarios);
    report.render(&base_dir, &new_dir);

    if fail_on_regression {
        // An incomplete/cancelled NEW run cannot certify "no regressions".
        if new_rec.completion != record::RunCompletion::Completed {
            eprintln!(
                "error: the new run did not complete ({}) — cannot certify no regressions",
                completion_word(new_rec.completion)
            );
            return ExitCode::TestFailure;
        }
        if !report.regressed.is_empty() {
            return ExitCode::TestFailure;
        }
    }
    ExitCode::Success
```

with helpers:

```rust
fn completion_word(c: record::RunCompletion) -> &'static str {
    match c {
        record::RunCompletion::Completed => "completed",
        record::RunCompletion::Cancelled => "cancelled",
        record::RunCompletion::Incomplete => "incomplete — no RunFinished",
    }
}

/// Warn (always, even without --fail-on-regression) when a diffed record did
/// not complete, so a human is never misled by a partial run.
fn incomplete_banner(which: &str, _flag: Option<&str>, dir: &Path, c: record::RunCompletion) {
    if c != record::RunCompletion::Completed {
        crate::render::outln!(
            "⚠ {which} run {} is {} — results may be partial",
            run_name(dir),
            completion_word(c)
        );
    }
}
```

(Drop the unused `_flag` param if it isn't needed — match the surrounding style; `run_name`/`Path` are already in scope in `diff.rs`.) `Report::compute` already takes `&BTreeMap<Key, ScenarioRun>`, so passing `&base_rec.scenarios` / `&new_rec.scenarios` needs no change to `compute`.

- [ ] **Step 6: §3.3 saturating math.** In `note_slower` (the sums and ratio from Task 3's edit): `base_ms = base_ms.saturating_add(base_step.duration_ms); new_ms = new_ms.saturating_add(new_step.duration_ms);` and the comparison `new_ms.saturating_mul(SLOWER_MIN_RATIO_DEN) >= base_ms.saturating_mul(SLOWER_MIN_RATIO_NUM)`.

- [ ] **Step 7: Run tests (verify GREEN) + full suite.** The three new tests pass; `duplicate_text_steps_are_kept_distinct` (updated to `.scenarios`) passes; existing diff tests pass.

- [ ] **Step 8: Full gate + commit.**

```bash
git add crates/proef-cli/src/record.rs crates/proef-cli/src/diff.rs crates/proef-cli/tests/execute.rs
git commit -m "fix(cli): fail the diff gate on an incomplete or cancelled new run

read_record now reports run completion from the tail RunFinished; diff
banners any incomplete/cancelled record and, under --fail-on-regression,
refuses to pass when the new run did not complete. Harden note_slower's
duration math with saturating ops."
```

---

### Task 5: exit-130 documentation (+ optional const dedupe)

**Files:**
- Modify: `docs/TECH-SPEC.md` (§10 exit-codes sentence, ~line 266-267)
- Modify: `docs/adr/ADR-0009-*.md` (one-line note)
- Modify: `docs/CHANGELOG.md` (`[Unreleased]`)
- Optional: `crates/proef-cli/src/exec.rs` (line 180) + `crates/proef-cli/src/watch.rs` (line 61) — shared const

**Interfaces:** none (docs; optional const is `proef-cli`-internal).

- [ ] **Step 1: TECH-SPEC §10.** Read `docs/TECH-SPEC.md` around line 266-267 first. Extend the exit-codes sentence ("Exit codes: 0 ok · 1 test failure · 2 user error · 3 system error (typed enum, assert_cmd-pinned)") with a note, e.g.: "A second interrupt (Ctrl-C) while a `test`/`watch` run is cancelling forces an immediate hard exit with code **130** (128+SIGINT, the shell convention) — deliberately outside the graceful 0/1/2/3 taxonomy, so it is not an `ExitCode` variant." Match the surrounding prose style.

- [ ] **Step 2: ADR-0009.** Read `docs/adr/ADR-0009-*.md`. Add a one-line note (in the consequences or an amendment, matching the repo's ADR amendment convention — dated `## Amendment — YYYY-MM-DD` if the file uses that form) that 130 is the sanctioned OS-signal hard-abort code on a second interrupt, intentionally not a typed `ExitCode` variant (it is not a graceful outcome). Do not invent an amendment format the file doesn't already use — mirror an existing amended ADR (e.g. ADR-0011/0012).

- [ ] **Step 3: CHANGELOG.** Under `## [Unreleased]` in `docs/CHANGELOG.md`, add a `### Documentation` (or `### Fixed`) note that the second-Ctrl-C hard-exit code 130 is now documented for `test` and `watch`; and a `### Fixed` group for the five code fixes (directory-setup rejection, EPIPE-safe diagnostics, diff step-collision, incomplete-run gate, duration overflow) if not already present from earlier tasks. (Earlier tasks did not touch the CHANGELOG — add all v0.5.2 entries here.)

- [ ] **Step 4 (optional): dedupe the literal.** If a clean shared home exists (both `exec.rs` and `watch.rs` can see a small const without a new module), define once:

```rust
/// Hard-exit code on a second interrupt: 128 + SIGINT(2), the shell convention.
/// Deliberately outside the typed ExitCode taxonomy (not a graceful outcome).
pub(crate) const INTERRUPT_EXIT_CODE: i32 = 130;
```

in whichever of `exec.rs`/`commands.rs`/`main.rs` both already reference, and use it at both `std::process::exit(...)` sites. If no clean shared home exists without over-engineering, KEEP the two `130` literals and add the one-line convention comment at each site instead — implementer's judgment; note which you chose in the report. Either way, do NOT add 130 to the `ExitCode` enum.

- [ ] **Step 5: docs-check + gate + commit.** `cargo run -p xtask -- docs-check` must pass; run the full gate (unaffected by docs but must stay green).

```bash
git add docs/TECH-SPEC.md docs/adr/ADR-0009-*.md docs/CHANGELOG.md crates/proef-cli/src/exec.rs crates/proef-cli/src/watch.rs
git commit -m "docs: document the second-interrupt hard-exit code 130

130 (128+SIGINT) is the hard-abort on a second Ctrl-C in test and watch,
deliberately outside the typed 0/1/2/3 exit taxonomy. Record it in
TECH-SPEC §10, ADR-0009, and the changelog."
```

(Drop the `exec.rs`/`watch.rs` paths from `git add` if you kept the literals rather than the const.)

---

## Self-review

**Spec coverage:** §3.1 → Task 3; §3.2 → Task 4; §3.3 → Task 4 Step 6; §3.4 → Task 1; §3.5 → Task 2; exit-130 → Task 5. All six covered. Each code fix has a RED-first regression test (Tasks 1-4); §3.3 is hardening folded into Task 4; exit-130 is docs.

**Placeholder scan:** no TBD/TODO. Test bodies are concrete. Two deliberate VERIFY-at-implementation points are flagged with exact fallbacks: the EPIPE test's `head`/Windows guard (Task 2 Step 1 — `#[cfg(unix)]`), and the run-id shape for synthetic run dirs (Task 4 Step 1 — copy a known-good id from an existing `execute.rs` diff test). These are "confirm against the real fixture", not missing content.

**Type consistency:** `ScenarioRun.steps` is `BTreeMap<(String, usize), StepRun>` in Task 3 and consumed with `.get(&(text.clone(), *ord))` in Tasks 3-4. `read_record -> Result<Record, String>` (Task 4) with `Record { scenarios, completion }`; `failed_scenarios` and the Task-3 test both switch to `.scenarios` in Task 4. `RunCompletion { Completed, Cancelled, Incomplete }` used consistently in `diff.rs` gate/banner. `run_phase(label, path, …)` guard uses the real `label`/`path` params. Consistent.
