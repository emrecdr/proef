# ADR-0009 — Error taxonomy by fault, stable exit codes, miette at the edge

**Status:** Accepted · **Date:** 2026-07-28

## Context

The error model categorizes by *who is at fault* — `User` /
`TestFailure` / `System(anyhow)` — with a total mapping to its stable exit-code scheme
(0 ok · 1 test failure · 2 user error · 3 system error), integration-tested via
assert_cmd. 2026 ecosystem consensus (blessed.rs et al.): thiserror in libraries, anyhow
at the application edge; miette's `Diagnostic` adds codes/help/labeled source spans for
user-facing errors. Survey of backend traits (tower/sqlx/rustls/probe-rs): behind `dyn`,
a unified error enum with boxed sources beats associated `type Error`. gherkin 0.16
spans are byte offsets (verified), directly convertible to miette `SourceSpan` (with an
EOF-trailing-newline clamp; `LineCol.column` is char-counted — never mixed into byte
math). snafu/error-stack: adopted by some large codebases, unnecessary at this crate
count.

## Decision

A fault-category model, extended for engines: `proef-core` defines

```rust
pub enum CoreError { User(..), TestFailure(..), System(..) }        // → exit 2 / 1 / 3
pub struct EngineError { pub class: EngineErrorClass,               // Infra | AssertFailed | Setup
                         pub message: String,
                         pub source: Option<Box<dyn Error + Send + Sync>> }
```

`AssertFailed` folds into `TestFailure`; `Infra`/`Setup` into `System`. thiserror 2
everywhere; no `anyhow` in library crates (only inside `System`'s boxed source at the
edge). miette lives **only in `proef-cli`**: parse/bind/validation errors wrap into
`Diagnostic`s with labeled spans into `.feature` files (gherkin byte spans) and pack
YAML (serde_norway locations); engine failures render the feature line + artifact span
from the sidecar. Exit codes are a typed enum, pinned by CLI integration tests.

## Consequences

Every failure has a fault category, a stable exit code, and a source-located rendering;
engine crates stay miette-free (usable headless); the `explain` command reuses the same
classification. Cost: two error layers (core vs engine) — justified by the seam:
engines can't know exit codes, core can't know engine internals.

## Alternatives considered

Associated `type Error` on engine traits — erased behind `dyn` anyway. anyhow
everywhere — loses matchable categories that exit codes require. snafu — per-crate
context ergonomics we don't yet need; revisit if the workspace grows past ~10 crates.
