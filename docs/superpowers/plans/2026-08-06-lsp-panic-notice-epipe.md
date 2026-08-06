# proef-lsp Panic-Notice EPIPE Guard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `proef-lsp`'s panic-recovery notice from panicking on a closed stderr and killing the server it just rescued.

**Architecture:** Replace one raw `eprintln!` with an explicitly-unchecked `writeln!` to `std::io::stderr()`. No macro, no new abstraction, no new dependency.

**Tech Stack:** Rust 2024, `cargo-nextest`.

**Approved spec:** `docs/superpowers/specs/2026-08-06-lsp-panic-notice-epipe-design.md` — it carries the verified `file:line` facts. Cite them; do not re-derive.

**Branch:** `fix/lsp-panic-notice-epipe`, off `main` (`e069e2d`).

## Global Constraints

- `proef-core` is untouched and stays sans-IO. `proef-lsp` only.
- No new dependencies. hurl pins stay exactly `hurl = "=8.0.1"`, `hurl_core = "=8.0.1"`.
- No behavior change to any path that currently works. The message text, the stream, and the surrounding control flow are unchanged; only the behavior when the write itself fails changes (swallow instead of panic).
- No task ids, plan numbers, or fix-pass section references in code comments.
- No AI-attribution commit trailers.
- No version bump. The entry rides the next release under `## [Unreleased]`.
- **No test.** Spec D3 explains why: the panic is not injectable without adding a test-only hook to shipping code. Adding a test here is a failure, not thoroughness.

---

### Task 1: Guard the panic-recovery notice

**Files:**
- Modify: `crates/proef-lsp/src/server.rs` (import block at `:6-8`, and the statement at `:253`)
- Modify: `docs/CHANGELOG.md`

**Interfaces:**
- Consumes: nothing from other tasks. `std::io::{stderr, Write}` only.
- Produces: nothing other tasks depend on. Single-task plan.

- [ ] **Step 1: Add the `Write` trait import**

In `crates/proef-lsp/src/server.rs`, the `std` import block currently reads:

```rust
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};
```

Insert the `io` import in sorted position:

```rust
use std::collections::{BTreeMap, HashSet};
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};
```

The anonymous `as _` form brings the trait into scope for `writeln!` without introducing a nameable `Write` symbol.

- [ ] **Step 2: Replace the raw `eprintln!`**

In `current_analysis`, this line (spec cites `server.rs:253`):

```rust
    eprintln!("proef-lsp: suite analysis panicked; keeping previous state");
```

becomes:

```rust
    // The recovery notice must not become the failure it reports: `eprintln!`
    // panics when its write fails, and a closed stderr surfaces as EPIPE
    // (Rust ignores SIGPIPE) — which would take down the very server this
    // `catch_unwind` exists to keep alive. Swallow the write error; stderr is
    // the only channel here, so there is nowhere left to report it.
    let _ = writeln!(
        std::io::stderr(),
        "proef-lsp: suite analysis panicked; keeping previous state"
    );
```

Leave the surrounding `catch_unwind`, the `return Some(analysis)`, and the trailing `None` exactly as they are.

- [ ] **Step 3: Confirm the crate now has no raw print macros**

Run:

```bash
rg -n 'eprintln!|eprint!|println!|print!' crates/proef-lsp/src/ ; echo "exit=$?"
```

Expected: no output and `exit=1` (ripgrep exits 1 when there are no matches).

- [ ] **Step 4: Build and check**

```bash
cargo build -p proef-lsp
cargo clippy -p proef-lsp --all-targets --all-features -- -D warnings
```

Expected: clean. If clippy reports the import as unused, the `writeln!` did not land — re-check Step 2 rather than deleting the import.

- [ ] **Step 5: Update the changelog**

In `docs/CHANGELOG.md`, add this bullet to the **existing** `### Fixed` list under `## [Unreleased]`, after the CLI entry already there:

```markdown
- **The language server no longer dies while recovering from a panic.**
  `proef-lsp` reports a caught analysis panic on stderr; that report used a raw
  `eprintln!`, which panics when its write fails — so a closed stderr (EPIPE)
  took down the very server the surrounding `catch_unwind` exists to keep
  alive. The write is now explicitly unchecked. Ships without a test: reaching
  the line needs a real analysis panic *and* a closed stderr, and the panic is
  not injectable without a test-only hook in shipping code; the mechanism
  itself is already covered by the CLI's closed-pipe tests.
```

- [ ] **Step 6: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

Expected: all green, with the same test count as `main` (no tests added or removed).

- [ ] **Step 7: Commit**

```bash
git add crates/proef-lsp/src/server.rs docs/CHANGELOG.md
git commit -m "fix(lsp): don't let the panic notice kill the recovered server

proef-lsp reports a caught analysis panic on stderr. That report used a raw
eprintln!, which panics when its write fails — and because Rust ignores
SIGPIPE, a closed stderr surfaces as EPIPE rather than a signal. The notice
could therefore take down the very server the surrounding catch_unwind
exists to keep alive: a recovery path failing in the way it recovers from.

The write is now explicitly unchecked. Message text, stream and control flow
are unchanged; only the write-failure behavior differs. Stderr is the only
diagnostic channel here, so a failed write has nowhere left to report."
```

---

## Definition of Done

- `rg -n 'eprintln!|eprint!|println!|print!' crates/proef-lsp/src/` returns nothing.
- The message text is byte-identical to what shipped before.
- No test was added (spec D3), and the suite count matches `main`.
- The full six-command gate is green.
- `docs/CHANGELOG.md` carries the new bullet under the existing `## [Unreleased]` → `### Fixed`; no version bump anywhere.
