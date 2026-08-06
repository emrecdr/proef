# Design: run-record correctness and drift guards

**Date:** 2026-08-06
**Status:** Approved
**Source:** external v0.5.3 review, validation round 6. Only findings I personally reproduced or traced are in scope — see "Out of scope".
**Branch:** `feat/first-run-ux` (phase 2; phase 1 is `03b442f..59f9243`). Same branch, same PR.

## Problem

The observability surface added by ADR-0014/0015 corrupts the run record badly
enough that `proef explain` reports **"1 passed · 0 failed" for a failed run**,
and a truncated record reads as an empty-but-successful one. Separately, a
scenario with no steps passes silently, the nightly canary alarm cannot fire,
run-dir rotation accepts more directory shapes than proef ever writes, and the
print-macro guard does not cover the crate where a stray print is worst.

## Verified facts (do not re-derive)

Reproduced or traced against `59f9243`.

| Fact | Citation |
| --- | --- |
| `runner::run` emits `RunStarted`/`RunFinished` unconditionally | `crates/proef-core/src/runner.rs:182`, `:299` |
| `execute` calls it up to three times — setup, main, teardown | `crates/proef-cli/src/exec.rs:213→593`, `:271`, `:278→593` |
| `RunTotals::observe` **assigns** on `RunFinished`, so the last pair wins | `crates/proef-core/src/report.rs:325-328` |
| The CLI already composes event sinks by wrapping | `crates/proef-cli/src/exec.rs:171`, `:480-489` |
| `ConsoleReporter` reacts to both `RunStarted` and `RunFinished` | `crates/proef-core/src/report.rs:217`, `:278` |
| `RunCompletion { Completed, Cancelled, Incomplete }` and `Record` **already exist** and are used by `diff` | `crates/proef-cli/src/record.rs:70-88` |
| `explain` and `report` hand-roll their own line loops instead | `crates/proef-cli/src/explain.rs`, `report.rs` |
| `stamp_scenario_timing` assigns `map.len()` per unseen `ThreadId` | `crates/proef-cli/src/exec.rs:482-489` |
| The runner spawns a thread per scenario, so ids are never reused | `crates/proef-core/src/runner.rs:370` |
| The dispatcher bounds concurrency via `active`, keyed by scenario index | `crates/proef-core/src/runner.rs:201-232` |
| A zero-step scenario dry-runs `ok … 0 step(s)`, 0 warnings, exit 0 | reproduced |
| `nightly.yml` pipes the canary to `tee`, with no `shell:` or `pipefail` in the file | `.github/workflows/nightly.yml:41` |
| `ci.yml`'s canary is unpiped | `.github/workflows/ci.yml:150` |
| hurl 8.0.1 (2026-04-29) is still the latest stable — nothing missed yet | releases API |
| `is_run_id` is a bare `Uuid::try_parse`, no length guard | `crates/proef-cli/src/fsutil.rs:47-49` |
| The print-macro guard scans `CARGO_MANIFEST_DIR/src` — proef-cli only | `crates/proef-cli/tests/stderr_hygiene.rs:38` |
| `proef-lsp` runs on `Connection::stdio()`, so **stdout is the protocol** | `crates/proef-lsp/src/server.rs:99` |

## Decisions

### D1 — One `RunStarted`/`RunFinished` per record, via a CLI sink wrapper

Add a sink wrapper beside `stamp_scenario_timing` that **drops** `RunStarted`
and `RunFinished`. Wrap every `runner::run` call with it; the CLI emits one
`RunStarted` before the phases and one `RunFinished` after, with totals
aggregated from the three `RunSummary` values it already holds.

`proef-core` is **not touched**: `runner::run` keeps its signature, the public
API snapshot does not move, and the event schema is unchanged — so ADR-0008's
additive-only rule is never engaged. The record simply matches what EVENTS.md
has always claimed.

The additive-`phase`-field alternative is rejected: it would leave three
head/tail pairs in the file, so the record contract stays violated while every
existing consumer needs teaching. It disambiguates the symptom without fixing
the cause.

### D2 — The triple console header is fixed by D1, not separately

`ConsoleReporter` reacts to `RunStarted` (`report.rs:217`) and `RunFinished`
(`:278`). Once phase head/tail never reach the sink, the run header and summary
print once per run instead of once per phase. One fix, two symptoms — treating
them separately would mean two mechanisms for one cause.

### D3 — `report` and `explain` move onto the existing record reader

Both switch to `record::read_record` and banner loudly when `completion` is
`Incomplete`. This is net **less** code: their hand-rolled line loops are
deleted.

The duplication *caused* the bug. `diff` got the completion guard in 0.5.2
because it reads through `read_record`; `report` and `explain` missed it because
each re-implemented the read. Consolidating is the fix, not tidying after it.

Reproduced severity, worse than the source review states: `explain`'s headline
derives solely from `RunFinished`, so a truncated record prints
`0 passed · 0 failed · 0 skipped` at exit 0 **even when the record contains a
passed scenario**. The banner is necessary but not sufficient — the totals must
come from the scenario outcomes the record actually holds.

### D4 — `worker` becomes a real slot index, fixed CLI-side

In the stamping closure, keep a free-list of slot indices: assign the lowest
free slot when a scenario starts, release it when that scenario finishes.

Release must key on **scenario identity, not `ThreadId`** — an abandoned
scenario's `ScenarioFinished` is emitted by the watchdog sweep on the dispatcher
thread, not the worker thread. Keying on the thread would leak slots exactly
when the watchdog fires.

No core change and no event-shape change: `worker` already exists and is already
stamped here; only the value becomes correct. EVENTS.md's "0-based worker index"
becomes true rather than needing a rewrite.

The existing snapshot test uses one scenario, where a per-scenario ordinal and a
worker slot are numerically identical — the test encodes the bug. The regression
test must use **2+ scenarios at `--jobs 1`** and assert every event stamps
`worker: 0`.

### D5 — A zero-step scenario is a hard error

New diagnostic `proef::feature::empty_scenario` at bind time, plus a seeded case
under `tests/errors/`.

Error, not warning: the project's stated invariant is never green while silently
running nothing, and a warning still exits 0 — narrowing the hole rather than
closing it. A commented-out scenario body currently stays green in CI, which is
precisely the failure this closes. Suites that keep deliberate placeholders will
break; that is the intended signal.

### D6 — `shell: bash` on the nightly canary step

GitHub's default `run:` shell is `bash -e {0}` **without** `pipefail`, so
`… | tee canary.log` exits with `tee`'s 0 and the `if: failure()` issue step is
unreachable. Naming `shell: bash` explicitly gets `-o pipefail`.

Nothing has been missed yet — hurl 8.0.1 is still latest — so this is
pre-emptive, and worth doing before 8.1 lands.

### D7 — `is_run_id` requires the hyphenated form

`name.len() == 36 && Uuid::try_parse(name).is_ok()`. `Uuid::try_parse` also
accepts bare 32-hex, `urn:uuid:…`, and braced spellings; proef only ever writes
hyphenated UUIDv7. Rotation `remove_dir_all`s the oldest run-shaped directories
beyond the retention limit, and the runs dir may point somewhere shared, so
breadth here is a deletion hazard. Add the rotation test that does not exist.

### D8 — The print-macro guard covers `proef-lsp`

Extend the scan to `crates/proef-lsp/src`. **This reverses an earlier decision
of mine**, and the reason is worth recording: I declined this guard on cost
grounds after weighing EPIPE and exit codes. That reasoning was incomplete.
`proef-lsp` runs on `Connection::stdio()`, so stdout *is* the JSON-RPC channel —
a stray `println!` corrupts protocol framing and breaks the editor session,
which is a worse failure than the one I was weighing.

Prefer widening the existing test's scan roots over duplicating the test into a
second crate, so there stays one implementation of the rule.

Also add the cross-namespace regression test the phase-1 review deferred: a
`vars:` key edit-closer than any `url:` key must not be suggested for a `url:`
typo. That scoping is currently proven only by inspection.

## Out of scope

- **The `--rerun`-after-phase-failure interaction** the source review mentions
  inside its P1-1. I did not independently reproduce it; this fold-in is
  validated-only.
- The review's remaining ~37 items (six other P2s, the whole P3/P4 batch), which
  the review itself marks as code-read-only. They need their own validation pass
  before any of them is acted on.

## Testing

| Decision | Test |
| --- | --- |
| D1/D2 | a setup+failing-main+teardown run produces exactly one `run_started` and one `run_finished`; `explain` reports the failure, not teardown's totals; console header appears once |
| D3 | a truncated record makes `report` and `explain` banner as incomplete, and `explain`'s totals reflect the scenarios actually present |
| D4 | 2+ scenarios at `--jobs 1` stamp `worker: 0` on every event |
| D5 | a zero-step scenario fails; seeded `tests/errors/` case |
| D6 | not unit-testable — verify by reading the rendered workflow and confirming `shell: bash` is present |
| D7 | rotation seeded with hyphenated-uuid, 32-hex, and other directory names deletes only the first kind |
| D8 | reintroduce a `println!` in `proef-lsp/src`, confirm the guard fails, revert |

Every test must fail without its change. D4's must use more than one scenario —
the existing single-scenario snapshot is why this survived.

## Delivery

- Same branch and PR as phase 1: `feat/first-run-ux`.
- Changelog under `## [Unreleased]`, extending the sections phase 1 created.
- No version bump.
- Full gate: `cargo fmt --all --check`;
  `cargo clippy --all-targets --all-features -- -D warnings`;
  `cargo nextest run --profile ci`; `cargo test --doc`;
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace`;
  `cargo run -p xtask -- docs-check`.

## Constraints

- `proef-core` stays sans-IO and, under D1/D3/D4, entirely untouched.
- No new dependencies; hurl pins stay `=8.0.1`.
- The event schema does not change, so ADR-0008's additive-only rule is not
  engaged. Any documentation claim that becomes true (EVENTS.md's worker index)
  or was already true (one head/tail per record) needs no rewrite — verify both.
- One canonical mechanism per outcome: D3 removes a duplicate reader rather than
  adding a guard to each; D8 widens one scan rather than copying a test.
- No raw print macros in `proef-cli` or, after D8, `proef-lsp`.
- No task ids, plan numbers, or review-section references in code comments.
- No AI-attribution commit trailers.
