# Run-Record Correctness and Drift Guards Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the run record from lying — one head/tail pair per record, honest totals for truncated records, a `worker` field that means what EVENTS.md says, and no scenario that passes while running nothing.

**Architecture:** Four of the five tasks are CLI-only. The record-contract fix needs no core change because the CLI already composes event sinks — a wrapper that drops phase head/tail lets the CLI emit one pair around all phases. The truncated-record fix is net *less* code: `report` and `explain` move onto the record reader `diff` already uses.

**Tech Stack:** Rust 2024, `cargo-nextest`, `assert_cmd`, `proef-fixture`, `tempfile`.

**Approved spec:** `docs/superpowers/specs/2026-08-06-run-record-correctness-design.md` — it carries the verified `file:line` facts. Cite them; do not re-derive.

**Branch:** `feat/first-run-ux`. Phase 1 complete at `59f9243`; spec at `5f92b5a`. **All five tasks land on this same branch and ship in the same PR as phase 1.**

## Global Constraints

- **SAME branch, SAME PR** as phase 1 — `feat/first-run-ux`.
- **`proef-core` is untouched by Tasks 1, 2 and 3.** Task 4 is the only one that may touch it; if it adds a public item, it regenerates `crates/proef-core/public-api.txt`.
- `proef-core` stays sans-IO: no IO, no clock reads, no env reads, no randomness.
- No new dependencies. hurl pins stay exactly `hurl = "=8.0.1"`, `hurl_core = "=8.0.1"`.
- The package name for `cargo -p` and `assert_cmd::cargo::cargo_bin` is **`proef`**, NOT `proef-cli`.
- **No raw print macros** in `proef-cli` — and after Task 5, none in `proef-lsp` either. Use `crate::render::outln!` / `errln!` in the CLI. The guard is line-based, so it also trips on those tokens inside a comment.
- **The event schema does not change**, so ADR-0008's additive-only rule is not engaged. Any EVENTS.md claim that becomes true (or was already true) must be verified and the verification stated.
- No task ids, plan numbers, or review-section references in code comments. Changelog only; cite durable ADRs.
- No AI-attribution commit trailers.
- **Every test must genuinely fail without its change.** Demonstrate RED. Task 3's bug survived precisely because a single-scenario test could not distinguish the two models.
- No version bump. Everything rides `## [Unreleased]`.

## File Structure

| File | Change | Responsibility |
| --- | --- | --- |
| `crates/proef-cli/src/exec.rs` | Modify | Sink suppressor + single head/tail (T1); slot free-list (T3) |
| `crates/proef-cli/src/report.rs` | Modify | Read via `record::read_record`; incomplete banner (T2) |
| `crates/proef-cli/src/explain.rs` | Modify | Same, plus totals from actual outcomes (T2) |
| `crates/proef-core/src/*` | Modify (T4 only) | `empty_scenario` diagnostic |
| `crates/proef-cli/src/fsutil.rs` | Modify | `is_run_id` length guard (T5) |
| `crates/proef-cli/tests/stderr_hygiene.rs` | Modify | Widen scan to `proef-lsp/src` (T5) |
| `.github/workflows/nightly.yml` | Modify | `shell: bash` (T5) |
| `crates/proef-cli/tests/execute.rs` | Modify | Fixture-backed tests for T1, T3 |
| `docs/DIAGNOSTICS.md`, `docs/CHANGELOG.md` | Modify | Diagnostic registration (T4), changelog (T5) |

---

### Task 1: One `RunStarted`/`RunFinished` per record

**Files:**
- Modify: `crates/proef-cli/src/exec.rs` (sink at `:171`; phase calls at `:213`, `:271`, `:278`)
- Test: `crates/proef-cli/tests/execute.rs` (append)

**Interfaces:**
- Consumes: `proef_core::event::EventSink` — `#[derive(Clone)]` over `Arc<dyn Fn(&Event) + Send + Sync>` (`crates/proef-core/src/event.rs:141-148`), constructed via `EventSink::new(f)`. `proef_core::runner::RunSummary` carries `outcomes`, `passed: usize`, `failed: usize`, `skipped: usize`, `cancelled: bool` (`crates/proef-core/src/runner.rs:72-83`). `EVENT_SCHEMA_VERSION` and `Event::{RunStarted, RunFinished}` come from `proef_core::event`.
- Produces: `fn suppress_run_head_tail(inner: EventSink) -> EventSink` in `exec.rs`.

**Why this needs no core change:** the CLI already wraps sinks — `stamp_scenario_timing(...)` at `exec.rs:171` is the pattern. `runner::run` keeps its signature, `public-api.txt` must not move, and the event schema is unchanged.

- [ ] **Step 1: Write the failing tests**

Append to `crates/proef-cli/tests/execute.rs`. Model the setup/teardown corpus on the existing `setup_shares_globals_teardown_runs_and_both_are_excluded` and `teardown_failure_is_a_distinct_cleanup_fault` in the same file — read them first for the `[run] setup` / `[run] teardown` `proef.toml` shape and the fixture wiring.

```rust
/// A run with setup and teardown must still produce ONE record: one
/// `run_started` line and one `run_finished` line. Three head/tail pairs make
/// every whole-file consumer read phase-blended results.
#[test]
fn phases_produce_a_single_run_started_and_run_finished() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // Build a suite with a PASSING setup, a FAILING main scenario, and a
    // PASSING teardown. The failing main is what makes the bug visible: the
    // last `run_finished` (teardown's) would otherwise win the headline.
    write_phase_suite(cwd.path());

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite"])
        .assert()
        .code(1);

    let record = latest_events_jsonl(cwd.path());
    let started = record.lines().filter(|l| l.contains("\"run_started\"")).count();
    let finished = record.lines().filter(|l| l.contains("\"run_finished\"")).count();
    assert_eq!(started, 1, "expected exactly one run_started:\n{record}");
    assert_eq!(finished, 1, "expected exactly one run_finished:\n{record}");
}

/// `explain` must report the run's own verdict, not the last phase's. Before
/// the fix this printed "1 passed · 0 failed" directly above a printed failure.
#[test]
fn explain_reports_the_failure_not_the_teardown_totals() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    write_phase_suite(cwd.path());

    proef_in(cwd.path(), &fixture).args(["test", "suite"]).assert().code(1);

    let assert = proef_in(cwd.path(), &fixture).arg("explain").assert().code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(out.contains("1 failed"), "headline must show the failure: {out}");
    assert!(
        !out.contains("1 passed · 0 failed"),
        "headline must not be teardown's totals: {out}"
    );
}

/// The console run header keys off `RunStarted`, so suppressing phase head/tail
/// must also collapse the three headers a phased run used to print.
#[test]
fn console_prints_the_run_header_once_per_run() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    write_phase_suite(cwd.path());

    let assert = proef_in(cwd.path(), &fixture).args(["test", "suite"]).assert().code(1);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert_eq!(
        out.matches("running ").count(),
        1,
        "run header should appear once, not once per phase: {out}"
    );
}
```

Write the two helpers alongside them. `write_phase_suite` creates `proef.toml` with `[run] suite = "suite"`, `setup = "suite/setup.feature"`, `teardown = "suite/teardown.feature"` plus the `[url] base` line the other fixture tests use; a passing setup feature, a **failing** main feature (assert a status the fixture does not return, as `failure_maps_to_feature_line_and_artifact_span` does), and a passing teardown feature; and a pack binding all three sentences. `latest_events_jsonl` reads the newest directory under `.proef-runs/` and returns its `events.jsonl` as a `String`.

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
cargo nextest run -p proef --test execute phases_produce_a_single -E 'test(explain_reports_the_failure)' 2>&1 | tail -30
cargo nextest run -p proef --test execute console_prints_the_run_header_once 2>&1 | tail -20
```

Expected: **FAIL** — three `run_started`/`run_finished` lines, an `explain` headline showing teardown's totals, and three console headers. Paste the verbatim output into the report.

- [ ] **Step 3: Add the suppressing sink wrapper**

In `crates/proef-cli/src/exec.rs`, beside `stamp_scenario_timing` (`:480`):

```rust
/// A sink that drops `RunStarted`/`RunFinished` and passes everything else
/// through. Each phase calls `runner::run`, which brackets its own work with
/// that pair; a record must carry exactly one pair overall (ADR-0008), so the
/// phases run against this wrapper and the caller emits the single pair.
fn suppress_run_head_tail(inner: EventSink) -> EventSink {
    EventSink::new(move |event| match event {
        Event::RunStarted { .. } | Event::RunFinished { .. } => {}
        other => inner.emit(other),
    })
}
```

- [ ] **Step 4: Emit one pair and route the phases through the wrapper**

At `exec.rs:171`, keep `sink` as it is, then derive the phase sink and open the record:

```rust
    let phase_sink = suppress_run_head_tail(sink.clone());
    sink.emit(&proef_core::event::Event::RunStarted {
        schema: proef_core::event::EVENT_SCHEMA_VERSION,
        run_id: Arc::clone(&front.run_id),
    });
```

Pass `&phase_sink` — not `&sink` — to all three `runner::run` paths: the setup `run_phase` call (`:213`), the main `runner::run` (`:271`), and the teardown `run_phase` call (`:278`).

After teardown, before computing the exit code, close the record with aggregated totals:

```rust
    sink.emit(&proef_core::event::Event::RunFinished {
        passed: total_passed,
        failed: total_failed,
        skipped: total_skipped,
        cancelled: cancel.is_cancelled(),
    });
```

Accumulate `total_passed` / `total_failed` / `total_skipped` from every `RunSummary` the three phases produce — each carries `passed`, `failed`, `skipped` (`runner.rs:72-83`). Declare them before the setup block as `let mut total_passed = 0usize;` (and the same for the other two) and add each summary's counts where that summary is already matched. Do not re-derive them from `outcomes`.

Adjust imports at the top of `exec.rs` if `Event` or `EVENT_SCHEMA_VERSION` are not already in scope; prefer fully-qualified paths over new `use` lines if only used here.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo nextest run -p proef --test execute phases_produce_a_single
cargo nextest run -p proef --test execute explain_reports_the_failure
cargo nextest run -p proef --test execute console_prints_the_run_header_once
```

Expected: all **PASS**.

- [ ] **Step 6: Confirm `proef-core` did not move**

```bash
git status --porcelain crates/proef-core
```

Expected: empty. If anything under `crates/proef-core` changed, revert it — this task is CLI-only by constraint.

- [ ] **Step 7: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

Expected: all green. **Watch for insta snapshots of event streams** — `event_stream_snapshot_reference_run` in `tests/execute.rs` covers a record. If it moves, inspect the diff, confirm the only change is the collapsed head/tail, and say so explicitly in your report with the snapshot name and diff. Never blind-accept.

- [ ] **Step 8: Commit**

```bash
git add crates/proef-cli/src/exec.rs crates/proef-cli/tests/execute.rs
git commit -m "fix(cli): emit one run_started/run_finished per record

Each phase — setup, the suite, teardown — called runner::run, which brackets
its own work with RunStarted/RunFinished. One events.jsonl therefore held up
to three head/tail pairs, breaking the record contract that every whole-file
consumer relies on. Because the totals reader assigns rather than accumulates,
explain showed the LAST pair's totals: a run with a failing suite and a passing
teardown reported \"1 passed · 0 failed\" directly above the printed failure.

The phases now run against a sink that drops that pair, and the CLI emits one
around all of them with totals aggregated across the phase summaries. The core
is untouched: the wrapper composes at the CLI edge like the timing stamper
beside it. The console run header collapses to one per run as a consequence —
the reporter keys off the same two events."
```

---

### Task 2: `report` and `explain` read through the record reader

**Files:**
- Modify: `crates/proef-cli/src/explain.rs`, `crates/proef-cli/src/report.rs`
- Test: `crates/proef-cli/tests/execute.rs` (append)

**Interfaces:**
- Consumes: `crate::record::read_record` and its types, already defined at `crates/proef-cli/src/record.rs:70-88` — `RunCompletion { Completed, Cancelled, Incomplete }` and `Record { scenarios: BTreeMap<(String, String), ScenarioRun>, completion: RunCompletion }`. Read that module before starting; `diff` is the existing consumer to model on.
- Produces: nothing later tasks depend on.

**This task should remove more lines than it adds.** `explain` and `report` each hand-roll a line loop over the record; `diff` reads through `read_record` and got the completion guard for free in 0.5.2. The duplication *is* the bug.

- [ ] **Step 1: Write the failing tests**

Append to `crates/proef-cli/tests/execute.rs`:

```rust
/// A record with no `run_finished` is a truncated run — OOM-kill, CI timeout,
/// crash. `explain` and `report` are the post-mortem tools, so they are exactly
/// the ones that must say so instead of rendering it as complete.
#[test]
fn explain_and_report_flag_a_truncated_record() {
    let cwd = tempfile::tempdir().unwrap();
    let run = cwd.path().join(".proef-runs/0198f3c1-0000-7000-8000-000000000001");
    std::fs::create_dir_all(&run).unwrap();
    // Starts, runs one scenario to completion, then stops: no run_finished.
    std::fs::write(
        run.join("events.jsonl"),
        concat!(
            r#"{"schema":1,"event":"run_started","run_id":"0198f3c1-0000-7000-8000-000000000001","scenarios":2}"#, "\n",
            r#"{"schema":1,"event":"scenario_started","scenario":"first","file":"suite/a.feature"}"#, "\n",
            r#"{"schema":1,"event":"scenario_finished","scenario":"first","file":"suite/a.feature","status":"passed","line":3}"#, "\n",
        ),
    )
    .unwrap();

    let mut explain = assert_cmd::Command::cargo_bin("proef").unwrap();
    let assert = explain.current_dir(cwd.path()).env("NO_COLOR", "1").arg("explain").assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(out.contains("incomplete"), "explain must flag incompleteness: {out}");
    // The record holds one passed scenario; reporting zeros is the bug.
    assert!(
        !out.contains("0 passed · 0 failed · 0 skipped"),
        "totals must come from the scenarios present, not the missing tail: {out}"
    );

    let out_html = cwd.path().join("report.html");
    let mut report = assert_cmd::Command::cargo_bin("proef").unwrap();
    report
        .current_dir(cwd.path())
        .env("NO_COLOR", "1")
        .args(["report", "-o", &out_html.display().to_string()])
        .assert()
        .code(0);
    let html = std::fs::read_to_string(&out_html).unwrap();
    assert!(html.contains("incomplete"), "report must banner incompleteness");
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo nextest run -p proef --test execute explain_and_report_flag_a_truncated 2>&1 | tail -25
```

Expected: **FAIL** — `explain` prints `0 passed · 0 failed · 0 skipped` with no incompleteness note, and the HTML has no banner. Paste the verbatim output.

- [ ] **Step 3: Move `explain` onto `read_record`**

In `crates/proef-cli/src/explain.rs`, replace the hand-rolled event-line loop with a `crate::record::read_record` call. Derive the headline totals by counting the `Record`'s `scenarios` values by status — **not** from a `RunFinished` event, which a truncated record does not have. When `completion` is `RunCompletion::Incomplete`, print a loud line before the totals:

```rust
    crate::render::outln!("⚠ run incomplete — no run_finished; results are partial");
```

Keep the existing failure detail output. Delete the now-unused line-parsing code rather than leaving it beside the new path.

- [ ] **Step 4: Move `report` onto `read_record`**

In `crates/proef-cli/src/report.rs`, do the same: read through `crate::record::read_record`, and when `completion` is `Incomplete`, pass a banner into the rendered HTML so the page itself says the run did not finish. Match how the existing HTML renderer receives its inputs (`proef_core::html::render_html`); if it has no banner parameter, prepend the notice to the page's existing heading content rather than changing the renderer's signature.

- [ ] **Step 5: Run the test to verify it passes**

```bash
cargo nextest run -p proef --test execute explain_and_report_flag_a_truncated
```

Expected: **PASS**.

- [ ] **Step 6: Confirm the net line count went down**

```bash
git diff --stat crates/proef-cli/src/explain.rs crates/proef-cli/src/report.rs
```

Expected: deletions present in both files. If either file only grew, the hand-rolled loop was left in place beside the new reader — remove it. Report the numbers.

- [ ] **Step 7: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

Expected: all green. If an insta snapshot of `report`/`explain` output moves, inspect and justify it in the report; never blind-accept.

- [ ] **Step 8: Commit**

```bash
git add crates/proef-cli/src/explain.rs crates/proef-cli/src/report.rs crates/proef-cli/tests/execute.rs
git commit -m "fix(cli): report and explain flag a truncated run record

diff learned in 0.5.2 that a record without a tail run_finished cannot be
certified. report and explain never did — because each hand-rolls its own pass
over the event lines while diff reads through record::read_record, which
already models completion. The duplication was the bug, so both now read
through the same reader and the hand-rolled loops are gone.

explain's headline was also derived solely from run_finished, so a truncated
record printed \"0 passed · 0 failed · 0 skipped\" even when the record held a
passed scenario. Totals now come from the scenarios the record actually
contains, and both commands say plainly that the run did not finish."
```

---

### Task 3: `worker` becomes a real slot index

**Files:**
- Modify: `crates/proef-cli/src/exec.rs` (`stamp_scenario_timing`, `:480-489`)
- Test: `crates/proef-cli/tests/execute.rs` (append)

**Interfaces:**
- Consumes: `Event::ScenarioStarted { scenario, file, .. }` and `Event::ScenarioFinished { scenario, file, .. }` — both carry the scenario identity needed for keying.
- Produces: nothing later tasks depend on.

**Why identity, not `ThreadId`:** the runner spawns a thread per scenario (`crates/proef-core/src/runner.rs:370`) and ids are never reused, so `map.len()` yields a per-scenario ordinal. Releasing on `ThreadId` would also leak slots exactly when the watchdog fires, because an abandoned scenario's `ScenarioFinished` is emitted by the sweep on the dispatcher thread, not the worker thread.

- [ ] **Step 1: Write the failing test**

Append to `crates/proef-cli/tests/execute.rs`:

```rust
/// With one job, every scenario runs on the one worker slot — so every stamped
/// `worker` must be 0. The pre-existing snapshot test uses a single scenario,
/// where a per-scenario ordinal and a worker slot are numerically identical;
/// this needs two or more to tell the two models apart.
#[test]
fn worker_is_a_slot_index_not_a_scenario_ordinal() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(cwd.path().join("proef.toml"), BASE_URL_CONFIG).unwrap();
    let mut feature = String::from("# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n");
    for n in 1..=3 {
        feature.push_str(&format!(
            "  Scenario: case {n}\n    When the health endpoint is checked\n"
        ));
    }
    std::fs::write(cwd.path().join("suite/case.feature"), feature).unwrap();
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  health:\n    match: the health endpoint is checked\n    steps:\n      - hurl: |\n          GET ${url:base}/health\n          HTTP 200\n",
    )
    .unwrap();

    proef_in(cwd.path(), &fixture)
        .args(["test", "suite", "--jobs", "1"])
        .assert()
        .code(0);

    let record = latest_events_jsonl(cwd.path());
    let stamped: Vec<&str> = record
        .lines()
        .filter(|l| l.contains("\"worker\""))
        .collect();
    assert!(stamped.len() >= 3, "expected stamped events: {record}");
    for line in &stamped {
        assert!(
            line.contains("\"worker\":0"),
            "every event should stamp slot 0 at --jobs 1: {line}"
        );
    }
}
```

`latest_events_jsonl` and `BASE_URL_CONFIG` already exist in this file (the helper is added by Task 1; `BASE_URL_CONFIG` is a module-level const at `execute.rs:29`).

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo nextest run -p proef --test execute worker_is_a_slot_index 2>&1 | tail -20
```

Expected: **FAIL** — the three scenarios stamp `worker:0`, `worker:1`, `worker:2`. Paste the verbatim output.

- [ ] **Step 3: Replace the ordinal map with a slot free-list**

In `crates/proef-cli/src/exec.rs`, change `stamp_scenario_timing`'s state from `HashMap<ThreadId, u64>` to a slot table keyed by scenario identity:

```rust
    // `worker` is the 0-based slot the scenario occupied, so the timeline shows
    // occupancy of the `--jobs` workers (ADR-0015). A fresh OS thread is
    // spawned per scenario, so thread identity would yield a per-scenario
    // ordinal instead; slots are assigned on start and released on finish.
    // Release keys on scenario identity because an abandoned scenario's
    // finish is emitted by the watchdog sweep, not by the worker thread.
    let slots: Arc<Mutex<HashMap<(String, String), u64>>> = Arc::new(Mutex::new(HashMap::new()));
```

Assign the lowest free slot when a scenario starts:

```rust
        let acquire_slot = |scenario: &str, file: &str| {
            let mut map = slots.lock().unwrap_or_else(PoisonError::into_inner);
            let taken: std::collections::BTreeSet<u64> = map.values().copied().collect();
            let slot = (0u64..).find(|i| !taken.contains(i)).unwrap_or(0);
            map.insert((scenario.to_owned(), file.to_owned()), slot);
            slot
        };
        let release_slot = |scenario: &str, file: &str| {
            let mut map = slots.lock().unwrap_or_else(PoisonError::into_inner);
            map.remove(&(scenario.to_owned(), file.to_owned()));
        };
```

Call `acquire_slot` in the `ScenarioStarted` arm in place of the old `worker_index()`, and call `release_slot` in the `ScenarioFinished` arm. `ScenarioFinished` currently stamps `worker: None` and should keep doing so — releasing is its only new job here. Remove the now-unused `ThreadId` import if nothing else uses it.

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo nextest run -p proef --test execute worker_is_a_slot_index
```

Expected: **PASS** — all events stamp `worker:0`.

- [ ] **Step 5: Verify the EVENTS.md wording**

Read the `worker` field's description in `docs/EVENTS.md`. It documents a "0-based worker index". Confirm whether that sentence is now true and needs no edit, or whether it needs adjusting. **State the verification either way in your report** — including the exact sentence you checked.

- [ ] **Step 6: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

Expected: all green. The pre-existing single-scenario timeline snapshot should be unaffected (one scenario stamps slot 0 under both models); if it moves, inspect and justify.

- [ ] **Step 7: Commit**

```bash
git add crates/proef-cli/src/exec.rs crates/proef-cli/tests/execute.rs
git commit -m "fix(cli): stamp the worker slot, not a per-scenario ordinal

The stamper assigned each newly-seen thread id the next index, but the runner
spawns a thread per scenario and ids are never reused — so \`worker\` counted
scenarios, not workers. Fifty scenarios produced fifty lanes in the timeline
the field exists to draw, whatever --jobs was set to.

Slots are now acquired on scenario start and released on finish, so the value
is the 0-based slot the scenario occupied. Release keys on scenario identity
rather than thread identity: an abandoned scenario's finish is emitted by the
watchdog sweep on the dispatcher thread, so keying on the thread would leak a
slot exactly when the watchdog fires. The regression test runs three scenarios
at --jobs 1 — the old single-scenario snapshot could not distinguish the two
models, which is why this survived."
```

---

### Task 4: A zero-step scenario is a hard error

**Files:**
- Modify: `crates/proef-core/src/` (the bind path — locate it in Step 3)
- Create: `tests/errors/feature__empty_scenario/case.feature`, `tests/errors/feature__empty_scenario/packs/broken.yaml`
- Modify: `docs/DIAGNOSTICS.md`
- Possibly modify: `crates/proef-core/public-api.txt`

**Interfaces:**
- Consumes: the existing diagnostic machinery in `proef-core` — read `crates/proef-core/src/diag.rs` and an existing bind-time diagnostic (`bind::unbound_step`) for the exact construction shape, code-naming convention, and span handling.
- Produces: diagnostic code `proef::feature::empty_scenario`.

**This is the only task permitted to touch `proef-core`.** It stays sans-IO: a parse/bind-time check over data already in hand.

- [ ] **Step 1: Write the failing corpus case**

Create `tests/errors/feature__empty_scenario/case.feature`:

```gherkin
Feature: E
  Scenario: todo later
```

Create `tests/errors/feature__empty_scenario/packs/broken.yaml`:

```yaml
macros:
  noop:
    match: nothing binds here
    steps:
      - hurl: |
          GET http://x/health
          HTTP 200
```

The pack exists only so the suite loads; the scenario's emptiness is the error.

- [ ] **Step 2: Run it to verify it currently passes (the bug)**

```bash
cargo run -q -p proef -- test --dry-run tests/errors/feature__empty_scenario ; echo "exit=$?"
```

Expected **before the fix**: `dry-run OK: … 1 scenario(s), 0 step(s), 0 batch(es) … 0 warning(s)`, `exit=0`. That green is the bug — the seeded corpus is supposed to fail by design. Paste the verbatim output as the RED evidence.

- [ ] **Step 3: Add the diagnostic**

Find where scenarios are bound in `proef-core` — start from `crates/proef-core/src/bind.rs` and follow how `bind_scenario` walks a scenario's steps. Raise an error diagnostic when a scenario has no steps, with the scenario's span, code `proef::feature::empty_scenario`, and a message naming the scenario plus a help line saying a scenario must have at least one step (a commented-out body is the usual cause).

Match the construction style of the neighbouring diagnostics exactly — same builder methods, same span conventions (0-based byte offsets, end-exclusive), same message tone.

- [ ] **Step 4: Verify the corpus case now fails**

```bash
cargo run -q -p proef -- test --dry-run tests/errors/feature__empty_scenario ; echo "exit=$?"
```

Expected: `exit=2`, one error, code `proef::feature::empty_scenario`.

- [ ] **Step 5: Check the repo's own corpora for breakage**

```bash
cargo run -q -p proef -- test --dry-run tests/features ; echo "exit=$?"
cargo run -q -p proef -- flows tests/features >/dev/null ; echo "flows exit=$?"
```

Expected: unchanged from before your edit (the reference corpus has no empty scenarios). **Report exactly what you found** — if any existing feature breaks, name it and stop rather than editing the corpus to suit the diagnostic.

- [ ] **Step 6: Register the diagnostic in the docs**

In `docs/DIAGNOSTICS.md`: add a table row for `proef::feature::empty_scenario` in the `feature::` group, matching neighbouring rows' column layout, with the corpus column ticked (copy the exact marker a seeded row uses). Then update the coverage sentence near line 108 — it currently reads "24 of the 59 codes carry a seeded corpus case today". Both the seeded count **and** the total code count change: recount both from the table rather than incrementing blindly, and state the numbers you counted in your report.

- [ ] **Step 7: Regenerate the public-api snapshot if needed**

```bash
git status --porcelain crates/proef-core
PROEF_PUBLIC_API_UPDATE=1 cargo run -p xtask -- public-api
git diff --stat crates/proef-core/public-api.txt
```

If the diagnostic added no public item, the snapshot will not move — say so. If it did, include the diff in your report.

- [ ] **Step 8: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

Expected: all green, including the corpus guard that walks `tests/errors/`.

- [ ] **Step 9: Commit**

```bash
git add crates/proef-core tests/errors/feature__empty_scenario docs/DIAGNOSTICS.md
git commit -m "fix(core): a scenario with no steps is an error

gherkin makes steps optional, so \`Scenario: todo later\` parsed, bound to an
empty step list, lowered to zero batches, and folded to Passed — a dry-run
reported 0 steps and 0 warnings at exit 0, and a commented-out scenario body
stayed green in CI indefinitely.

The zero-entry step and zero-entry payload cases were already guarded; this
was the remaining way to be green while running nothing, so it fails loudly
now. A warning would have left the run at exit 0 and only narrowed the hole."
```

---

### Task 5: Guards batch and changelog

**Files:**
- Modify: `.github/workflows/nightly.yml` (`:41`)
- Modify: `crates/proef-cli/src/fsutil.rs` (`:47-49`)
- Modify: `crates/proef-cli/tests/stderr_hygiene.rs` (`:38`)
- Modify: `crates/proef-core/src/resolve.rs` (test module only)
- Modify: `docs/CHANGELOG.md`
- Test: rotation test — add to `crates/proef-cli/tests/cli.rs`

**Interfaces:**
- Consumes: `crate::fsutil::is_run_id` (`crates/proef-cli/src/fsutil.rs:47`), used by run rotation and `explain`'s latest-run lookup. `proef_core::matcher::closest` for the resolve test.
- Produces: nothing. Final task.

- [ ] **Step 1: Fix the nightly canary step**

In `.github/workflows/nightly.yml`, the canary step at `:41` is:

```yaml
      - name: Canary — build + test against the next hurl release
        id: canary
        run: cargo run -p xtask -- canary 2>&1 | tee canary.log
```

Add `shell: bash` to it:

```yaml
      - name: Canary — build + test against the next hurl release
        id: canary
        shell: bash
        run: cargo run -p xtask -- canary 2>&1 | tee canary.log
```

GitHub's default `run:` shell is `bash -e {0}` **without** `pipefail`, so the step's exit code is `tee`'s 0 and the `if: failure()` issue-on-red step below it is unreachable. Naming the shell explicitly gets `-o pipefail`. Change nothing else about the step.

- [ ] **Step 2: Write the failing rotation test**

`is_run_id` lives in a binary crate, so an integration test cannot reach it. Add the test to the `#[cfg(test)] mod tests` block **already present** at the bottom of `crates/proef-cli/src/fsutil.rs` (it holds the `parent_dir` test), calling `is_run_id` directly:

```rust
    #[test]
    fn only_hyphenated_uuid_dirs_count_as_run_ids() {
        // Rotation deletes the oldest run-shaped directories beyond the
        // retention limit, and the runs dir can point somewhere shared — so
        // "run-shaped" must mean the hyphenated form proef actually writes,
        // not every spelling the uuid parser accepts.
        assert!(is_run_id("0198f3c1-0000-7000-8000-00000000001a"));
        assert!(!is_run_id("0198f3c100007000800000000000001a"));
        assert!(!is_run_id("urn:uuid:0198f3c1-0000-7000-8000-00000000001a"));
        assert!(!is_run_id("{0198f3c1-0000-7000-8000-00000000001a}"));
        assert!(!is_run_id("cache-abc"));
    }
```

- [ ] **Step 3: Run it to verify it fails**

```bash
cargo nextest run -p proef only_hyphenated_uuid_dirs 2>&1 | tail -20
```

Expected: **FAIL** on the 32-hex, urn, and braced cases — `Uuid::try_parse` accepts all three today. Paste the verbatim output.

- [ ] **Step 4: Add the length guard**

In `crates/proef-cli/src/fsutil.rs:47-49`:

```rust
pub fn is_run_id(name: &str) -> bool {
    // proef only ever writes the hyphenated form. `Uuid::try_parse` also
    // accepts bare 32-hex, `urn:uuid:…` and braced spellings — and rotation
    // deletes the oldest run-shaped directories, so breadth here is a deletion
    // hazard when the runs dir points somewhere shared.
    name.len() == 36 && uuid::Uuid::try_parse(name).is_ok()
}
```

- [ ] **Step 5: Widen the print-macro guard to `proef-lsp`**

In `crates/proef-cli/tests/stderr_hygiene.rs`, the scan root at `:38` is:

```rust
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
```

Scan both crates instead — keep **one** implementation of the rule rather than copying the test into `proef-lsp`:

```rust
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let roots = [manifest.join("src"), manifest.join("../proef-lsp/src")];
```

Walk each root, and keep the existing non-empty assertion per root so a mistyped path cannot make the scan vacuous. Update the test's doc comment to say why `proef-lsp` is covered: it runs on `Connection::stdio()` (`crates/proef-lsp/src/server.rs:99`), so stdout **is** the JSON-RPC channel and a stray print corrupts protocol framing rather than merely risking an exit code.

- [ ] **Step 6: Prove the widened guard can fail**

Temporarily add `println!("temporary guard check");` inside a function in `crates/proef-lsp/src/server.rs`, then:

```bash
cargo nextest run -p proef --test stderr_hygiene
```

Expected: **FAIL**, naming `server.rs` and the correct line. Then revert:

```bash
git checkout -- crates/proef-lsp/src/server.rs
cargo nextest run -p proef --test stderr_hygiene
```

Expected: **PASS**. Record both outcomes in your report.

- [ ] **Step 7: Add the cross-namespace suggestion test**

In `crates/proef-core/src/resolve.rs`'s test module, beside `missing_config_var_suggests_the_closest_key_in_the_same_namespace`:

```rust
    #[test]
    fn missing_config_var_never_suggests_across_namespaces() {
        // A `vars:` key that is edit-closer than any `url:` key must not be
        // offered for a `${url:…}` typo — candidates are namespace-scoped.
        let f = Fixture::new();
        let err = resolve("${url:nearvar}", &f.ctx(ResolveMode::Strict)).unwrap_err();
        let message = err.to_string();
        assert!(
            !message.contains("did you mean `nearvars`"),
            "suggestion crossed namespaces: {message}"
        );
    }
```

Read the `Fixture` helper in that module first and add a `vars:` key named so it is edit-distance-1 from the `url:` reference you use, adjusting both names to match what the fixture defines. The test must be discriminating: verify it fails if you temporarily drop the `strip_prefix` scoping.

- [ ] **Step 8: Update the changelog**

Extend the existing sections under `## [Unreleased]` in `docs/CHANGELOG.md` (phase 1 already created Added / Changed / Fixed / Documentation — add to them, do not create duplicates):

```markdown
- **Setup and teardown no longer corrupt the run record.** Each phase bracketed
  its own `run_started`/`run_finished`, so one record held up to three pairs and
  `proef explain` reported the last phase's totals — printing "1 passed ·
  0 failed" above a failure it had just listed. The record now carries one pair
  with totals aggregated across phases, and the console run header prints once
  per run instead of once per phase.
- **`report` and `explain` flag a truncated run.** Both rendered an incomplete
  record as if it were whole; `explain` also derived its headline solely from
  the missing tail event, reporting all zeros for a record that held completed
  scenarios. Both now read through the same record reader `diff` uses.
- **`worker` is the slot a scenario occupied, not a per-scenario counter.** The
  timeline drew one lane per scenario regardless of `--jobs`.
- **A scenario with no steps is now an error.** It previously bound to nothing,
  ran nothing, and passed — so a commented-out scenario body stayed green.
- **Run rotation only treats hyphenated UUID directories as run records.** The
  parser also accepted bare 32-hex, `urn:uuid:` and braced spellings, which
  rotation could then delete when the runs directory points somewhere shared.
- The nightly canary can fail again: its step piped through `tee` without
  `pipefail`, so a red canary exited 0 and the open-an-issue step was
  unreachable.
- The raw-print-macro guard now covers `proef-lsp`, where stdout is the
  JSON-RPC channel and a stray print corrupts protocol framing.
```

- [ ] **Step 9: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

Expected: all green.

- [ ] **Step 10: Commit**

```bash
git add .github/workflows/nightly.yml crates/proef-cli/src/fsutil.rs crates/proef-cli/tests/stderr_hygiene.rs crates/proef-core/src/resolve.rs docs/CHANGELOG.md
git commit -m "fix: restore the canary alarm, narrow run-id matching, widen the print guard

The nightly canary piped through tee without pipefail, so the step exited with
tee's zero and the open-an-issue-on-red step was unreachable — the scheduled
alarm for a breaking hurl release could not fire. Naming the shell explicitly
restores pipefail.

is_run_id accepted every spelling the uuid parser does, including bare 32-hex,
while proef only writes the hyphenated form; rotation deletes the oldest
run-shaped directories, so that breadth was a deletion hazard for a shared
runs directory.

The print-macro guard now scans proef-lsp too. Its stdout is the JSON-RPC
channel, so a stray print corrupts protocol framing rather than merely risking
an exit code — a worse failure than the one the guard was first written for."
```

---

## Definition of Done

- A phased run produces exactly one `run_started` and one `run_finished`; `explain` reports the run's verdict, not teardown's; the console header prints once.
- `report` and `explain` read through `record::read_record`, banner an incomplete record, and derive totals from the scenarios present. Both files shrank.
- Three scenarios at `--jobs 1` all stamp `worker: 0`; the EVENTS.md wording was verified either way and the verification stated.
- A zero-step scenario fails; `tests/errors/feature__empty_scenario/` is seeded; `DIAGNOSTICS.md` carries the row and a recounted coverage line.
- `nightly.yml`'s canary step names `shell: bash`; `is_run_id` requires the 36-character form and has tests; the print guard covers `proef-lsp` and was proven able to fail; the cross-namespace suggestion test exists.
- `proef-core` is untouched by Tasks 1–3 (`git status --porcelain crates/proef-core` clean after each).
- Every new test was observed failing before its change, with the RED output recorded in the task report.
- The full six-command gate is green; no version bump; `## [Unreleased]` carries the new entries.
