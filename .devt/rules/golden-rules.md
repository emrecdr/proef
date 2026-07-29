# Golden Rules

> Template baseline — the project's own `CLAUDE.md` and documented conventions WIN on any conflict with this file. Tailor it to the project (`/devt:setup`); untailored copies are flagged by `/devt:setup --health`.

Devt-integration document for Rust projects. Read by every agent. These rules are non-negotiable — they encode invariants that produce buggy / unsound / unreviewable code when violated.

## 1. No `unwrap()` / `expect()` Outside Tests + `main`

```rust
// WRONG — panic in production path
let user = repo.find(&id).await.unwrap();

// CORRECT — propagate via `?`
let user = repo.find(&id).await?;
```

Allowed contexts:

- Test functions (`#[test]`, `#[tokio::test]`, `#[cfg(test)] mod tests`)
- `main` (top-level error becomes process exit)
- After a check that proves invariance:
  ```rust
  if let Some(x) = maybe {
      // `expect()` here is fine — we just checked `is_some()`
  }
  ```

When unavoidable, use `expect("invariant: <reason>")` not `unwrap()`. The message must state WHY the value is guaranteed present.

## 2. Use `?` for Error Propagation — Not Manual `match`

```rust
// WRONG — manual match adds noise without adding behavior
let user = match repo.find(&id).await {
    Ok(u) => u,
    Err(e) => return Err(e),
};

// CORRECT
let user = repo.find(&id).await?;
```

Reach for `match` only when each variant requires distinct handling (different error wrapping, recovery branch, logging at this site).

## 3. Library Errors Are `thiserror`, Application Errors Are `anyhow`

```rust
// Library crate — concrete enum, callers can match
#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("not found")]
    NotFound,
    #[error("connection failed: {0}")]
    ConnectionFailed(#[from] sqlx::Error),
}

// Application crate — context-rich, opaque
async fn handle() -> anyhow::Result<()> {
    let user = repo.find(&id)
        .await
        .context("failed to load user during checkout")?;
    Ok(())
}
```

Never use `Box<dyn Error>` in public library API. Never use `thiserror` enums for high-level application orchestration where ad-hoc context messages matter more than typed variants.

## 4. Document `# Safety` and `# Panics` Sections

```rust
/// Reads from the buffer without bounds checking.
///
/// # Safety
/// Caller must ensure `idx < self.len()`. Violating this is undefined behavior.
pub unsafe fn read_unchecked(&self, idx: usize) -> u8 {
    *self.ptr.add(idx)
}

/// Decode and return the value.
///
/// # Panics
/// Panics if the buffer length is less than 4 bytes.
pub fn decode(buf: &[u8]) -> u32 {
    assert!(buf.len() >= 4);
    u32::from_le_bytes(buf[..4].try_into().unwrap())
}
```

Every `unsafe fn` MUST have a `# Safety` rustdoc section. Every function that can panic on input variation MUST have a `# Panics` section.

## 5. Every `unsafe` Block Has a `SAFETY:` Comment

```rust
// SAFETY: idx is bounds-checked in the public caller; this is a hot path
// where the redundant check is measurable.
unsafe { *self.ptr.add(idx) }
```

The comment must state the invariant that makes the operation safe. "Looks fine" / "should be OK" / no comment = audit-fail.

## 6. No `unsafe` Without Justification

Prefer `#![forbid(unsafe_code)]` at the crate root for libraries that don't need `unsafe`. When `unsafe` IS needed, the justification is captured in:

1. The `// SAFETY:` comment at the call site
2. The `# Safety` rustdoc section on any `unsafe fn`
3. A test exercising the boundary, where feasible

Crates that lift `forbid(unsafe_code)` require code-review approval — soundness is not a single-author decision.

## 7. Minimize `mut` and `.clone()`

Each `mut` or `.clone()` is a signal — usually fine, sometimes the right answer, sometimes a sign of a borrowing redesign. Reviewers ask: "Why this instead of borrowing / rebinding / `Cow<'_, T>` / `Arc<T>`?" An answer like "the borrow checker complained" is insufficient — the question is whether the design avoided the borrow correctly.

## 8. No `pub` Without Justification

Default to private. Promote to `pub(crate)` when crossed by sibling modules. Promote to `pub` only when external consumers genuinely need access. Each `pub` is API surface that must be maintained.

## 9. Tests Live Beside Source

```rust
// src/user.rs
pub fn validate(email: &str) -> bool { ... }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_string() {
        assert!(!validate(""));
    }
}
```

Unit tests in `#[cfg(test)] mod tests` blocks at the bottom of the source file. Integration tests in `tests/`. Never split unit tests into a separate file — proximity to the code under test is part of the test's documentation value.

## 10. Async Functions Are Cancellation-Safe by Default

When an async function holds resources (locks, file handles, network connections) across `.await` points, it must remain correct under cancellation — the caller dropping the future at any await point should not corrupt state.

When a function is NOT cancellation-safe, document it at the top:

```rust
/// # Cancellation
/// This function is NOT cancellation-safe. Dropping the future mid-operation
/// will leave the database connection in an inconsistent state. Wrap calls
/// in `tokio::task::spawn` if cancellation handling is required.
pub async fn risky_update(&self, ...) -> Result<...> { ... }
```

## 11. No `println!` / `eprintln!` for Logging

```rust
// WRONG
println!("loaded {} users", users.len());

// CORRECT
tracing::info!(count = users.len(), "loaded users");
```

`println!`/`eprintln!` are for binary stdout/stderr that is part of the program's contract (CLI output). Diagnostic logging goes through `tracing` (preferred) or `log`.

## 12. No `static mut` / Global Mutable State

```rust
// WRONG — undefined behavior under concurrent access
static mut COUNTER: u32 = 0;

// CORRECT — use atomics
use std::sync::atomic::{AtomicU32, Ordering};
static COUNTER: AtomicU32 = AtomicU32::new(0);
```

For sharable initialized-once state, use `OnceCell` / `OnceLock` (standard) or `once_cell::sync::Lazy` / `LazyLock`.

## 13. Format with `cargo fmt` — Always

No exceptions. Override only via `rustfmt.toml` configuration at repo root, never via inline `#[rustfmt::skip]` without a comment explaining the reason.

## 14. Clippy Warnings Are Errors

`cargo clippy -- -D warnings` is the gate. Allowing a lint requires a justification comment:

```rust
#[allow(clippy::too_many_arguments)] // 8-arg constructor is the public API stability contract — refactor would be breaking
pub fn new(...) -> Self { ... }
```

## 15. Newtype Wrappers for Domain Identifiers

```rust
// WRONG — bare primitives lose type safety
fn find_user(id: u64) { ... }
fn find_order(id: u64) { ... }  // can pass user_id by accident

// CORRECT — newtype provides compile-time discrimination
pub struct UserId(u64);
pub struct OrderId(u64);

fn find_user(id: UserId) { ... }
fn find_order(id: OrderId) { ... }
```

## 16. Cargo.lock Is Committed for Binaries

For applications and CLI tools, commit `Cargo.lock`. For libraries published to crates.io, exclude it via `.gitignore`. Mixed workspaces (binary + libraries) commit it.

## 17. No Drift Between `Cargo.toml` and `Cargo.lock`

```bash
cargo check --locked     # fails if Cargo.lock would need changes
```

CI must run with `--locked` or `--frozen` to catch dependency drift before merge.

## 18. No `extern crate` in Edition 2018+

Edition 2018+ replaced `extern crate` with automatic dependency resolution from `Cargo.toml`. Only exception: linking against `alloc` / `core` / `proc_macro` in `no_std` contexts.

## 19. Trait Implementations Stay Close to the Type

When you own the type, put `impl Trait for Type` near the type definition. When you own the trait but not the type (foreign type), document why the impl is here and not elsewhere.

## 20. Public API Changes Require Semver Bump

Breaking changes to public types, functions, or trait signatures require a major version bump (or pre-1.0 minor bump). Run `cargo semver-checks` on release-prep cycles to catch unintentional breakages.

---

## 21. Never Weaken Tests to Pass

Never remove, skip, or weaken a failing test, gate, or assertion to make a run
pass — fix the code, not the test. A deleted test cannot fail; pass/fail
diffing is blind to it, and the gap ships as missing or buggy functionality.
If a test is genuinely wrong, change it visibly and state why in the output
artifact — the verifier diffs test counts against the baseline and flags
silent drops.

---
