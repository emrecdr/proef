# Coding Standards

> Template baseline — the project's own `CLAUDE.md` and documented conventions WIN on any conflict with this file. Tailor it to the project (`/devt:setup`); untailored copies are flagged by `/devt:setup --health`.

Devt-integration document for Rust projects. Read by `code-review-guide`, `complexity-assessment`, and `architect` skills. Rules express the project's idiomatic Rust shape so agents catch deviations early.

## Edition + MSRV

- **Edition**: Rust 2024 (default for new crates). Set `edition = "2024"` in every `Cargo.toml`. Older crates targeting 2021 are acceptable but should migrate at the next major version.
- **MSRV (Minimum Supported Rust Version)**: Library crates declare an explicit `rust-version = "1.X.0"` in `Cargo.toml` — typically 6–12 months behind current stable. Application crates may track stable without an MSRV constraint.

## Naming

- `snake_case` for functions, variables, modules, file names
- `PascalCase` for types, traits, enums, type parameters
- `SCREAMING_SNAKE_CASE` for constants and `static` items
- Acronyms in identifiers: lowercase if not the first word (`http_url`, `json_parser`), PascalCase when standalone (`HttpClient`)
- Avoid prefixes like `get_` on getters — return the value directly (`user.name()`, not `user.get_name()`)
- Boolean fields and functions read as predicates: `is_active`, `has_permission`, `should_retry`

## Module organization

- Prefer the `<name>.rs` form over `<name>/mod.rs` for new modules. Mixing both in one workspace is a smell — pick one and stick.
- Inline `mod foo { ... }` is acceptable for private helpers when the body is small (< ~100 lines); promote to a separate file once it grows.
- Use `mod tests { ... }` blocks at the bottom of source files for unit tests; do NOT factor unit tests into a separate file.
- Integration tests live in `<crate-root>/tests/`. Each file is a separate test crate.
- Benchmarks live in `<crate-root>/benches/` and use `criterion` (preferred) over the unstable built-in `#[bench]`.

## Visibility

- Minimize `pub`. Default to private; surface only what callers genuinely need.
- Prefer `pub(crate)` for items used across modules within the same crate.
- Use `pub(in path::to::module)` for fine-grained re-exports across sibling modules.
- Re-exports (`pub use`) belong at the crate root for stable public API; intermediate modules should not `pub use` lower modules without intent.
- Newtype wrappers (`pub struct UserId(u64);`) keep field private unless the wrapping is purely cosmetic.

## Error handling

Two-crate split based on context:

- **Libraries**: define error types via `thiserror`. Each public error variant captures its source (`#[from]` or `#[source]`). Never use `Box<dyn Error>` in library public API — callers can't match on it.
- **Applications + binaries**: use `anyhow::Result<T>` with `anyhow::Context::context` and `with_context` to add diagnostic information at each `?` site that would otherwise lose context.
- **Alternative for cause-chain debugging**: `eyre` (richer error reports, drop-in alternative to `anyhow`). Pick one per crate; do not mix.

Result conventions:

- Functions that can fail return `Result<T, E>`; never panic on input variation.
- Use `?` for error propagation; reach for `match` only when each variant truly needs different handling.
- `unwrap()` and `expect()` are banned outside `tests`, `main`, and contexts where invariants prove the case is unreachable. Every `expect("...")` must include a message explaining the invariant.
- For "this should be impossible" cases, prefer `unreachable!("...")` with a justification, not `unwrap()`.

## Ownership and borrowing

- Take references (`&T`, `&mut T`) by default. Reach for owned `T` only when the function logically consumes the value (constructors, channel sends, async returns).
- Avoid `.clone()` reflexively — most `.clone()` calls signal a borrowing redesign. Reach for `Cow<'a, T>`, `Arc<T>`, or borrowing instead when ownership is conditional.
- Lifetimes: elide where possible (most function signatures don't need explicit lifetimes). Add explicit lifetimes only when the compiler asks or when documentation needs them visible.
- Iterators over `Vec<T>::iter()` / `Vec<T>::iter_mut()` over indexed `for i in 0..` loops — clearer intent + better optimization.

## Mutability + Functional style

- Minimize `mut`. Prefer rebinding (`let x = transform(x);`) or iterator chains over in-place mutation.
- Pipelines: prefer `iter().map().filter().collect()` over manual loops with mutable accumulators when the chain is ≤ 4 steps.
- For longer chains, name each intermediate to keep the pipeline readable.

## Async

- Tokio is the dominant runtime; assume it unless the project declares otherwise (`async-std`, `smol`).
- Async functions return `impl Future<Output = ...>` implicitly via `async fn`. Document expected behavior under cancellation.
- Channel choice:
  - `tokio::sync::mpsc` for back-pressured single-consumer
  - `tokio::sync::broadcast` for fan-out events
  - `flume` for sync+async interop
  - `std::sync::mpsc` for purely synchronous code only
- Use `tokio::select!` for racing futures; avoid manual `Future` polling.

## Logging + tracing

- Use the `tracing` crate over `log` for new code. `tracing` is structured, span-aware, and the dominant modern choice.
- Library code: emit `tracing::trace!`, `debug!`, `info!`, `warn!`, `error!` macros — do NOT initialize a subscriber.
- Application code: configure a subscriber at startup (typically `tracing-subscriber` with `EnvFilter`).
- Add `#[tracing::instrument(...)]` to entry points; control verbosity via `level` and `skip` attributes.

## Documentation

- `#![warn(missing_docs)]` at crate root for library crates. Optional for binaries.
- Every public item gets a doc comment with at least one example.
- Examples in `///` blocks are tested by `cargo test --doc` — write them as if they're tests.
- Use `//!` for module-level docs explaining the module's purpose, not for item docs.
- Link to other items with `[\`OtherType\`]` syntax; `cargo doc` resolves and validates these.

## Formatting + linting

- `cargo fmt -- --check` MUST pass on every commit. Pre-commit hook recommended.
- `cargo clippy -- -D warnings` MUST pass — clippy warnings are errors. Allowlist specific lints in `Cargo.toml` `[lints.clippy]` table with justification comments.
- Common pedantic lints worth enabling project-wide: `clippy::pedantic`, `clippy::nursery`, `clippy::cargo`. Allow specific noise lints (`clippy::module_name_repetitions`, `clippy::missing_errors_doc`) per-project as needed.

## `#[must_use]` discipline

- Annotate every result-like return that callers must inspect: handles, futures (auto-applied to `Future`), iterators with side-effecting `next`, error types, validation outputs.
- For methods returning a builder, `#[must_use = "Builder does nothing until .build() is called"]` documents intent.

## Cargo features

- Feature flags carry semantic meaning — name them after what they enable, not what they disable.
- Document each feature in `Cargo.toml` with a comment explaining when consumers should enable it.
- Default features should match the most common use case; opt-in features cover specialized scenarios.
- Avoid `default-features = false` patterns in transitive dependencies unless declared explicitly — they can break unrelated consumers.

## Unsafe code

- `unsafe` blocks require a `// SAFETY:` comment explaining the invariant that makes the call safe.
- Public functions exposing `unsafe` must be marked `unsafe fn` and document preconditions in `# Safety` rustdoc section.
- Library crates should minimize `unsafe`; consider `#![forbid(unsafe_code)]` at crate root when no `unsafe` is needed (pure-safe library).
- Each `unsafe` block reviewed independently — soundness reasoning is local.

## What this document does NOT cover

- General Rust education (see The Rust Book + Rust by Example)
- Web framework specifics (axum, actix-web, rocket — choose per-project)
- Database integration (sqlx, diesel, sea-orm — choose per-project)
- Specific design patterns (see `architecture.md` for the structural patterns devt enforces)
