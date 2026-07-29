# Versioning & release procedure

This document is the versioning policy and the release runbook. The README carries a
summary; this file wins on detail.

## Versioning policy

**Scheme:** [SemVer 2.0.0](https://semver.org). Pre-1.0 semantics, applied strictly:

- **MINOR (0.X.0)** — any breaking change, or a coherent feature series/milestone.
- **PATCH (0.x.Y)** — fixes and purely additive changes that break nothing below.

**What counts as breaking** (these are the public contracts, per the ADRs):

| Surface | Breaking examples | Non-breaking examples |
|---|---|---|
| CLI + exit codes (ADR-0009) | removing/renaming a flag; changing an exit-code meaning | new flag; new subcommand |
| Pack schema (ADR-0004) | removing a key; changing key semantics | new optional key |
| Event wire schema (ADR-0008) | removing/renaming a field or variant; changing `schema` semantics | new variant; new field with a default (additive-only rule) |
| Canonical artifact format (ADR-0010) | any change to emitted bytes (snapshot-locked) | — (changes are inherently breaking; bump minor) |
| Engine seam (ADR-0002) | changing `EngineFactory`/`EngineSession`/`StepBatch`/`ScenarioCtx` shapes | new defaulted trait method |
| Config file | removing/renaming a `proef.toml` key | new optional key |

**1.0.0** is declared when the pack schema, CLI grammar, event schema, and exit codes
are stable enough to promise MAJOR-only breakage. Until then, downstream consumers
should pin minor versions.

**Single source of truth:** `[workspace.package] version` in the root `Cargo.toml`.
Every crate inherits it (`version.workspace = true`); the workspace releases as one
set, always. Never version a crate individually.

**Orthogonal versions, not to confuse with the crate version:**

- The **event schema** version is the `schema` field in `run_started`
  (`EVENT_SCHEMA_VERSION`). It only moves on a semantic break of the stream —
  additive variants/fields do not bump it.
- The **hurl pins** (`=8.0.1`) never move as a side effect of a release. Upgrades go
  exclusively through the canary + runbook (IMPLEMENTATION-PLAN §7, ADR-0003).
- **MSRV** is the toolchain pinned in `rust-toolchain.toml`; it may rise in any
  MINOR release pre-1.0 and is not a separate contract yet.

**Tags:** annotated `vX.Y.Z` on `main`, linear history. **Cadence:** release when a
milestone or a coherent series lands — not on a calendar.

## CHANGELOG rules

[Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/):

- `## [Unreleased]` always exists at the top; every landed change adds a line there
  in the same commit series that lands it.
- On release, `Unreleased` content moves under `## [X.Y.Z] - YYYY-MM-DD` (with a
  short parenthetical theme) and a fresh empty `Unreleased` is left behind.
- The compare-link references at the bottom of the file are updated every release
  (`[Unreleased]` compares `vX.Y.Z...HEAD`).

## Release runbook

From a clean, green `main` (all gates local + CI):

```bash
# 1. Cut the changelog: move [Unreleased] → [X.Y.Z] - date, update bottom links.
# 2. Bump the single source:
#      [workspace.package] version = "X.Y.Z"      (root Cargo.toml)
cargo build --workspace                            # refreshes Cargo.lock versions
# 3. Full gates:
cargo nextest run && cargo test --doc
cargo clippy --all-targets --all-features -- -D warnings && cargo fmt --all --check
cargo deny check && cargo audit
# 4. Commit + tag + push:
git commit -am "release: vX.Y.Z"
git tag -a vX.Y.Z -m "proef X.Y.Z"
git push origin main --follow-tags
```

The tag push triggers `.github/workflows/release.yml`, which:

1. builds release binaries for five targets (macOS arm64/x86_64, Linux
   arm64/x86_64-gnu, Windows x86_64-msvc — the Windows zip bundles the vcpkg
   DLLs; macOS links the SDK's system libxml2 and vendors OpenSSL, so shipped
   binaries need no Homebrew), via `cargo auditable` (binaries stay scannable)
   with **no cache restore** (cache poisoning must not reach published
   artifacts), attesting SLSA build provenance per artifact once the repo is
   public;
2. publishes the GitHub Release with the version's CHANGELOG section and all
   five archives (asset names must stay in sync with the `binstall` metadata in
   the `proef` package manifest);
3. regenerates `Formula/proef.rb` in the `emrecdr/homebrew-proef` tap (deploy-key
   auth via the `HOMEBREW_TAP_DEPLOY_KEY` repo secret).

`workflow_dispatch` runs build+attest only — a full matrix smoke without
publishing. crates.io publication remains a deliberate manual `cargo publish`
per crate in dependency order (core → engine-hurl → proef) and is **not**
automated.

## History

- `v0.1.0` — initial release (fresh history baseline, 2026-07-29)
- `v0.2.0` — deep-review correctness blockers (duplicate-request, body
  corruption, delay budget), panic containment, the output contract, and the
  author guides — breaking: `--output json` stream split, empty selections
  exit 2, event schema grew additively
