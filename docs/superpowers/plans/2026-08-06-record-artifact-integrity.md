# Record & Artifact Integrity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **⛔ DO NOT START YET.** PR #13 (`feat/first-run-ux`, 19 commits) is open and `MERGEABLE` but blocked on a GitHub Actions outage. Task 1 completes the record contract that PR made explicit, so it must land first.
>
> When #13 has merged: **cut the work branch fresh off `main`.** Do **not** branch from `docs/tier1-record-integrity`, which carries only this plan and its spec.

**Goal:** Stop the run record and the emitted artifacts from containing wrong data — events after the tail, colliding fake values, phantom capture rows, and an inverted span.

**Architecture:** Four independent fixes, all inside `proef-core`, all pure computation over data already in hand. Two of them (fakes, capture names) change bytes that are snapshot-locked, so their snapshot movement is a reviewed deliverable rather than a side effect.

**Tech Stack:** Rust 2024, `cargo-nextest`, `insta`, `proptest`.

**Approved spec:** `docs/superpowers/specs/2026-08-06-record-artifact-integrity-design.md` — it carries the validated evidence. Cite it; do not re-derive.

**Provenance:** every finding below was reproduced or traced against the tree at `6b15393`. Evidence: `.superpowers/sdd/validation/v053-validation.md`.

## Global Constraints

- **Implementation waits for PR #13 to merge.** Cut the work branch fresh off `main` after that.
- **`proef-core` stays sans-IO**: no IO, no clock reads, no environment reads, no randomness. All four fixes are pure computation over data already in hand.
- No new dependencies. hurl pins stay exactly `hurl = "=8.0.1"`, `hurl_core = "=8.0.1"`.
- The package name for `cargo -p` and `assert_cmd::cargo::cargo_bin` is **`proef`**, NOT `proef-cli`.
- **The event schema is additive-only (ADR-0008).** Task 1 changes event *ordering*, not event *shape* — no variant or field changes.
- **Artifacts are a contract (ADR-0010).** Tasks 2 and 3 move snapshots. Every moved snapshot is reviewed with `cargo insta review` and justified in the task report. **Never blind-accept.**
- If `proef-core`'s public surface changes, regenerate `crates/proef-core/public-api.txt` with `PROEF_PUBLIC_API_UPDATE=1 cargo run -p xtask -- public-api` and report the diff.
- No task ids, plan numbers, or review-section references in code comments. Cite durable ADRs instead.
- No AI-attribution commit trailers.
- **Every test must genuinely fail without its change — demonstrate RED.** This project has repeatedly caught vacuous tests. Task 1's is the specific hazard: asserting that `run_finished` is *present* passes today.
- **No version bump.** PR #13 already forces `0.6.0` and this rides it.

## File Structure

| File | Change | Responsibility |
| --- | --- | --- |
| `crates/proef-core/src/runner.rs` | Modify | Gate late events from finalized scenarios (T1) |
| `crates/proef-core/tests/runner.rs` | Modify | Watchdog ordering test (T1) |
| `crates/proef-core/src/resolve.rs` | Modify | Thread the fake occurrence counter in (T2) |
| `crates/proef-core/src/lower.rs` | Modify | Own the counter across a scenario (T2); export `is_method_line` (T3) |
| `crates/proef-core/src/emit.rs` | Modify | Share the method recogniser (T3); span underflow (T4) |
| `crates/proef-core/src/pack/validate.rs` | Modify | `empty_expect` diagnostic (T4, preferred route) |
| `tests/errors/pack__empty_expect/` | Create | Seeded corpus case (T4, preferred route) |
| `docs/DIAGNOSTICS.md`, `docs/CHANGELOG.md` | Modify | Diagnostic registration (T4), changelog (T2/T4) |

---

### Task 1: No events after `RunFinished`

**Files:**
- Modify: `crates/proef-core/src/runner.rs` (sink handed to workers at `:239`; `sweep_expired` at `:318-356`)
- Test: `crates/proef-core/tests/runner.rs` (append)

**Interfaces:**
- Consumes: `proef_core::event::EventSink` — `#[derive(Clone)]` over `Arc<dyn Fn(&Event) + Send + Sync>`, constructed via `EventSink::new(f)`. `Event::{ScenarioStarted, StepFinished, ScenarioFinished, RunFinished}` all carry `scenario` and `file` fields identifying their scenario.
- Produces: nothing later tasks depend on.

**The bug.** `sweep_expired`'s own doc comment (`runner.rs:314-317`) states the intent verbatim — *"the cancelled token is the thread's signal to stop appending to the record"* — and nothing enforces it. The sweep cancels the child token and emits that scenario's `ScenarioFinished` (`:348-354`) from the **dispatcher** thread, then `run()` proceeds to `RunFinished`. Meanwhile the abandoned scenario's own thread (spawned per scenario at `:370`) only notices its token at the next batch boundary and keeps emitting — after its own terminal event, and (reproduced) **after the run's `RunFinished`**.

`docs/EVENTS.md:9-10` says *"the last is `run_finished`"*, and that wording predates this work (`91bdd59`). So this is a pre-existing violation of an existing contract: **a bug fix, no new ADR.** ADR-0007 is unchanged — abandonment stays cooperative; what changes is that a finalized scenario's late events never reach the sink.

- [ ] **Step 1: Write the failing test**

Append to `crates/proef-core/tests/runner.rs`. Model the setup on `watchdog_abandons_a_hung_scenario` (`:518`), which uses `MisbehavingFactory(Misbehavior::Hang)` and a 50ms `default_batch_budget` — read it first. Unlike that test, this one needs a **recording** sink rather than `EventSink::null()`:

```rust
/// The record's tail must actually be the tail. A watchdog-abandoned
/// scenario's thread is detached and notices its token only at the next batch
/// boundary, so without a gate it keeps emitting after the run is finalized.
#[test]
fn abandoned_scenario_emits_nothing_after_run_finished() {
    let engines = engines(vec![Box::new(MisbehavingFactory(Misbehavior::Hang))]);
    let store = Arc::new(Mutex::new(GlobalStore::new()));
    let mut config = config(1);
    config.default_batch_budget = Duration::from_millis(50);

    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let seen = Arc::clone(&seen);
        EventSink::new(move |event| {
            let label = match event {
                Event::RunStarted { .. } => "run_started",
                Event::RunFinished { .. } => "run_finished",
                Event::ScenarioStarted { .. } => "scenario_started",
                Event::ScenarioFinished { .. } => "scenario_finished",
                Event::StepFinished { .. } => "step_finished",
                _ => "other",
            };
            seen.lock().unwrap().push(label.to_owned());
        })
    };

    let _summary = run(
        vec![spec("hangs", &["misbehaving"])],
        &engines,
        &store,
        &config,
        &sink,
        &CancellationToken::new(),
    );

    // Give the abandoned thread time to reach its next boundary and try to
    // emit. Without the gate it appends here; with it, nothing arrives.
    std::thread::sleep(Duration::from_millis(500));

    let events = seen.lock().unwrap().clone();
    let tail = events
        .iter()
        .rposition(|e| e == "run_finished")
        .expect("record must contain run_finished");
    assert_eq!(
        tail,
        events.len() - 1,
        "run_finished must be the LAST event, got {:?} after it: {events:?}",
        &events[tail + 1..]
    );
}
```

**The assertion is on position, not presence.** `assert!(events.contains("run_finished"))` passes today and would be vacuous — that is the exact failure mode this project keeps catching.

Add any missing imports (`Event`, `EventSink`, `Arc`, `Mutex`, `Duration`) to the test file's existing import block.

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo nextest run -p proef-core abandoned_scenario_emits_nothing 2>&1 | tail -25
```

Expected: **FAIL**, with the message listing the events that arrived after `run_finished`. Paste the verbatim output into your report. If it passes, the sleep may be too short for the hang's boundary — raise it once and re-run; if it still passes, STOP and report BLOCKED rather than weakening the assertion.

- [ ] **Step 3: Gate late events centrally**

Add a wrapper in `runner.rs` that drops events for scenarios the run has already finalized, and hand **that** to workers instead of the raw sink. The natural seam is `:239`, where each worker receives `events.clone()`.

```rust
/// Wraps the run's sink so a finalized scenario's late events never reach the
/// record. An abandoned scenario's thread is detached and observes its
/// cancellation token only at its next batch boundary (ADR-0007), so it can
/// still try to emit after the sweep recorded its outcome — and after the run
/// itself was finalized. The record's tail must be the tail.
#[derive(Clone)]
struct RecordGate {
    inner: EventSink,
    closed: Arc<Mutex<HashSet<(Arc<str>, Arc<str>)>>>,
    run_closed: Arc<AtomicBool>,
}
```

Requirements the implementation must satisfy — choose the shape, but meet all of these:

1. **Central, not per-emitter.** Exactly one place decides. Do not add a check at each `events.emit` site.
2. **Safe across threads.** The sweep runs on the dispatcher thread; workers each run on their own. Both go through the gate.
3. **Keyed on scenario identity** `(scenario, file)` — the same identity pairing used elsewhere in the runner. A worker's late events are dropped once that scenario is finalized.
4. **Run-level close.** After `RunFinished` is emitted, nothing further reaches the sink at all.
5. **`RunFinished` itself must pass.** Order the close so the tail is written, then the gate shuts.

Mark a scenario closed wherever its terminal `ScenarioFinished` is emitted — both the normal `Msg::Done` path and `sweep_expired` (`:348`).

Sans-IO holds: this is bookkeeping over data already in hand — no IO, clock, env, or randomness.

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo nextest run -p proef-core abandoned_scenario_emits_nothing
```

Expected: **PASS**. Also re-run the neighbouring watchdog tests:

```bash
cargo nextest run -p proef-core watchdog
```

Expected: still green — the gate must not suppress the sweep's own terminal event.

- [ ] **Step 5: Full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

Watch the event-stream snapshots. A well-behaved run emits nothing late, so they should not move; if one does, inspect it and justify it in your report.

- [ ] **Step 6: Commit**

```bash
git add crates/proef-core/src/runner.rs crates/proef-core/tests/runner.rs
git commit -m "fix(core): keep run_finished the last line of the record

An abandoned scenario's thread is detached and notices its cancellation token
only at its next batch boundary, so it kept appending events after the sweep
had recorded its outcome — and after the run itself was finalized. The record
contract says the last line is run_finished; it was not.

Late events from a finalized scenario are now dropped at a single gate rather
than by asking every emitter to check. Abandonment stays cooperative
(ADR-0007); only the record's tail changes."
```

---

### Task 2: `${fake:*}` must not collide across steps

**Files:**
- Modify: `crates/proef-core/src/resolve.rs` (`Resolution` at `:62`, `resolve()` at `:168-169`, the counter at `:317-318`)
- Modify: `crates/proef-core/src/lower.rs` (`resolve::resolve` call at `:185`, and the JSON-string call at `:372`)
- Modify: `docs/CHANGELOG.md`

**Interfaces:**
- Consumes: `crate::fake::generate(run_id, occurrence, kind) -> Option<String>` — deterministic in exactly those three inputs.
- Produces: whatever signature change `resolve()` takes. State it in your report so a later reader can follow it.

**The bug.** `resolve()` builds a fresh `Resolution::default()` per call (`resolve.rs:169`); the occurrence counter is `resolution.fakes`, read and incremented at `:317-318` and fed to `fake::generate`. `resolve()` is called **once per step** (`lower.rs:185`), so the counter restarts every step. Reproduced: two steps each with a fresh `${fake:email}` emitted the identical address. `docs/AUTHORING.md` reads as though each reference is independent.

**The fix threads a counter in rather than resetting per call.** Decide whether the scope is per-scenario or per-run, and **justify the choice in your report**.

**HARD REQUIREMENT — determinism.** The new scope must remain a pure function of inputs the run already fixes (`run_id` plus the suite's own structure), so the same `run_id` reproduces the same artifacts. Anything keyed on wall-clock, iteration order across threads, or thread identity breaks **both** ADR-0010 (artifacts-as-contract) and `proef-core`'s sans-IO purity. Do not do it.

**This changes snapshot-locked artifact bytes** for any suite using `${fake:*}`. That is expected and deliberate.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/proef-core/src/resolve.rs`, or as an integration test if the counter's new owner lives in `lower.rs` — put them where they can observe two steps in one scenario. Read the module's existing `Fixture` helper first.

```rust
    #[test]
    fn fake_values_do_not_collide_across_steps_in_a_scenario() {
        // Two steps, each with its own `${fake:email}`, must get distinct
        // values — the counter belongs to the scenario, not to one resolve().
        let (first, second) = resolve_two_steps("${fake:email}", "${fake:email}");
        assert_ne!(
            first, second,
            "two steps' fake values collided: {first} == {second}"
        );
    }

    #[test]
    fn fake_values_are_reproducible_for_the_same_run_id() {
        // Determinism is the property that makes artifacts a contract
        // (ADR-0010): the same run id must reproduce the same bytes.
        let first_run = resolve_two_steps("${fake:email}", "${fake:email}");
        let second_run = resolve_two_steps("${fake:email}", "${fake:email}");
        assert_eq!(first_run, second_run, "same run id produced different fakes");
    }
```

Write `resolve_two_steps(a: &str, b: &str) -> (String, String)` as a helper that resolves two steps under one scenario with a **fixed** `run_id`, mirroring how `lower.rs` drives `resolve()`. Both runs in the second test must use the same fixed `run_id`.

- [ ] **Step 2: Run them to verify the first fails**

```bash
cargo nextest run -p proef-core fake_values_ 2>&1 | tail -20
```

Expected: `fake_values_do_not_collide_across_steps_in_a_scenario` **FAILS** (the two values are equal); the reproducibility test passes both before and after — it is a guard, not a RED. Paste the verbatim output.

- [ ] **Step 3: Thread the counter**

Change `resolve()` so the occurrence counter is supplied by the caller and carried across a scenario's steps rather than reset per call, and update both call sites (`lower.rs:185` and `:372`). Keep `Resolution`'s other fields as they are.

Update `docs/AUTHORING.md` **only if** its current wording is now wrong; if it already describes independent values, it becomes true rather than needing an edit. State which in your report.

- [ ] **Step 4: Verify GREEN, then review the snapshots**

```bash
cargo nextest run -p proef-core fake_values_
cargo nextest run --profile ci 2>&1 | tail -20
```

Artifact snapshots covering `${fake:*}` **will** move. For each:

```bash
cargo insta review
```

**Confirm the diff shows only fake values changing** — no structural change, no other field. Record every moved snapshot's name and its diff in your report. If a snapshot moves in any other way, STOP and report it: that means the change reached further than intended.

- [ ] **Step 5: Changelog**

Add to `docs/CHANGELOG.md` under `## [Unreleased]` → **`### Changed`** (not `### Fixed` — this alters emitted artifact bytes):

```markdown
- **`${fake:…}` values no longer repeat across a scenario's steps.** The
  occurrence counter restarted on every step, so two steps each asking for a
  fresh `${fake:email}` received the same address. Values remain deterministic
  for a given `--run-id`, but suites using `${fake:…}` will see their emitted
  artifacts change.
```

- [ ] **Step 6: Full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

If `resolve()`'s signature changed and it is public, regenerate the public-api snapshot and include the diff.

- [ ] **Step 7: Commit**

```bash
git add crates/proef-core/src docs/CHANGELOG.md
git commit -m "fix(core): give each fake reference its own occurrence

The occurrence counter lived on a Resolution built fresh inside resolve(),
which runs once per step — so it restarted at every step and two steps each
asking for a fresh \${fake:email} got the same address, while the authoring
guide describes every reference as independent.

The counter now spans the scenario. Values stay a pure function of the run id,
so a pinned --run-id still reproduces its artifacts byte for byte; the bytes
themselves change for any suite using \${fake:…}."
```

---

### Task 3: `.map.json` phantom capture rows

**Files:**
- Modify: `crates/proef-core/src/emit.rs` (`capture_names` at `:249-296`)
- Modify: `crates/proef-core/src/lower.rs` (`is_method_line` at `:583` — make it reachable from `emit`)

**Interfaces:**
- Consumes: `is_method_line(trimmed: &str) -> bool`, defined at `crates/proef-core/src/lower.rs:583` and used there at `:425`, `:458`, `:607`.
- Produces: nothing later tasks depend on.

**The bug.** `capture_names` is fence-unaware — a fenced `[Captures]` line re-arms the scan — and it does not recognise custom methods, so a `PROPFIND` entry fails to terminate it. Phantom rows land in `.map.json`, which is a **normative artifact** (ADR-0010).

**Share the existing recogniser.** `lower.rs:583` already gets methods right. Do not write a second one — one canonical mechanism. Make it `pub(crate)` (or move it to a shared spot) rather than duplicating.

- [ ] **Step 1: Write the failing test**

Add to `crates/proef-core/src/emit.rs`'s test module (or the emitter's integration tests, wherever `capture_names` is currently exercised — check first):

```rust
    #[test]
    fn capture_scan_ignores_fenced_lines_and_ends_at_custom_methods() {
        // A fenced `[Captures]` must not re-arm the scan, and a custom method
        // must end the previous entry — otherwise phantom rows reach the
        // sidecar, which is a normative artifact (ADR-0010).
        let hurl = concat!(
            "GET http://x/a\n",
            "HTTP 200\n",
            "[Captures]\n",
            "real: jsonpath \"$.id\"\n",
            "\n",
            "PROPFIND http://x/b\n",
            "```\n",
            "[Captures]\n",
            "phantom: jsonpath \"$.nope\"\n",
            "```\n",
            "HTTP 207\n",
        );
        let names = capture_names(hurl);
        assert!(names.contains(&"real".to_owned()), "{names:?}");
        assert!(
            !names.contains(&"phantom".to_owned()),
            "fenced capture leaked into the sidecar: {names:?}"
        );
    }
```

Adjust the call to `capture_names`' real signature and return type — read it at `:249` first.

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo nextest run -p proef-core capture_scan_ignores_fenced 2>&1 | tail -20
```

Expected: **FAIL** — `phantom` appears. Paste the verbatim output.

- [ ] **Step 3: Fix the scan**

Make `capture_names` fence-aware and have it use `is_method_line` to detect the start of a new entry. Track fence state as the scan walks lines; a line inside a fence contributes nothing.

- [ ] **Step 4: Verify GREEN and review snapshots**

```bash
cargo nextest run -p proef-core capture_scan_ignores_fenced
cargo nextest run --profile ci 2>&1 | tail -20
```

Sidecar snapshots may move if any corpus pack contains a fenced `[Captures]` or a custom method. Review each with `cargo insta review`, confirm only phantom rows disappear, and record the diffs.

- [ ] **Step 5: Full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

- [ ] **Step 6: Commit**

```bash
git add crates/proef-core/src
git commit -m "fix(core): stop phantom captures reaching the sidecar

The capture scan re-armed on a fenced [Captures] line and did not treat a
custom method as the start of a new entry, so names that never existed were
written into .map.json — a normative artifact. The scan is now fence-aware and
shares the lowering pass's method recogniser instead of carrying a second,
weaker copy."
```

---

### Task 4: Inverted sidecar span from a whitespace-only `expect:`

**Files:**
- Modify: `crates/proef-core/src/pack/validate.rs` (preferred route)
- Modify: `crates/proef-core/src/emit.rs:211` (fallback route only)
- Create: `tests/errors/pack__empty_expect/case.feature`, `tests/errors/pack__empty_expect/packs/broken.yaml`
- Modify: `docs/DIAGNOSTICS.md`, `docs/CHANGELOG.md`

**Interfaces:**
- Consumes: the diagnostic construction style used by neighbouring `pack::` codes in `validate.rs` — read one before writing.
- Produces: diagnostic code `proef::pack::empty_expect` (preferred route).

**The bug.** A whitespace-only `expect:` fragment yields `MergedAsserts { lines: 0 }`, and the `start + lines - 1` computation at `emit.rs:211` underflows into a span of `[9,8]` — start greater than end.

**PREFERRED FIX: reject at pack-validation time** with a new `proef::pack::empty_expect` diagnostic. This matches how the project already rejects zero-entry payloads and its stated posture of failing loudly rather than emitting something degenerate. A saturating span would produce a *valid-looking* artifact for input that means nothing.

**FALLBACK: saturate instead** — but only if validation rejection breaks an existing corpus.

- [ ] **Step 1: Check the repo's own corpora first**

Before writing anything, find out whether any existing pack has an empty or whitespace-only `expect:`:

```bash
rg -n -A 3 'expect:' tests/ crates/*/tests/ | rg -B 1 -A 2 'expect:\s*$'
cargo run -q -p proef -- test --dry-run tests/features ; echo "exit=$?"
```

**Report exactly what you find.** If a corpus pack would newly fail, take the fallback route and record why — do **not** edit a corpus to fit the diagnostic. That would be fitting the evidence to the fix.

- [ ] **Step 2: Write the failing corpus case**

Create `tests/errors/pack__empty_expect/case.feature`:

```gherkin
Feature: E
  Scenario: S
    When I check the thing
```

Create `tests/errors/pack__empty_expect/packs/broken.yaml`:

```yaml
macros:
  check:
    match: I check the thing
    steps:
      - hurl: |
          GET http://x/health
          HTTP 200
  empty:
    match: nothing binds this
    expect:
      - hurl: |

```

Note the corpus is dry-run from the repo root, so it inherits the root `proef.toml` — do **not** add a local one (this mirrors `tests/errors/resolve__unknown_variable/`, which is exactly two files).

- [ ] **Step 3: Run it to verify it currently passes (the bug)**

```bash
cargo run -q -p proef -- test --dry-run tests/errors/pack__empty_expect ; echo "exit=$?"
```

Expected **before the fix**: `exit=0`. `tests/errors/` is supposed to fail by design, so a green case here *is* the defect. Paste the verbatim output as the RED evidence.

- [ ] **Step 4: Add the diagnostic**

In `crates/proef-core/src/pack/validate.rs`, reject a macro whose `expect:` fragment has no non-whitespace content, with code `proef::pack::empty_expect`, the fragment's span, and a help line saying an `expect:` must carry at least one assert line. Match the neighbouring diagnostics' construction style exactly.

- [ ] **Step 5: Verify the corpus case now fails**

```bash
cargo run -q -p proef -- test --dry-run tests/errors/pack__empty_expect ; echo "exit=$?"
```

Expected: `exit=2`, one error, code `proef::pack::empty_expect`.

- [ ] **Step 6: Register the diagnostic**

In `docs/DIAGNOSTICS.md`: add a row for `proef::pack::empty_expect` in the `pack::` group, matching neighbouring rows' column layout, with the corpus column ticked (copy the exact marker a seeded row uses). Then update the coverage sentence — it currently reads "25 of the 60 codes carry a seeded corpus case today". **Both numbers change.** Recount each from the table rather than incrementing, and state the numbers you counted in your report.

- [ ] **Step 7: Changelog**

Add to `docs/CHANGELOG.md` under `## [Unreleased]` → `### Fixed`:

```markdown
- An `expect:` fragment with no assert lines is now rejected at pack load. It
  previously produced an empty asserts block and an inverted span in the
  emitted sidecar, where the start offset exceeded the end.
```

If you took the fallback route, describe the saturation instead.

- [ ] **Step 8: Full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

The corpus guard walks `tests/errors/` and may generate a snapshot for the new case. Inspect it, confirm it matches the dry-run output you observed in Step 5, and say so.

- [ ] **Step 9: Commit**

```bash
git add crates/proef-core/src tests/errors/pack__empty_expect docs/DIAGNOSTICS.md docs/CHANGELOG.md
git commit -m "fix(core): reject an expect: fragment with no assert lines

A whitespace-only expect: lowered to a merged-asserts step carrying zero lines,
and the sidecar span computation underflowed into a start offset greater than
its end — a malformed region in a normative artifact.

An empty expect: has no meaning, so it now fails at pack load like the other
degenerate payloads rather than emitting something that only looks valid."
```

---

## Definition of Done

- A watchdog-abandoned scenario's record has `run_finished` as its **last** event, asserted by position.
- Two steps in one scenario asking for the same `${fake:kind}` get different values, and a pinned `--run-id` still reproduces both exactly. Every moved artifact snapshot was reviewed and shown to differ only in fake values.
- A fenced `[Captures]` line and a `PROPFIND` entry produce no phantom rows in `.map.json`, using the lowering pass's method recogniser rather than a second copy.
- A whitespace-only `expect:` is rejected (or, on the recorded fallback, produces a non-inverted span); `tests/errors/pack__empty_expect/` is seeded; `DIAGNOSTICS.md` carries the row and both recounted numbers.
- Every new test was observed failing before its fix, with the RED output in the task report.
- `proef-core` remains sans-IO; the event schema is unchanged in shape; the full six-command gate is green; no version bump.
