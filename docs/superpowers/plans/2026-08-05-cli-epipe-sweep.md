# proef-cli EPIPE-Safety Sweep Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make exit code 101 unreachable from a closed stderr pipe anywhere in `proef-cli`, and keep it that way.

**Architecture:** Convert all 73 raw `eprintln!` calls in `crates/proef-cli/src/` to the existing `crate::render::errln!` macro, which swallows write failures instead of panicking. A source-scanning integration test then enforces zero raw `eprintln!` in that directory, so the rule needs no allowlist and cannot drift. A fixture-backed closed-pipe test pins the behavior on the execution path — the largest stderr emitter.

**Tech Stack:** Rust 2024, `cargo-nextest`, `assert_cmd`, `proef-fixture` (in-process test API server), `tempfile`.

**Approved spec:** `docs/superpowers/specs/2026-08-05-cli-epipe-sweep-design.md` — it carries the verified `file:line` facts. Cite them; do not re-derive.

**Branch:** `fix/cli-epipe-sweep`, already created off `main` (`ac84d13` = v0.5.2). Spec committed at `6593be7`.

## Global Constraints

- `proef-core` is untouched. This is a `proef-cli`-only change.
- No new dependencies. hurl pins stay exactly `hurl = "=8.0.1"`, `hurl_core = "=8.0.1"`.
- The package name for `cargo -p` and `assert_cmd::cargo::cargo_bin` is **`proef`**, NOT `proef-cli`.
- **No behavior change for any currently-working path.** Converting `eprintln!` to `errln!` preserves the message text, the format arguments, and the stream. It changes only what happens when the write itself fails.
- No task ids, plan numbers, or fix-pass section references in code comments. The changelog carries those. Cite durable ADRs instead.
- No AI-attribution commit trailers.
- The closed-pipe test must **genuinely fail before the sweep**. Demonstrate the RED empirically; never assume it.
- No version bump. Both changes ride the next release under `## [Unreleased]`.

## Commit Structure (deviation, read this)

The plan brief asked for one commit per task. Task 1 produces a **deliberately failing** test, so committing it alone would put a red commit in history and break bisectability. Instead:

- **Task 1 ends with recorded RED evidence and no commit.**
- **Task 2 commits the test and the sweep together** (the test goes green in the same commit that earns it).
- **Task 3 commits separately.**

The RED-observation step in Task 1 is mandatory and must be recorded verbatim in the task report. It is the whole reason the test is trustworthy.

## File Structure

| File | Change | Responsibility |
| --- | --- | --- |
| `crates/proef-cli/tests/execute.rs` | Modify (append one test) | Closed-pipe regression on the execution failure-summary path |
| `crates/proef-cli/src/*.rs` (12 files) | Modify | 73 `eprintln!` → `crate::render::errln!` |
| `crates/proef-cli/tests/stderr_hygiene.rs` | Create | Drift guard: zero raw `eprintln!` under `src/` |
| `crates/proef-cli/src/report.rs` | Modify (`:46`) | Reuse `fsutil::parent_dir` for the output-dir derivation |
| `docs/CHANGELOG.md` | Modify | `## [Unreleased]` entries |

Note: `report.rs` is touched by **both** Task 2 (3 `eprintln!` sites at `:15`, `:22`, `:35`) and Task 3 (the `:46` parent derivation). These are different lines and do not conflict.

---

### Task 1: RED-first closed-pipe test for the execution failure summary

**Files:**
- Modify: `crates/proef-cli/tests/execute.rs` (append at end of file)

**Interfaces:**
- Consumes (already present in `execute.rs`): the module-level const `BASE_URL_CONFIG: &str`, and the imports `use proef_fixture::{API_TOKEN, Fixture};`. `Fixture::start()` returns `Result<Fixture, _>`; the struct exposes the field `base_url`.
- Produces: the test function `failure_summary_does_not_panic_on_a_closed_stderr_pipe`, which Task 2 must flip from RED to GREEN.

**Background the implementer needs:**

`eprintln!` panics when its write fails. Rust sets `SIGPIPE` to `SIG_IGN` at startup, so writing to a pipe whose reader has closed returns an `EPIPE` error rather than killing the process — and the resulting panic aborts with **exit 101**, which is not one of the four codes the exit contract permits (`0` ok, `1` test failure, `2` user error, `3` system error — ADR-0009).

The execution failure summary (`crates/proef-cli/src/exec.rs:307-352`, raw calls at `:314`, `:326`, `:333`, `:351`) emits one fault line, one failed-step line, a curl hint, and a reproduce line **per failing scenario**. Several failing scenarios are required so the summary emits enough bytes that a write reliably lands after the reader has closed, rather than racing a single short line.

This test does **not** use the `proef_in` helper. That helper returns an `assert_cmd::Command`, which cannot be spawned with piped stdio. The test builds a `std::process::Command` directly and reproduces what `proef_in` does: writes `proef.toml`, sets `NO_COLOR`, `PROEF_BASE_URL`, and `PROEF_SECRET_APITOKEN`.

- [ ] **Step 1: Append the failing test to `crates/proef-cli/tests/execute.rs`**

Add at the end of the file:

```rust
/// A closed stderr pipe must not panic the execution path. `head -c0` reads
/// nothing and exits, closing the read end, so every later stderr write gets
/// EPIPE — and a raw `eprintln!` would abort with 101, outside the typed
/// 0/1/2/3 exit taxonomy (ADR-0009). Unix-only because EPIPE and `head` are
/// POSIX; the guard under test is cross-platform.
#[cfg(unix)]
#[test]
fn failure_summary_does_not_panic_on_a_closed_stderr_pipe() {
    let fixture = Fixture::start().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("suite/packs")).unwrap();
    std::fs::write(cwd.path().join("proef.toml"), BASE_URL_CONFIG).unwrap();

    // Several failing scenarios: the summary writes a fault line, a failed-step
    // line, a curl hint and a reproduce line per failure, so the write lands
    // well after the reader closes instead of racing one short line.
    let mut feature = String::from("# baseURL: ${env:PROEF_BASE_URL}\nFeature: F\n");
    for n in 1..=6 {
        feature.push_str(&format!(
            "  Scenario: health case {n}\n    When the health endpoint is checked\n"
        ));
    }
    std::fs::write(cwd.path().join("suite/case.feature"), feature).unwrap();
    // The fixture answers /health with 200; asserting 500 fails every scenario.
    std::fs::write(
        cwd.path().join("suite/packs/p.yaml"),
        "macros:\n  health:\n    match: the health endpoint is checked\n    steps:\n      - hurl: |\n          GET ${url:base}/health\n          HTTP 500\n",
    )
    .unwrap();

    let bin = assert_cmd::cargo::cargo_bin("proef");
    let mut proef = std::process::Command::new(&bin)
        .current_dir(cwd.path())
        .env("NO_COLOR", "1")
        .env("PROEF_BASE_URL", &fixture.base_url)
        .env("PROEF_SECRET_APITOKEN", API_TOKEN)
        .args(["test", "suite"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Consume nothing, then drop the reader to close the pipe early.
    let mut head = std::process::Command::new("head")
        .args(["-c", "0"])
        .stdin(proef.stderr.take().unwrap())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let _ = head.wait();
    let status = proef.wait().unwrap();
    // The exact non-zero code doesn't matter; 101 (panic) must NOT occur.
    assert_ne!(
        status.code(),
        Some(101),
        "the execution failure summary panicked on EPIPE"
    );
}
```

- [ ] **Step 2: Run the test against unmodified `src/` and record the RED**

Run:

```bash
cargo nextest run -p proef --test execute failure_summary_does_not_panic_on_a_closed_stderr_pipe
```

Expected: **FAIL**, with the assertion reporting `Some(101)` — the raw `eprintln!` in the failure summary panicked.

**This step is mandatory.** Copy the actual failure output into the task report verbatim. If the test PASSES here, stop and report it: either the corpus is not producing enough stderr, or the panic is not reaching the exit status. A test that passes before the fix proves nothing, and this repo has repeatedly caught vacuous tests.

- [ ] **Step 3: Do not commit**

Leave the failing test in the working tree. Task 2 commits it alongside the sweep that makes it pass. Report status DONE with the recorded RED evidence.

---

### Task 2: Convert all 73 raw `eprintln!` sites to `crate::render::errln!`

**Files:**
- Modify: `crates/proef-cli/src/exec.rs` (29 sites), `commands.rs` (13), `main.rs` (8), `watch.rs` (6), `diff.rs` (4), `report.rs` (3), `fmt.rs` (3), `secretstore.rs` (2), `explain.rs` (2), `render.rs` (1), `lsp.rs` (1), `front.rs` (1) — 73 total
- Test: `crates/proef-cli/tests/execute.rs` (the Task 1 test, uncommitted in the tree)

**Interfaces:**
- Consumes: `crate::render::errln!` — declared at `crates/proef-cli/src/render.rs:31-37` and exported with `pub(crate) use errln;`. It takes the same arguments as `eprintln!` and expands to `let _ = writeln!(std::io::stderr(), …)`, so it can never panic. Roughly 40 existing call sites already spell it `crate::render::errln!(…)`; match that spelling.
- Produces: a `crates/proef-cli/src` tree containing **zero** `eprintln!` tokens — the precondition Task 3's guard asserts.

- [ ] **Step 1: Replace the macro name across all 12 files**

Only the macro name changes. Message text, format arguments, and argument layout stay byte-identical. Multi-line calls are unaffected because only the head of the call is rewritten.

```bash
cd /Users/emrec/Projects/playground/proef
rg -l 'eprintln!' crates/proef-cli/src/ \
  | xargs perl -pi -e 's/\beprintln!\(/crate::render::errln!(/g'
```

`perl -pi -e` is used rather than `sed -i` because BSD sed (macOS) supports
neither the `\b` word boundary nor GNU's `-i` spelling. Nothing else in the
tree contains the substring `eprintln!(`, so the boundary is belt-and-braces.

Then read the diff before moving on:

```bash
git diff --stat
git diff | head -60
```

Confirm every hunk changes only the macro name — no message text, no format
arguments, no stream. If any hunk touched a doc comment or a string literal,
revert that hunk by hand.

- [ ] **Step 2: Handle the `render.rs` special case deliberately**

`crates/proef-cli/src/render.rs:19` is the `eprintln!` **inside `outln!`'s** non-`BrokenPipe` fallback. Step 1 rewrites it to `crate::render::errln!(…)`, which is correct and needs **no reorder**: path-based macro resolution is order-independent, so `errln!` may stay defined below `outln!` (spec D3).

Read `crates/proef-cli/src/render.rs:13-23` and confirm the body now reads:

```rust
            crate::render::errln!("error: cannot write to stdout: {err}");
```

If — and only if — the compiler rejects this with a macro-resolution error, the fallback is to move the entire `errln!` definition (`render.rs:31-37`, including its `pub(crate) use errln;`) above the `outln!` definition. Do not make this move pre-emptively.

- [ ] **Step 3: Verify zero remaining sites**

Run:

```bash
rg -n 'eprintln!' crates/proef-cli/src/ ; echo "exit=$?"
```

Expected: no output and `exit=1` (ripgrep exits 1 when there are no matches). Any match is a miss — fix it before continuing.

- [ ] **Step 4: Format, then build**

`crate::render::errln!` is longer than `eprintln!`, so rustfmt will reflow some calls. Run the formatter before the check gate:

```bash
cargo fmt --all
cargo build -p proef
```

Expected: builds clean.

- [ ] **Step 5: Flip the Task 1 test RED → GREEN**

Run:

```bash
cargo nextest run -p proef --test execute failure_summary_does_not_panic_on_a_closed_stderr_pipe
```

Expected: **PASS**. The process now exits `1` (test failure) instead of `101`.

- [ ] **Step 6: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

Expected: all green. Pay attention to the existing test `diagnostics_do_not_panic_on_a_closed_stderr_pipe` (`crates/proef-cli/tests/cli.rs:150`) — it must still pass.

- [ ] **Step 7: Commit**

```bash
git add crates/proef-cli/src crates/proef-cli/tests/execute.rs
git commit -m "fix(cli): route every stderr write through the EPIPE-safe guard

eprintln! panics when its write fails, and because Rust ignores SIGPIPE a
closed stderr pipe surfaces as EPIPE rather than a signal — so proef … |&
head aborted with exit 101, outside the typed 0/1/2/3 taxonomy. All 73
remaining raw eprintln! calls in proef-cli now route through render::errln!,
which swallows the failure (stderr has no fallback channel to report to).

The execution failure summary was the largest exposure: it writes several
lines per failing scenario, so a reader closing mid-stream was realistic.
A closed-pipe regression test covers that path against the fixture."
```

---

### Task 3: Drift guard, `report.rs` hygiene, and changelog

**Files:**
- Create: `crates/proef-cli/tests/stderr_hygiene.rs`
- Modify: `crates/proef-cli/src/report.rs:46`
- Modify: `docs/CHANGELOG.md`

**Interfaces:**
- Consumes: `crate::fsutil::parent_dir` — declared at `crates/proef-cli/src/fsutil.rs:56-60` with signature `pub(crate) fn parent_dir(path: &Path) -> PathBuf`. It normalizes both an empty and a `None` parent to `.`.
- Produces: nothing other tasks depend on. This is the final task.

- [ ] **Step 1: Create the drift guard**

Create `crates/proef-cli/tests/stderr_hygiene.rs`:

```rust
//! Drift guard: `proef-cli` writes to stderr only through the EPIPE-safe
//! `render::errln!` macro, never a raw `eprintln!`.
//!
//! `eprintln!` panics when its write fails, and a closed stderr pipe surfaces
//! as EPIPE (Rust ignores SIGPIPE), so a raw call aborts with exit 101 —
//! outside the typed 0/1/2/3 taxonomy (ADR-0009).
//!
//! This lives in `tests/` rather than a `#[cfg(test)] mod` inside `src/` for a
//! correctness reason, not a stylistic one: a source-scanning assertion placed
//! inside its own scan target would match its own needle.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir`, recursively.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("readable source dir {}: {err}", dir.display()))
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn cli_sources_never_use_a_raw_eprintln() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&src, &mut files);
    // A silently empty scan would make this test vacuous.
    assert!(
        !files.is_empty(),
        "no Rust sources found under {}",
        src.display()
    );

    let mut offenders = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("readable source file");
        for (index, line) in text.lines().enumerate() {
            if line.contains("eprintln!") {
                offenders.push(format!("{}:{}", file.display(), index + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "raw eprintln! panics on a closed stderr pipe — use crate::render::errln! instead:\n  {}",
        offenders.join("\n  ")
    );
}
```

- [ ] **Step 2: Run the guard — expect GREEN**

```bash
cargo nextest run -p proef --test stderr_hygiene
```

Expected: PASS (Task 2 removed every raw site).

- [ ] **Step 3: Prove the guard can actually fail**

A guard that cannot fail is worthless. Temporarily reintroduce one raw call — add this line inside the body of `pub fn install()` in `crates/proef-cli/src/render.rs`:

```rust
    eprintln!("temporary guard check");
```

Run:

```bash
cargo nextest run -p proef --test stderr_hygiene
```

Expected: **FAIL**, and the message must name `render.rs` with the correct line number.

Then revert the temporary line and re-run to confirm GREEN again:

```bash
git checkout -- crates/proef-cli/src/render.rs
cargo nextest run -p proef --test stderr_hygiene
```

Expected: PASS. Record both outcomes in the task report.

- [ ] **Step 4: Reuse `fsutil::parent_dir` in `report.rs`**

In `crates/proef-cli/src/report.rs`, change the first line of `artifacts_href` (currently line 46):

```rust
    let out_dir = out_path.parent().unwrap_or(Path::new(""));
```

to:

```rust
    let out_dir = crate::fsutil::parent_dir(out_path);
```

`parent_dir` returns `PathBuf` while `record_dir` is `&Path`. The comparison on the next line should still typecheck via the standard `impl PartialEq<&Path> for PathBuf`. If the compiler disagrees, use `out_dir.as_path() == record_dir`.

**Add no test.** This is confirmed benign, and spec D5 records why: the only use of `out_dir` is `out_dir == record_dir`, and `record::resolve_dir` (`crates/proef-cli/src/record.rs:39-44`) always returns a uuid-named subdirectory, so `record_dir` is never `""` or `"."`. Both the old `""` and the new `"."` compare unequal and take the same branch, producing an identical href. A test here would assert a difference that does not exist.

- [ ] **Step 5: Confirm the build and that `Path` is still used**

```bash
cargo build -p proef
cargo clippy -p proef --all-targets --all-features -- -D warnings
```

Expected: clean. `report.rs` still uses `Path` in its function signatures and at line 31, so the `use std::path::{Path, PathBuf};` import must not become unused. If clippy reports an unused import, fix the import rather than reverting the change.

- [ ] **Step 6: Update the changelog**

In `docs/CHANGELOG.md`, replace the empty `## [Unreleased]` section (line 7, directly above `## [0.5.2] - 2026-08-05 (CLI correctness)`) with:

```markdown
## [Unreleased]

### Fixed

- **The CLI no longer panics when stderr is a closed pipe.** Every remaining
  raw `eprintln!` in `proef-cli` now routes through the EPIPE-safe `errln!`
  guard added in 0.5.2, so `proef test … |& head` ends the pipeline with the
  contracted exit code instead of aborting with 101 — a code outside the typed
  0/1/2/3 taxonomy (ADR-0009). The execution failure summary, which writes
  several lines per failing scenario, was the largest remaining exposure. A
  source-scanning test now keeps raw `eprintln!` out of the crate.

### Changed

- `proef report` derives its output directory through the shared
  `fsutil::parent_dir` helper instead of an open-coded empty-parent fallback,
  so there is one spelling of that derivation. Internal consistency only — the
  emitted artifact links are unchanged.
```

- [ ] **Step 7: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add crates/proef-cli/tests/stderr_hygiene.rs crates/proef-cli/src/report.rs docs/CHANGELOG.md
git commit -m "test(cli): guard against raw eprintln!, reuse parent_dir in report

A source-scanning test asserts crates/proef-cli/src holds no raw eprintln!,
so the EPIPE-safe stderr rule needs no allowlist and cannot drift back. It
lives in tests/ because a scanner inside its own scan target matches its own
needle.

report.rs derives its output directory via fsutil::parent_dir rather than an
open-coded empty-parent fallback. No behavior change: the derived value only
feeds an equality check against a run directory, which is never empty, so
both spellings already produced the same href — this keeps one spelling, and
ships without a test that could only assert a difference that does not exist."
```

---

## Definition of Done

- All 73 raw `eprintln!` sites in `crates/proef-cli/src/` are converted; `rg -n 'eprintln!' crates/proef-cli/src/` returns nothing.
- `failure_summary_does_not_panic_on_a_closed_stderr_pipe` was observed **failing with 101** before the sweep and passes after. The RED output is recorded in the task report.
- `cli_sources_never_use_a_raw_eprintln` was observed **failing** with a deliberately reintroduced call and passes after reverting. Both outcomes are recorded.
- The pre-existing `diagnostics_do_not_panic_on_a_closed_stderr_pipe` (`tests/cli.rs:150`) still passes.
- The full six-command gate is green.
- `docs/CHANGELOG.md` has both entries under `## [Unreleased]`; no version bump anywhere.
