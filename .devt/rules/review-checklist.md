# Review Checklist — Rust

> Template baseline — the project's own `CLAUDE.md` and documented conventions WIN on any conflict with this file. Tailor it to the project (`/devt:setup`); untailored copies are flagged by `/devt:setup --health`.

Language-specific review priorities. The code-reviewer reads this alongside `coding-standards.md` and `golden-rules.md`.

---

## CRITICAL — Soundness + Safety

- [ ] **`unsafe` without `SAFETY:` comment** — every `unsafe` block must explain the invariant that makes it safe
- [ ] **`unsafe fn` missing `# Safety` rustdoc section** — public unsafe API must document caller preconditions
- [ ] **`static mut`** — undefined behavior under concurrent access; use atomics or `OnceLock`
- [ ] **Data races**: shared `&mut` across threads without `Mutex` / `RwLock` / atomic ordering
- [ ] **Send/Sync bounds wrong**: `Send`/`Sync` impl on types that hold non-thread-safe inner state
- [ ] **Transmute / pointer casts** between incompatible types
- [ ] **Drop ordering assumptions**: relying on specific drop order across fields or scopes

## CRITICAL — Error Handling

- [ ] **`unwrap()` / `expect()` in production paths** — outside `tests/`, `main`, or proven invariants
- [ ] **`Box<dyn Error>` in library public API** — callers can't match on variants
- [ ] **Mixing `anyhow` + `thiserror` in the same crate** without justification
- [ ] **Swallowed errors**: `let _ = result;` without comment explaining why
- [ ] **Panic in library public API** without `# Panics` rustdoc section
- [ ] **`?` on a `Result<T, E1>` returned from a `-> Result<T, E2>` fn without `From<E1> for E2`** (compile error, but watch for placeholder `From` impls that lose context)

## CRITICAL — Security

- [ ] **SQL injection**: string concatenation in queries — use parameterized queries (`sqlx::query!`, `diesel::sql_query` with bound params)
- [ ] **Command injection**: unvalidated input in `std::process::Command::arg` — never shell-pipe untrusted input
- [ ] **Path traversal**: user-controlled paths without `Path::canonicalize` + prefix check
- [ ] **Hardcoded secrets**: API keys, passwords, tokens in source — use `std::env::var` or a config crate
- [ ] **Weak crypto**: MD5/SHA1 for security purposes — use SHA-256+ (`sha2`) or `argon2` for passwords
- [ ] **Unsafe deserialization**: `serde_json::from_str` on untrusted input WITHOUT a size limit upstream

## HIGH — Type Safety

- [ ] **Bare primitives where newtypes belong**: `u64` for IDs, `String` for codes — see `canonical-entities.yaml`
- [ ] **`Option<T>` flattened to defaults silently**: `unwrap_or_default()` hiding logical errors
- [ ] **`as` casts that may truncate** (`i64 as i32`) without bounds check or `try_from`
- [ ] **Public function returns `()` losing information** that could be a `Result` or output type
- [ ] **Generic over too many params** when associated types would express the constraint

## HIGH — Borrowing + Lifetimes

- [ ] **Excessive `.clone()`** — could borrow / use `Cow` / use `Arc`
- [ ] **`String` where `&str` would do** in function parameters
- [ ] **`Vec<T>` parameter where `&[T]` would do**
- [ ] **Explicit lifetimes that compiler would elide** — noise without value
- [ ] **`'static` bound where a generic lifetime would work** — over-restrictive API

## HIGH — Idiomatic Rust

- [ ] **Manual `match` where `?` would propagate** (and the only logic IS propagation)
- [ ] **C-style `for i in 0..vec.len()` indexed loop** when `vec.iter()` works
- [ ] **`Vec::new()` + `.push()` in a loop** when `iter().collect()` works
- [ ] **`map(|x| x.foo()).collect::<Vec<_>>()` followed by `for` loop** — chain inline
- [ ] **Builder pattern missing `#[must_use]`**
- [ ] **`pub` on items only used by tests** — should be `pub(crate)` or test-only re-export

## HIGH — Async + Concurrency

- [ ] **`.await` while holding a `std::sync::Mutex`** — deadlock risk; use `tokio::sync::Mutex`
- [ ] **`tokio::sync::Mutex` held across NO awaits** — unnecessary overhead; use `std::sync::Mutex`
- [ ] **Async fn doing CPU-bound work** without `tokio::task::spawn_blocking`
- [ ] **Unbounded channel (`mpsc::unbounded_channel`)** without backpressure justification
- [ ] **Cancellation safety unstated** for async fn holding mutable state across `.await`
- [ ] **`tokio::spawn` without joining the handle** — fire-and-forget without error logging

## MEDIUM — Code Quality

- [ ] **Functions over 50 lines** — extract helper
- [ ] **Functions with > 5 parameters** — group into a struct (consider builder)
- [ ] **Deep nesting (> 4 levels)** — use early returns / `?` propagation
- [ ] **Duplicate code patterns** — extract shared fn or trait
- [ ] **Magic numbers without named `const`**
- [ ] **`println!` / `eprintln!` for logging** — use `tracing` macros
- [ ] **`pub use` re-exports outside crate root** without intent

## MEDIUM — Documentation

- [ ] **Public items missing rustdoc** (when `#![warn(missing_docs)]` is set)
- [ ] **Doc examples that don't `cargo test --doc`** — broken intra-doc behavior
- [ ] **`# Panics` / `# Errors` / `# Safety` sections missing** on functions that need them
- [ ] **Broken `[\`OtherType\`]` intra-doc links**

## MEDIUM — Testing Gaps

- [ ] **New public function without tests**
- [ ] **Error paths not tested** — every `Err` variant should have a regression test
- [ ] **Mock overuse** — prefer integration tests with real services where feasible
- [ ] **Missing edge case tests** for boundary values (0, MAX, empty collections, unicode)
- [ ] **`#[ignore]` without comment** explaining why + when to re-enable
- [ ] **Time-dependent test** using `SystemTime::now()` without injected `Clock`

## MEDIUM — Dependency Hygiene

- [ ] **New dependency for trivial functionality** — could be 20 lines of `std`?
- [ ] **Dependency with permissive license that conflicts with project license** (check `cargo deny`)
- [ ] **Dependency with active RustSec advisory** (check `cargo audit`)
- [ ] **Duplicate dependency versions** in `Cargo.lock` — increases binary size
- [ ] **Pinned exact version (`= "1.2.3"`)** without justification — blocks downstream upgrades

## LOW — Style

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] No `#[allow(...)]` without a justification comment
- [ ] No `#[allow(dead_code)]` on production code (only test scaffolding)

## Diagnostic Commands

```bash
cargo check --all-targets --all-features                  # Compile check
cargo clippy --all-targets --all-features -- -D warnings  # Lints
cargo fmt --all -- --check                                # Format
cargo test --all-targets --all-features                   # Tests
cargo test --doc                                          # Doc tests
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps            # Doc build
cargo audit                                               # RustSec advisories
cargo deny check                                          # License / ban policy
cargo tree --duplicates                                   # Duplicate dependency versions
```

## Severity Rubric

- **CRITICAL**: soundness, safety, security — block merge
- **HIGH**: type safety, error handling, idiomatic violations — require revision unless explicitly accepted
- **MEDIUM**: quality, docs, tests, deps — require justification to accept as-is
- **LOW**: style, formatting — auto-fix via `cargo fmt` / `cargo clippy --fix`
