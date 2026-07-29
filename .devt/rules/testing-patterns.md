# Testing Patterns — Rust

> Template baseline — the project's own `CLAUDE.md` and documented conventions WIN on any conflict with this file. Tailor it to the project (`/devt:setup`); untailored copies are flagged by `/devt:setup --health`.

Devt-integration document. Read by `tester`, `programmer`, `code-reviewer`, and `verifier` agents. Defines how tests are written, organized, and run in Rust projects.

## Test Pyramid

1. **Integration tests** (`tests/`) — verify crate boundaries; each `tests/*.rs` file is a separate test binary linked against the public API
2. **Doc tests** (`///` examples) — verify public API examples in rustdoc; run as part of `cargo test`
3. **Unit tests** (`#[cfg(test)] mod tests`) — isolated logic per module, fastest feedback

## TDD Workflow (Red-Green-Refactor)

When implementing new features or fixing bugs, consider test-first development:

### RED: Write Failing Test

```rust
#[test]
fn validates_email_rejects_empty_string() {
    assert!(!validate_email(""));  // function doesn't exist yet
}
```

Run `cargo test validates_email_rejects_empty_string` — the test MUST fail (compile error or assertion failure). If it passes, the feature already exists or the test is wrong.

### GREEN: Make It Pass

Minimal code to make the test pass — no future-proofing.

### REFACTOR: Clean Up (only if needed)

Improve code (naming, structure, DRY) — run ALL tests; they MUST still pass. If tests break, undo the refactor. Working > clean.

### Why TDD?

- A test that passes immediately proves nothing — see it fail first
- Minimal implementation prevents over-engineering
- Refactoring with tests is safe; without tests is gambling

## Regression Test Pattern

When fixing a bug:

1. Write a test that reproduces the bug
2. Run it — MUST fail (proves the test catches the bug)
3. Apply the fix
4. Run it — MUST pass
5. Revert the fix temporarily — MUST fail again (proves causation)
6. Re-apply the fix and commit

This proves the test catches the specific bug, not something else.

## File Naming + Organization

```
src/
├── lib.rs
├── user.rs                       # source with inline #[cfg(test)] mod tests
└── repository.rs

tests/                            # integration tests — each file is a test crate
├── api_integration.rs
├── repository_integration.rs
└── common/
    └── mod.rs                    # shared helpers (use `mod common;` from each test file)

benches/                          # criterion benchmarks (optional)
└── parse_bench.rs
```

- Unit tests stay in the source file under `#[cfg(test)] mod tests { ... }` — never split unit tests into separate files
- Integration tests live in `tests/` and link against the crate's public API only
- Shared helpers for integration tests live in `tests/common/mod.rs` — loaded via `mod common;` in each test file

## Unit Test Structure

```rust
// src/user.rs
pub fn validate_email(email: &str) -> bool {
    !email.is_empty() && email.contains('@')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_string() {
        assert!(!validate_email(""));
    }

    #[test]
    fn rejects_missing_at_sign() {
        assert!(!validate_email("user.example.com"));
    }

    #[test]
    fn accepts_minimal_valid_email() {
        assert!(validate_email("a@b"));
    }
}
```

- Arrange / Act / Assert pattern — separated by blank lines when helpful
- One assertion concept per test (multiple `assert!` on the same object are fine)
- Descriptive names that explain the scenario + expected behavior — `validates_email_rejects_empty_string`, not `test_1`
- Never use temporal markers in names ("new_test", "old_logic") — describe behavior

## Async Test Patterns

```rust
#[tokio::test]
async fn loads_user_from_repo() {
    let repo = InMemoryUserRepository::new();
    repo.save(User::new("a@b.com")).await.unwrap();

    let loaded = repo.find_by_email("a@b.com").await.unwrap();
    assert!(loaded.is_some());
}
```

- Use `#[tokio::test]` for async test functions (assumes the project uses Tokio)
- Use `#[tokio::test(flavor = "multi_thread")]` when concurrency under test matters
- Alternative runtimes: `#[async_std::test]` (async-std), `#[smol_potat::test]` (smol)

## Integration Tests

```rust
// tests/api_integration.rs
use my_crate::application::GetUserUseCase;
use my_crate::infrastructure::InMemoryUserRepository;

#[tokio::test]
async fn get_user_returns_not_found_for_unknown_id() {
    let repo = InMemoryUserRepository::new();
    let use_case = GetUserUseCase::new(Arc::new(repo));

    let result = use_case.execute(UserId::new()).await;
    assert!(matches!(result, Err(GetUserError::NotFound)));
}
```

- Each file in `tests/` is compiled as a separate test binary — slower to compile, isolated at runtime
- Integration tests use ONLY the crate's public API (no `pub(crate)` access)
- For repository tests with real databases, use `testcontainers` (Rust crate) or `sqlx::test` (sqlx-managed transactions)

## Doc Tests

```rust
/// Validates an email address.
///
/// # Example
///
/// ```
/// use my_crate::validate_email;
///
/// assert!(validate_email("user@example.com"));
/// assert!(!validate_email(""));
/// ```
pub fn validate_email(email: &str) -> bool {
    !email.is_empty() && email.contains('@')
}
```

- Run via `cargo test --doc` or as part of `cargo test`
- Doc examples are first-class tests — broken examples are broken tests
- For setup-heavy examples, use ` ```no_run` (compiles but doesn't execute) or ` ```ignore` (skipped entirely)

## Frameworks + Tools

- `cargo test` — built-in test runner; no external test framework needed
- `tokio-test` — async test utilities (`tokio_test::block_on`, mocking)
- `assert_matches` — pattern-matching assertions cleaner than nested `match`
- `pretty_assertions` — diff output for `assert_eq!` on large structs
- `rstest` — parameterized tests + fixtures
- `mockall` — mock generation for trait-based DI
- `proptest` / `quickcheck` — property-based testing
- `criterion` — benchmarking (in `benches/`)
- `testcontainers` — ephemeral databases / services in CI
- `cargo-tarpaulin` / `cargo-llvm-cov` — coverage measurement

## Mocking Rules

- Mock at trait boundaries only (repository traits, external clients)
- Never mock the type under test
- `mockall` is the standard mock-generator; manual mocks are fine for small trait surfaces
- Integration tests using real databases — no mocking the database layer
- Each `mock!` declaration carries a comment explaining the boundary it's mocking

```rust
mockall::mock! {
    pub UserRepository {}
    #[async_trait::async_trait]
    impl UserRepository for UserRepository {
        async fn find(&self, id: &UserId) -> Result<Option<User>, RepoError>;
    }
}
```

## Speed Targets

| Tier              | Target  | Scope                                        |
| ----------------- | ------- | -------------------------------------------- |
| Unit              | < 1 min | Per-module logic, no I/O                     |
| Doc tests         | < 30s   | Public-API examples                          |
| Integration       | < 5 min | Real DB / external service via testcontainers|
| Property + bench  | n/a     | Run on perf-investigation cycles, not CI     |

## Coverage Requirements

- Every public function has tests
- Edge cases: empty inputs, boundary values, error conditions
- Error paths: verify correct error variant via `matches!` or destructuring
- No placeholder tests — every test asserts meaningful behavior

## Property-Based Tests

For invariant-style properties (input space is huge, hand-written cases miss edge cases):

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn validate_email_never_panics(s in ".*") {
        let _ = validate_email(&s);  // assert: doesn't panic
    }

    #[test]
    fn parse_then_format_roundtrips(n in 0u32..1_000_000) {
        let s = format!("{}", n);
        assert_eq!(s.parse::<u32>().unwrap(), n);
    }
}
```

## Benchmarks

```rust
// benches/parse_bench.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_parse(c: &mut Criterion) {
    c.bench_function("parse_email", |b| {
        b.iter(|| validate_email(black_box("user@example.com")))
    });
}

criterion_group!(benches, bench_parse);
criterion_main!(benches);
```

Run with `cargo bench`. Not part of the gate stack — run on perf-investigation cycles.

## Test Anti-Patterns

- **Tests that don't assert**: `assert!(true)`, `()` return, no panic — vacuously passing
- **Order-dependent tests**: tests that fail when run in isolation — use `--test-threads=1` to diagnose
- **`#[ignore]` without justification**: every ignored test has a comment explaining why + when to re-enable
- **`unwrap()` chains masking the real assertion**: use `.expect("specific invariant")` so failures explain themselves
- **Time-dependent tests**: never use `std::time::SystemTime::now()` — inject a `Clock` trait so tests can advance virtual time
