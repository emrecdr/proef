# Common Code Smells — Rust

> Template baseline — the project's own `CLAUDE.md` and documented conventions WIN on any conflict with this file. Tailor it to the project (`/devt:setup`); untailored copies are flagged by `/devt:setup --health`.

Anti-patterns to detect and fix during code review and development.

## `unwrap()` / `expect()` in Production Paths

**Smell**: `.unwrap()` or `.expect("...")` outside `tests/`, `main`, or proven-invariant scopes.

**Why it's bad**: A panic in production crashes the process. Library code that panics on input variation cannot be safely reused.

**How to detect**: `grep -rn "\.unwrap()\|\.expect(" --include="*.rs" src/`

**Fix**: Propagate via `?` and define a proper error type. If the invariant is genuinely proven, use `expect("invariant: <reason>")` with a message stating why the value is guaranteed.

## `Box<dyn Error>` in Public Library API

**Smell**: `Result<T, Box<dyn std::error::Error>>` as the return type of a public function in a library crate.

**Why it's bad**: Callers cannot match on specific variants. The error type erases all information — downstream code has no way to handle different failures differently.

**Fix**: Define a `thiserror`-derived enum with concrete variants. Use `Box<dyn Error>` only in application `main` or binary crates.

## `unsafe` Without `// SAFETY:` Comment

**Smell**: `unsafe { ... }` block with no preceding comment explaining the invariant.

**Why it's bad**: `unsafe` invariants are local — without a per-block comment, a reviewer cannot verify soundness. Stale invariants creep in silently.

**How to detect**: `grep -rn "unsafe {" --include="*.rs" src/ | xargs -I{} sh -c 'echo "{}"; grep -B1 "unsafe {" "{}" | head -2'`

**Fix**: Add `// SAFETY: <reason the operation is safe>` immediately above every `unsafe` block. Reviewers should reject any `unsafe` block without one.

## Excessive `.clone()`

**Smell**: Multiple `.clone()` calls in a hot path, or `.clone()` on a parameter just to satisfy the borrow checker.

**Why it's bad**: Allocation overhead, hides borrowing design problems, ships fixable performance issues.

**How to detect**: `grep -rn "\.clone()" --include="*.rs" src/ | wc -l`  (sanity-check the per-file rate)

**Fix**: Borrow with `&T` / `&mut T`. Use `Cow<'a, T>` when ownership is conditional. Use `Arc<T>` when ownership is genuinely shared across tasks. Re-design the function signature when a single `.clone()` resolves multiple borrow conflicts.

## `Arc<Mutex<T>>` Everywhere

**Smell**: `Arc<Mutex<T>>` as the default container for any shared state.

**Why it's bad**: Lock contention under load. Often the right answer is a channel, an actor, or `Arc<RwLock<T>>` for read-heavy workloads — `Mutex<T>` is the most contention-prone choice.

**Fix**: Reach for `Arc<RwLock<T>>` for read-heavy state, `Arc<T>` with interior `AtomicU64`/`AtomicBool` for simple counters/flags, `tokio::sync::mpsc::channel` for producer-consumer flows. `Arc<Mutex<T>>` should be a deliberate choice, not the default.

## `tokio::sync::Mutex` Without an `await` Held

**Smell**: `tokio::sync::Mutex` used in code that doesn't `.await` while holding the lock.

**Why it's bad**: `tokio::sync::Mutex` is slower than `std::sync::Mutex` because it's designed to suspend tasks on contention. If the critical section doesn't `.await`, you pay the async overhead for nothing.

**Fix**: Use `std::sync::Mutex` (or `parking_lot::Mutex`). Reserve `tokio::sync::Mutex` for sections that actually hold the lock across `.await`.

## `.await` Inside `std::sync::Mutex` Guard

**Smell**: `let guard = mutex.lock().unwrap(); ... .await ...` — holding a `std::sync::Mutex` across an `.await`.

**Why it's bad**: Deadlock risk + future is not `Send`. The lock can be held across task suspension, blocking other tasks that need the same lock.

**Fix**: Either drop the guard before `.await` (extract the data first), or switch to `tokio::sync::Mutex`. clippy's `await_holding_lock` lint catches this.

## `for i in 0..vec.len()` Indexed Loop

**Smell**: C-style indexed iteration when `iter()` would do.

**Why it's bad**: Bounds checks on every access, loses iterator-chain composability, hides intent.

**How to detect**: `grep -rn "for .* in 0\\.\\." --include="*.rs" src/`

**Fix**: Use `for x in vec.iter()` / `for x in vec.iter_mut()` / `vec.iter().enumerate()` when the index is genuinely needed.

## `unwrap_or_default()` Hiding Logical Errors

**Smell**: `.unwrap_or_default()` on a `Result` or `Option` where the default silently masks a real error.

**Why it's bad**: A failure that should propagate becomes an empty `Vec`, a zero, or an empty `String` — symptoms surface far from the cause.

**Fix**: Propagate via `?` when the absence is an error. Use `unwrap_or_default()` only when the default IS the correct response (e.g., empty input maps to empty output).

## Missing `Send + Sync` Bounds on Trait Objects

**Smell**: `Box<dyn MyTrait>` in async code without `+ Send` (or `+ Send + Sync` when shared across tasks).

**Why it's bad**: Compiler error when you try to send the value across a task boundary — usually surfaces far from the trait definition, frustrating to debug.

**Fix**: Define traits intended for async use with `Send + Sync` bounds: `pub trait MyTrait: Send + Sync { ... }`. Saves downstream consumers from re-discovering the bound at every use site.

## `Result<(), E>` Returns Losing Information

**Smell**: A function returning `Result<(), E>` where the success case carries useful information.

**Why it's bad**: Caller must call a separate function (or read mutable state) to retrieve the result. Tightly couples to the call sequence.

**Fix**: Return `Result<T, E>` with the actual computed value. Reserve `Result<(), E>` for genuine void operations (e.g., `save`, `commit`).

## `as` Cast with Truncation Risk

**Smell**: `value as i32`, `value as u8`, etc., where the source type is wider.

**Why it's bad**: Silent truncation. `300_u32 as u8` = 44, no warning, no error.

**How to detect**: `grep -rn " as [iu][0-9]" --include="*.rs" src/`

**Fix**: Use `TryFrom` / `try_into()` for fallible conversions. Use `as` only when the cast is provably safe and document why with a comment.

## `unwrap()` Chains in Tests Without Context

**Smell**: `repo.find(id).await.unwrap().email.unwrap().to_string()` in test code.

**Why it's bad**: When the test fails, the panic message is "called `Result::unwrap()` on an `Err` value: ..." — nothing about which step failed.

**Fix**: Use `.expect("description")` at each step, or destructure with `let Some(x) = y else { panic!("..."); }`. Better: a single `assert_matches!` or pattern-matching helper.

## `println!` for Logging in Library Code

**Smell**: `println!()` or `eprintln!()` for diagnostic output in library code.

**Why it's bad**: Library consumers cannot configure log levels or destinations. Output pollutes stdout/stderr unconditionally.

**How to detect**: `grep -rn "println!\\|eprintln!" --include="*.rs" src/ | grep -v "examples/\\|tests/\\|bin/"`

**Fix**: Use `tracing` macros (`tracing::info!`, `tracing::debug!`, etc.). Library code emits events; the application configures a subscriber.

## `String` Where `&str` Would Do

**Smell**: Function signature taking `name: String` when the function only reads the value.

**Why it's bad**: Forces callers to allocate or `.clone()` even when they already have a `&str`. API friction.

**Fix**: Take `name: &str`. Take `String` only when the function logically takes ownership (storing it, returning it, sending it to a channel).

## `Vec<T>` Parameter Where `&[T]` Would Do

**Smell**: Function taking `items: Vec<T>` when it only iterates.

**Why it's bad**: Forces callers to relinquish ownership or `.clone()`. Couples the API to a specific collection type.

**Fix**: Take `items: &[T]`. Accept `impl IntoIterator<Item = T>` when the function genuinely consumes a sequence.

## Hand-Rolled `From` Impl Where `#[from]` Would Do

**Smell**: Manual `impl From<X> for MyError { ... }` instead of `#[from]` in a `thiserror`-derived enum.

**Why it's bad**: More code, easier to drift out of sync with the variant definition, harder to refactor.

**Fix**: Use `#[from]` attribute on the variant:

```rust
#[derive(Debug, thiserror::Error)]
pub enum MyError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
```

## God Module (Single `.rs` over 1000 lines)

**Smell**: A single `.rs` file containing 1000+ lines of code (excluding tests).

**Why it's bad**: Hard to navigate, hard to test independently, signals missing structure.

**How to detect**: `wc -l src/**/*.rs | sort -n | tail`

**Fix**: Split into focused submodules. Each module should do one thing.

## `derive(Clone, Copy, ...)` on Types Holding Sensitive State

**Smell**: `#[derive(Clone, Copy)]` on a type holding cryptographic keys, passwords, session tokens.

**Why it's bad**: Makes accidental duplication trivial — copies of secrets propagate through the call graph without explicit awareness. Defeats `Drop`-based zeroing.

**Fix**: Wrap secret types in `secrecy::Secret<T>` (or equivalent). Don't `derive(Copy)`; require explicit `.clone()` so propagation is visible at the call site.

## Hardcoded Timeouts

**Smell**: `tokio::time::sleep(Duration::from_secs(5))` or `.timeout(Duration::from_secs(30))` with magic numbers.

**Why it's bad**: Not configurable, not testable, not documented.

**Fix**: Define as a `const TIMEOUT: Duration = Duration::from_secs(30);` or accept as a configuration parameter.

## Empty `impl` Block

**Smell**: `impl SomeTrait for SomeType {}` with no methods, used only to mark the type.

**Why it's bad**: Doesn't express intent — a reviewer reads it as "did the author forget to fill this in?".

**Fix**: If the trait is genuinely a marker, document why with a `// Marker impl: <reason>` comment. Better: design the API so a marker isn't needed.

## `#[allow(...)]` Without Justification

**Smell**: `#[allow(dead_code)]`, `#[allow(clippy::unwrap_used)]`, etc., with no comment explaining why.

**Why it's bad**: Lint allowances accrete silently. Future readers can't tell if the allowance is still needed.

**Fix**: Every `#[allow(...)]` gets a `// Why: <reason>` or `// SAFETY: <invariant>` comment.

## Test Logic in Production Code

**Smell**: `if cfg!(test) { ... }` or `#[cfg(test)] static MOCK: ...` in `src/`.

**Why it's bad**: Test concerns leak into production. Creates untestable branches.

**Fix**: Use trait-based dependency injection. Tests inject mock implementations via the trait; production injects the real one. Keeps `src/` free of test-specific branches.
