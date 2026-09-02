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

doctor:
    cargo run -p proef -- doctor

fixture:
    cargo run -p xtask -- fixture

canary:
    cargo run -p xtask -- canary
