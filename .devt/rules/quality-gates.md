# Quality Gates

> Template baseline — the project's own `CLAUDE.md` and documented conventions WIN on any conflict with this file. Tailor it to the project (`/devt:setup`); untailored copies are flagged by `/devt:setup --health`.

Devt-integration document for Rust projects. Read by the `tester`, `code-reviewer`, and `verifier` agents. Each gate has a deterministic command; agents run the exact command and verify exit code 0 + clean output.

## Gate Stack

Run in this order. Each gate is independent; one failure does not block the others from running. Fix in order — earlier gates (build / type) often expose the underlying problem causing later failures (test / lint).

```
1. cargo check          ← does it compile?
2. cargo clippy         ← does it pass lints?
3. cargo fmt            ← is the formatting clean?
4. cargo test           ← do tests pass?
5. cargo test --doc     ← do doc examples pass?
6. cargo doc            ← does documentation build?
7. cargo audit          ← are dependencies free of advisories? (optional but recommended)
8. cargo deny           ← are dependency licenses + bans clean? (optional, enable per-project)
```

## Gate Commands

### 1. Build (`cargo check`)

```bash
cargo check --all-targets --all-features
```

- `--all-targets`: includes tests, benches, examples — not just the library/binary
- `--all-features`: ensures feature-gated code compiles
- Exit code 0 + zero warnings = pass
- For workspaces, add `--workspace` to compile every member crate

For release-mode validation (catches more optimizer-related issues):

```bash
cargo check --all-targets --all-features --release
```

### 2. Lint (`cargo clippy`)

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

- `-D warnings`: every clippy warning becomes an error
- Project-level lint configuration lives in `Cargo.toml` `[lints]` table or `clippy.toml`
- For workspaces: `cargo clippy --workspace --all-targets --all-features -- -D warnings`

Common per-project lint allows (declare in `Cargo.toml`):

```toml
[lints.clippy]
pedantic = "warn"
nursery = "warn"
cargo = "warn"
module_name_repetitions = "allow"   # legitimate noise — submodule name often repeats parent
missing_errors_doc = "allow"         # bulky on small Error enums
```

### 3. Format (`cargo fmt`)

```bash
cargo fmt --all -- --check
```

- `--check`: exit 0 if formatted, non-zero (with diff) if not — does NOT modify files
- Pre-commit hook recommended: `cargo fmt --all` (drop `--check` to actually format)
- Configuration lives in `rustfmt.toml` at repo root; defaults are usually fine

### 4. Test (`cargo test`)

```bash
cargo test --all-targets --all-features
```

- Runs unit tests (`#[cfg(test)] mod tests`), integration tests (`tests/`), and doc tests
- For workspaces: `cargo test --workspace --all-targets --all-features`
- Set `RUST_BACKTRACE=1` for full panic backtraces when a test fails

For deterministic test ordering (helpful when tests have hidden order dependencies):

```bash
cargo test -- --test-threads=1
```

### 5. Doc Tests (`cargo test --doc`)

```bash
cargo test --doc --all-features
```

- Executes code in `///` doc comments — examples in public API docs are real tests
- For workspaces: `cargo test --workspace --doc --all-features`
- Doc tests catch when API examples drift from the actual signatures

### 6. Documentation Build (`cargo doc`)

```bash
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
```

- `RUSTDOCFLAGS="-D warnings"`: broken intra-doc links + missing items become errors
- `--no-deps`: do not rebuild dependency docs
- Surfaces broken `[\`OtherType\`]` references and missing-doc warnings (when `#![warn(missing_docs)]` is set)

### 7. Audit (`cargo audit`) — Optional

```bash
cargo audit
```

- Checks `Cargo.lock` against the RustSec advisory database
- Requires `cargo-audit` installed (`cargo install cargo-audit`)
- Add `--deny warnings` to escalate yanked-crate warnings to errors

### 8. Deny (`cargo deny`) — Optional

```bash
cargo deny check
```

- Comprehensive policy: licenses, bans, advisories, sources
- Requires `cargo-deny` + `deny.toml` config at repo root
- Useful for organizations with license-allowlist requirements

## Pre-Commit Pipeline

A complete pre-commit script combining the essential gates:

```bash
#!/usr/bin/env bash
set -euo pipefail

cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo check --all-targets --all-features
cargo test --all-targets --all-features
```

## CI Pipeline

Production CI should run all 8 gates. Recommended GitHub Actions configuration outline (project chooses concrete runner):

```yaml
- run: cargo fmt --all -- --check
- run: cargo clippy --workspace --all-targets --all-features -- -D warnings
- run: cargo check --workspace --all-targets --all-features
- run: cargo test --workspace --all-targets --all-features
- run: cargo test --workspace --doc --all-features
- run: RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
- run: cargo audit
```

## Coverage (Recommended, Not Required)

```bash
cargo install cargo-tarpaulin
cargo tarpaulin --workspace --out Stdout
```

- Tarpaulin is the most-used coverage tool; `cargo llvm-cov` is a faster alternative based on LLVM source-based coverage
- Set a minimum coverage target in `Cargo.toml` or CI script (commonly 70–80% line coverage)

## Benchmarks (Optional)

```bash
cargo bench --workspace
```

- Uses `criterion` crate when `benches/` directory contains `criterion`-style benchmarks
- Not a gate (would be too slow for CI on every push) — run on release-prep and perf-investigation cycles

## Recipes

### Skip a specific clippy lint for one line

```rust
#[allow(clippy::unwrap_used)]
let x = some_safe_invariant.unwrap();
```

Always include a `// SAFETY:` or `// Why:` comment explaining the allow.

### Force colored test output

```bash
cargo test --color=always 2>&1 | less -R
```

### Build with all warnings as errors locally

```bash
RUSTFLAGS="-D warnings" cargo check --all-targets --all-features
```

### Faster incremental builds during development

Set in `~/.cargo/config.toml`:

```toml
[build]
rustflags = ["-C", "link-arg=-fuse-ld=lld"]
```

(requires `lld` installed)

## What Counts as Pass

| Gate | Pass criteria |
|---|---|
| `cargo check` | exit 0, zero warnings on stderr |
| `cargo clippy -- -D warnings` | exit 0 |
| `cargo fmt -- --check` | exit 0 (no diff produced) |
| `cargo test` | exit 0, `test result: ok.` line, no failures |
| `cargo test --doc` | exit 0, `test result: ok.` line for doc-tests |
| `cargo doc` (with `-D warnings`) | exit 0 |
| `cargo audit` | exit 0, no advisories matching `Cargo.lock` |
| `cargo deny check` | exit 0, no policy violations |

A gate that emits warnings without erroring is NOT a pass — the agent should report `WARNINGS` status and require explicit acknowledgement before declaring done.
