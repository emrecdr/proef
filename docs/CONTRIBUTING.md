# Contributing to proef

Small, focused PRs against `main`. The corpus in `docs/` is the source of
truth — `CLAUDE.md` is the working summary, `docs/TECH-SPEC.md` and the ADRs
win on conflict.

## Setup

The toolchain is pinned by `rust-toolchain.toml` (latest stable; rustup picks
it up automatically). One-time tools:

```bash
cargo install cargo-nextest cargo-deny cargo-audit cargo-insta just
# plus, for the full CI surface locally:
cargo install cargo-fuzz cargo-public-api cargo-machete
```

Native build prerequisites (only `proef-engine-hurl` needs them):
Debian/Ubuntu `apt install build-essential pkg-config libssl-dev
libcurl4-openssl-dev libxml2-dev libclang-dev`; macOS: Xcode CLT.

## The gates (green before every commit)

```bash
cargo nextest run                     # all tests
cargo test --doc                      # doctests (nextest skips them)
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo deny check
cargo run -p xtask -- docs-check      # indexes ↔ reality
cargo run -p xtask -- public-api      # proef-core API surface (nightly rustdoc)
```

`just` carries aliases for the common ones. CI additionally runs
`cargo machete`, `zizmor`, a fuzz smoke, and the hurl canary; `cargo audit`
runs nightly.

## Rules that are easy to trip over

- **hurl pins are exact** (`=8.0.1`, built `--locked`). Never bump them in a
  PR — upgrades go through the canary + runbook (ADR-0003).
- **Snapshots are deliberate.** Artifact bytes, sidecars, diagnostics, and
  event streams are insta-locked; `cargo insta review` each diff and be able
  to say why it changed. Never blind-accept.
- **`proef-core` API is snapshot-locked** (`crates/proef-core/public-api.txt`).
  An intended surface change regenerates it:
  `PROEF_PUBLIC_API_UPDATE=1 cargo run -p xtask -- public-api`.
- **Core purity:** `proef-core` does no IO and reads no clocks/env/randomness
  — inject values instead. This keeps every snapshot deterministic.
- **New architectural decision → new ADR** (`docs/adr/ADR-00NN-*.md`, next
  number, same format) in the same PR. Diverging from an ADR without a
  superseding one is a bug.
- **New diagnostic → index it** in `docs/DIAGNOSTICS.md`, prefer a seeded
  case under `tests/errors/<area>__<name>/` (dry-running that corpus fails
  by design).
- **YAML is `serde_norway`**, datetime is `jiff`, and reqwest/async-trait/
  a tokio runtime are banned (see `CLAUDE.md` for the full list and why).
- **No raw print macros in `proef-cli`.** Use `crate::render::outln!` for
  stdout and `crate::render::errln!` for stderr. `println!`/`eprintln!` panic
  when the write fails, and a closed pipe (`proef … | head`) surfaces as EPIPE
  rather than a signal — so a raw macro aborts with 101, outside the typed
  0/1/2/3 exit contract (ADR-0009). A source-scanning test enforces this.

## Testing

`docs/TESTING-STRATEGY.md` is normative. In short: everything is device- and
network-free except the fixture integration suite
(`cargo run -p xtask -- fixture` runs the dev API server standalone). Assert
attempt counts and normalized event order, never wall-clock.

## Commit messages

Conventional prefixes (`fix:`, `feat:`, `docs:`, `refactor:`, `release:`),
imperative subject, body explains *why*. No AI-attribution footers.
