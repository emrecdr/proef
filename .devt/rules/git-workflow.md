# Git Workflow — proef

> Grounded in `docs/CONTRIBUTING.md` and `CLAUDE.md`. Those sources WIN on any conflict.

Trunk-based: small, focused PRs against **`main`**. The `docs/` corpus is the source of
truth; `CLAUDE.md` is the working summary.

## Committing and pushing

- **Commit or push only when the user asks.** If you are on `main` and the user wants a
  commit, branch first.
- Keep PRs small — split by crate or concern. A giant PR (many crates at once) is a smell.

## Commit messages

- Conventional lowercase prefixes: **`feat:` · `fix:` · `docs:` · `refactor:` · `test:` ·
  `chore:` · `release:` · `perf:`** (scope optional, e.g. `feat(secret): …`). Imperative
  subject; body explains **why**, not what.
- **No AI-attribution footers** — no "Generated with", no "Co-Authored-By: Claude", no
  "powered by" trailers. See `[[no-ai-footers-in-commits]]`.

## Rules that are easy to trip over in a diff

- **Never bump the hurl pins** (`=8.0.1`) in a feature/fix PR — upgrades go through the
  canary + runbook only (ADR-0003). A pin bump outside that path fails review.
- **Snapshots are deliberate.** Artifact bytes, sidecars, diagnostics, and event streams
  are insta-locked. `cargo insta review` each diff and be able to say why it changed —
  never blind-accept (ADR-0010).
- **`proef-core` public API is snapshot-locked** (`crates/proef-core/public-api.txt`); an
  intended surface change regenerates it in the same PR.
- **A new architectural decision → a new ADR** (`docs/adr/ADR-00NN-*.md`) in the same PR;
  update the `CLAUDE.md` Status list as milestones land.
- **A new diagnostic code → a `docs/DIAGNOSTICS.md` row** + a seeded `tests/errors/` case
  where reachable.

## Cargo.lock & drift

The workspace ships binaries (`proef`, `xtask`) → `Cargo.lock` **is committed**. hurl is
built `--locked`; CI runs with `--locked`, so refresh and commit `Cargo.lock` whenever
`Cargo.toml` changes.

## Before you commit — gates green

```bash
cargo nextest run
cargo test --doc
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo deny check
cargo run -p xtask -- docs-check
```

`just` carries aliases for the common ones. See `quality-gates.md` for the full gate stack
(machete, public-api, fuzz smoke, canary, nightly audit).

## Releases

Version bumps go through `docs/RELEASING.md` (prefix `release:`). Bump every version source
the project uses in the same commit; tag `vX.Y.Z` after the bump lands on `main`.
