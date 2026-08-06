# Design: proef-cli EPIPE-safety sweep

**Date:** 2026-08-05
**Status:** Approved
**Scope:** `crates/proef-cli` only. `proef-core` untouched.

## Problem

`eprintln!` panics when the underlying write fails. Rust sets `SIGPIPE` to
`SIG_IGN` at startup, so a closed stderr pipe surfaces as an `EPIPE` error
rather than killing the process — and the panic aborts with **exit 101**,
which is not one of the four codes the exit contract allows (`0` ok · `1`
test failure · `2` user error · `3` system error, ADR-0009).

Any `proef … |& head` therefore risks a 101. With `set -o pipefail` this
surfaces to the caller and breaks the contract.

v0.5.2 introduced the guarded `errln!` macro but applied it to only the
dry-run diagnostic path. **73 raw `eprintln!` sites remain** in
`crates/proef-cli/src/`.

## Verified facts (do not re-derive)

Confirmed against the tree at `ac84d13` (v0.5.2).

| Fact | Citation |
| --- | --- |
| `errln!` swallows *all* stderr errors via `let _ = writeln!(…)` — cannot panic | `crates/proef-cli/src/render.rs:31-37` |
| `outln!` reports non-`BrokenPipe` stdout failures **to stderr** via a raw `eprintln!` | `crates/proef-cli/src/render.rs:13-23`, raw call at `:19` |
| `print_all` already routes through `errln!` | `crates/proef-cli/src/render.rs:54-62` |
| Existing closed-pipe test: `head -c0`, `#[cfg(unix)]`, asserts exit `!= 101` | `crates/proef-cli/tests/cli.rs:150` |
| Failure-summary loop — 1-4 stderr lines *per failing scenario* | `crates/proef-cli/src/exec.rs:307-352`; raw calls at `:314`, `:326`, `:333`, `:351` |
| Failing-corpus + fixture test to model on | `crates/proef-cli/tests/execute.rs:237` |
| `CARGO_MANIFEST_DIR` used to reach repo paths from tests | `cli.rs`, `corpus.rs`, `execute.rs` |
| No `clippy.toml` exists at the workspace root | — |
| `eprintln!` outside scope: `xtask/src/main.rs` (32), `crates/proef-lsp/src/server.rs` (1) | — |

### Raw `eprintln!` census — `crates/proef-cli/src/` (73 total)

| File | Sites | File | Sites |
| --- | --- | --- | --- |
| `exec.rs` | 29 | `report.rs` | 3 |
| `commands.rs` | 13 | `fmt.rs` | 3 |
| `main.rs` | 8 | `secretstore.rs` | 2 |
| `watch.rs` | 6 | `explain.rs` | 2 |
| `diff.rs` | 4 | `render.rs` / `lsp.rs` / `front.rs` | 1 each |

## Decisions

### D1 — Sweep all 73 sites, not a triaged subset

A partial sweep leaves an unwritable rule ("convert the user-facing ones"),
which is a judgment call that drifts — contrary to the project's
one-canonical-mechanism property. Completeness is also what makes the guard
(D2) a single-line assertion instead of a maintained allowlist.

`xtask` (dev-only automation) and `proef-lsp` (a different crate; `errln!` is
`pub(crate)` to proef-cli) stay out of scope.

### D2 — Guard: an integration test that scans `src/`

A test under `crates/proef-cli/tests/` walks `crates/proef-cli/src/**/*.rs`
and asserts zero occurrences of `eprintln!`.

Chosen because it adds no new machinery: it runs inside the existing
`cargo nextest` gate, needs no CI or docs wiring, and scopes exactly to
`proef-cli/src`.

It must live in `tests/`, **not** a `#[cfg(test)] mod` inside `src/` — a
source-scanning assertion placed inside its own scan target matches its own
needle and fails vacuously.

Rejected alternatives:

- **`clippy.toml` `disallowed-macros`** — `clippy.toml` is workspace-wide; it
  would fire on `xtask`'s 32 sites and `proef-lsp`'s 1, forcing `#[allow]`
  into crates with no stake in the rule.
- **New `xtask` subcommand** — precise, and it matches the `docs_check()`
  shape, but costs a new gate command in CI, `CLAUDE.md`, and the docs. More
  surface than one rule earns.

### D3 — No macro reorder

Inside `outln!`'s body, call the fully-qualified `crate::render::errln!(…)`.
Path-based macro resolution is order-independent, so `errln!` may stay below
`outln!`, and the spelling matches every existing call site.

If the compiler rejects it, the fallback is to move `errln!` above `outln!`.
The failure mode is a compile error — immediate and unambiguous.

### D4 — No exceptions

After `render.rs:19` converts, `crates/proef-cli/src` contains zero
`eprintln!` tokens. The guard is a plain zero-occurrence assertion.

### D5 — `report.rs` empty parent: hygiene only, no test

`crates/proef-cli/src/report.rs:46` computes
`out_path.parent().unwrap_or(Path::new(""))`. For a bare-filename `-o`, this
yields `Some("")` → the empty path, the same shape as the bug fixed in
v0.5.2.

**It is benign here, and this was verified, not assumed.** The only use of
`out_dir` is the comparison `out_dir == record_dir` (`report.rs:47`).
`record::resolve_dir` returns `runs_root.join(id)` or `latest_run(runs_root)`
(`crates/proef-cli/src/record.rs:39-44`) — always a uuid-named subdirectory,
so `record_dir` can never be `""` or `"."`. Both the current `""` and the
normalized `"."` therefore compare unequal and take the identical `else`
branch, producing the same href. The link is also correct either way: the
report lands in the cwd and the href is cwd-relative.

Change it to `crate::fsutil::parent_dir(out_path)`
(`crates/proef-cli/src/fsutil.rs:56-60`) to keep one canonical
path-base derivation, and ship **no test** — any test would be
non-discriminating. The changelog says plainly that this is consistency, not
a fix.

Implementation note: `parent_dir` returns `PathBuf` while `record_dir` is
`&Path`; if the comparison does not typecheck directly, use
`out_dir.as_path() == record_dir`.

## Testing

One new regression test, plus the guard from D2.

**`exec.rs` failure-summary closed-pipe test** — `#[cfg(unix)]`, in
`crates/proef-cli/tests/execute.rs`:

1. Stand up the fixture and a corpus with **several** failing scenarios
   (model on `execute.rs:237`). Multiple failures matter: the summary must
   emit enough bytes that a write reliably lands *after* the reader closes,
   rather than racing a single short line.
2. Spawn `proef test` with stderr piped; consume nothing via `head -c0` and
   drop the reader (model on `cli.rs:150`).
3. Assert the exit code is **not 101**. The specific non-zero code is not
   pinned — only the absence of a panic.

**The RED must be demonstrated empirically before the sweep lands.** Run the
new test against unmodified `exec.rs` and record the observed 101. This repo
has repeatedly caught vacuous tests; an assertion that passes before the fix
is a plan failure, not a detail.

No test accompanies D5 — see the rationale above.

## Delivery

- Branch off `main`; its own PR.
- Changelog under `## [Unreleased]`: the sweep under **Fixed**, the
  `report.rs` change under **Changed** (internal consistency, no behavior
  change).
- No version bump — both ride the next release.
- Full gate: `cargo fmt --all --check`;
  `cargo clippy --all-targets --all-features -- -D warnings`;
  `cargo nextest run --profile ci`; `cargo test --doc`;
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace`;
  `cargo run -p xtask -- docs-check`.

## Constraints

- `proef-core` untouched; no new dependencies; hurl pins stay `=8.0.1`.
- Package name for `cargo -p` and `assert_cmd::cargo::cargo_bin` is `proef`.
- No behavior change for any currently-working path. Converting `eprintln!`
  to `errln!` preserves the message text and stream; it changes only what
  happens when the write itself fails.
- No plan/task numbers or fix-pass section references in code comments.
- No AI-attribution commit trailers.
