# Design: guard proef-lsp's panic-recovery notice against EPIPE

**Date:** 2026-08-06
**Status:** Approved
**Scope:** One statement in `crates/proef-lsp/src/server.rs`. No other crate.

## Problem

`crates/proef-lsp/src/server.rs:253` reports a recovered analysis panic with a
raw `eprintln!`:

```rust
if let Ok(analysis) =
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| recompute(&inputs)))
{
    return Some(analysis);
}
eprintln!("proef-lsp: suite analysis panicked; keeping previous state");
None
```

`eprintln!` panics when its write fails. Rust sets `SIGPIPE` to `SIG_IGN`, so a
closed stderr surfaces as an `EPIPE` error rather than a signal. The line
therefore panics if the editor has closed the server's stderr.

That is a recovery path failing in the way it is recovering from. The
`catch_unwind` directly above it exists, in its own words, so that "a panic
inside analysis must never take the server down" — and the notice announcing
the rescue can itself kill the server mid-rescue.

The consequence is worse here than in `proef-cli`, where the same mechanism
produced a wrong exit code on a process that was exiting anyway. `proef-lsp`
runs as a long-lived editor child: a panic here crashes a live language server.

## Verified facts (do not re-derive)

Confirmed against `e069e2d`.

| Fact | Citation |
| --- | --- |
| The call sits immediately after `catch_unwind`, in the recovery branch | `crates/proef-lsp/src/server.rs:246-254` |
| It is the **only** stdout/stderr write in the entire crate | `rg 'eprintln!\|println!\|eprint!\|print!\|stderr()\|stdout()' crates/proef-lsp/src/` → one hit |
| `server.rs` has no `std::io` import today | `crates/proef-lsp/src/server.rs:6-8` |
| `proef-lsp` is a lib crate; deps are `proef-core`, `lsp-server`, `lsp-types`, `crossbeam-channel`, `serde`, `serde_json` | `crates/proef-lsp/Cargo.toml` |
| `proef-cli`'s `errln!` is `pub(crate)`, so unreachable from here | `crates/proef-cli/src/render.rs:37` |
| The CLI drives the server via `proef_lsp::run(cfg)` | `crates/proef-cli/src/lsp.rs:93` |

## Decisions

### D1 — Inline the guarded write; no macro

Replace the statement with an explicitly-unchecked `writeln!`:

```rust
let _ = writeln!(
    std::io::stderr(),
    "proef-lsp: suite analysis panicked; keeping previous state"
);
```

Add `use std::io::Write as _;` to the existing `std` import block (it sorts
between `std::collections` and `std::path`).

A private `errln!` copy was rejected: a five-line macro needs a second caller
to earn itself, and this is the only such write in the crate's history.
`proef-core` cannot host a shared macro — it is sans-IO by ADR — and a new
shared crate is out of scope.

The message text, the stream, and the surrounding control flow are unchanged.
Only the behavior on write failure changes: swallow instead of panic. Stderr is
the only diagnostic channel here, so a failed write has nowhere left to report.

### D2 — No drift guard for this crate

`proef-cli` earned its source-scanning guard by accumulating 73 raw sites.
`proef-lsp` has exactly one, and no logging pattern to accumulate more.
Duplicating a ~40-line scanning test into a second crate is machinery guarding
a crate that does not drift. Revisit if a second site ever appears.

### D3 — No test, deliberately

Reaching this line requires a genuine panic inside `recompute()` on a real
analysis **and** a closed stderr. The panic is not injectable without adding a
test-only hook to shipping code — which would cost more than the line it
protects, and would mean changing production code to test a one-line write.

The mechanism itself is already pinned by two closed-pipe regression tests in
`proef-cli` (`tests/cli.rs`, `tests/execute.rs`). This change applies that
proven mechanism at a site that cannot be driven from outside the process.

The changelog states plainly that it ships untested and why, as the `report.rs`
change did.

## Delivery

- Branch off `main`; own PR.
- Changelog under `## [Unreleased]` → `### Fixed`, joining the two entries
  already there.
- No version bump — rides the next release.
- Full gate: `cargo fmt --all --check`;
  `cargo clippy --all-targets --all-features -- -D warnings`;
  `cargo nextest run --profile ci`; `cargo test --doc`;
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace`;
  `cargo run -p xtask -- docs-check`.

## Constraints

- `proef-core` untouched and still sans-IO; no new dependencies; hurl pins stay
  `=8.0.1`.
- No behavior change to any path that currently works. Only the write-failure
  behavior changes.
- No task ids, plan numbers, or fix-pass section references in code comments.
- No AI-attribution commit trailers.
