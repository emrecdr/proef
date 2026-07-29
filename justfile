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

# All CI gates, locally (global definition of done)
gates:
    cargo fmt --all --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo nextest run
    cargo test --doc
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
    cargo deny check
    cargo audit

doctor:
    cargo run -p proef -- doctor

fixture:
    cargo run -p xtask -- fixture

canary:
    cargo run -p xtask -- canary
