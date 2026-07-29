# Documentation — Rust

> Template baseline — the project's own `CLAUDE.md` and documented conventions WIN on any conflict with this file. Tailor it to the project (`/devt:setup`); untailored copies are flagged by `/devt:setup --health`.

## Crate-Level Documentation

Every library crate begins with crate-level docs via inner-attribute `//!` comments at the top of `lib.rs`:

```rust
//! # my_crate
//!
//! `my_crate` provides X for Y. The primary entry point is [`Client::new`].
//!
//! ## Quick Start
//!
//! ```
//! use my_crate::Client;
//!
//! let client = Client::new("api-key");
//! ```

#![warn(missing_docs)]
```

`#![warn(missing_docs)]` at crate root surfaces undocumented public items as compiler warnings; turn into errors via `cargo doc` with `RUSTDOCFLAGS="-D warnings"`.

## Public Items

Every public function, struct, enum, trait, and constant gets a doc comment:

```rust
/// Validates an email address.
///
/// Returns `true` when the input is well-formed per the simplified RFC-5321 subset
/// this crate supports — non-empty, single `@`, no whitespace.
pub fn validate_email(email: &str) -> bool { ... }
```

Style:

- Start with a one-sentence summary
- Blank line, then the longer-form explanation
- Use full sentences with terminating punctuation
- Describe behavior + contracts, not implementation

## Required Sections

Three rustdoc sections are mandatory in the situations they describe:

### `# Examples`

Every public item gets at least one example. Examples are executed by `cargo test --doc`:

```rust
/// Parses an ISO-8601 date.
///
/// # Examples
///
/// ```
/// use my_crate::parse_date;
///
/// let d = parse_date("2026-06-06").unwrap();
/// assert_eq!(d.year(), 2026);
/// ```
pub fn parse_date(s: &str) -> Result<Date, ParseError> { ... }
```

### `# Errors`

Required for any function returning `Result<T, E>` when the public API exposes the error type:

```rust
/// # Errors
///
/// Returns [`ParseError::Empty`] when `s` is empty.
/// Returns [`ParseError::InvalidFormat`] when `s` does not match `YYYY-MM-DD`.
```

### `# Panics`

Required for any function that can panic on input variation:

```rust
/// # Panics
///
/// Panics when `buf.len() < 4`. Caller must validate length first.
```

### `# Safety`

Required for every `unsafe fn`. Documents the caller-side invariants:

```rust
/// # Safety
///
/// `ptr` must be a valid, non-null, aligned pointer to at least `len` initialized bytes.
/// Caller must not mutate the underlying memory for the lifetime of the returned slice.
pub unsafe fn from_raw_parts<'a>(ptr: *const u8, len: usize) -> &'a [u8] { ... }
```

## Intra-Doc Links

Use markdown-style links to reference other items:

```rust
/// See [`Client::send`] for the async variant.
/// Errors are documented in [`SendError`].
```

`cargo doc` resolves these and warns on broken links (with `RUSTDOCFLAGS="-D warnings"`, broken links are errors). Prefer intra-doc links over hand-written URLs — they survive renames.

## Doc Tests

Code blocks in `///` comments compile + run as tests:

```rust
/// ```
/// use my_crate::add;
/// assert_eq!(add(2, 3), 5);
/// ```
pub fn add(a: i32, b: i32) -> i32 { a + b }
```

Annotations:

- ` ```ignore` — skipped (use only when the example references an external resource)
- ` ```no_run` — compiles but is not executed (use for I/O-bound examples)
- ` ```compile_fail` — example is expected to fail compilation
- ` ```should_panic` — example is expected to panic
- ` ```text` — opaque text, not Rust (no compile attempt)

## Module-Level Docs

Document each module via inner `//!` at the top of `<module>.rs`:

```rust
//! User-domain types: identifiers, value objects, validation rules.
//!
//! See [`UserId`] for the canonical identifier newtype.

pub struct UserId(uuid::Uuid);
```

## README

- `README.md` at the workspace root — install, build, run instructions
- Each significant crate in a workspace MAY have its own `README.md` linked from `Cargo.toml`:
  ```toml
  [package]
  readme = "README.md"
  ```
- crates.io publishes the README — keep it accurate, focused on HOW to use, not internals

## Cargo doc Workflow

Build local docs:

```bash
cargo doc --no-deps --open
```

Strict CI build (warnings → errors):

```bash
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
```

The strict form catches broken intra-doc links + missing-docs warnings (when `#![warn(missing_docs)]` is set).

## API Documentation (HTTP / gRPC)

When the crate exposes a network API:

- HTTP/REST: use `utoipa` (axum) or framework-native OpenAPI generators
- gRPC: protobuf service definitions live in `proto/` and are the source of truth — generated Rust types document themselves
- Document all endpoints, request/response schemas, error codes
- Include example requests + responses in module-level docs

## Common Failures

- **Doc test references a private item** — fails to compile; either make the item `pub` or use `pub(crate)` + remove from doc test
- **Example uses an old function name** — `cargo test --doc` catches this; rename in docs alongside the code rename
- **Missing `# Errors` section on `Result`-returning fn** — flagged by `clippy::missing_errors_doc` (pedantic)
- **Missing `# Panics` section on fn that calls `.unwrap()`** — flagged by `clippy::missing_panics_doc` (pedantic)
- **Hand-written URL where intra-doc link works** — breaks on rename, doesn't validate
