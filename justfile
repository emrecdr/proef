# just = thin aliases; automation logic lives in xtask (TECH-SPEC §15)

default:
    @just --list

build:
    cargo build

test:
    cargo nextest run

doctest:
    cargo test --doc

fmt:
    cargo fmt --all

# All CI gates, locally (global definition of done). cargo-audit runs
# nightly in CI; keep `just audit` for on-demand advisory sweeps.
gates:
    cargo fmt --all --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo nextest run
    cargo test --doc
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
    cargo deny check
    cargo machete
    cargo run -p xtask -- public-api
    cargo check --manifest-path fuzz/Cargo.toml --all-targets --locked
    cargo run -p xtask -- docs-check

# The complexity guards, alone. Timing assertions cannot share a machine with
# the rest of the suite — measured: the linear-validation ratio reads 2.05x
# alone and 3.09x under nextest's full parallelism — so they are `#[ignore]`d
# and run only here and in their own CI step.
perf:
    cargo nextest run --run-ignored only -E 'test(validation_cost_stays_linear)'

audit:
    cargo audit

# Line coverage over the whole workspace via cargo-llvm-cov + nextest
# (install once: `cargo install cargo-llvm-cov`). Advisory, not a gate: the
# 2026 norm is a *ratchet* (fail only if coverage drops), not a fixed
# threshold — see docs/TESTING-STRATEGY.md. `just cover` prints the summary;
# `just cover-html` opens a browsable report. Doctests are excluded (nextest
# does not run them); the number is line coverage of the unit + integration
# suites.
cover:
    cargo llvm-cov nextest --workspace --summary-only

cover-html:
    cargo llvm-cov nextest --workspace --html --open

# Emit lcov for a CI coverage service (Codecov/Coveralls) — used by the
# optional coverage job, not by `just gates`.
cover-lcov:
    cargo llvm-cov nextest --workspace --lcov --output-path lcov.info

doctor:
    cargo run -p proef -- doctor

fixture:
    cargo run -p xtask -- fixture

canary:
    cargo run -p xtask -- canary
