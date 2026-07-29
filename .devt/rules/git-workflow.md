# Git Workflow — Rust

> Template baseline — the project's own `CLAUDE.md` and documented conventions WIN on any conflict with this file. Tailor it to the project (`/devt:setup`); untailored copies are flagged by `/devt:setup --health`.

## Branch Creation Rules

### Always Branch from the Default Branch

```bash
# CORRECT
git checkout main
git pull
git checkout -b feature/<feature-name>

# WRONG — creates branch from another feature branch
git checkout -b feature/add-auth  # while on feature/add-users — pollutes PR
```

### Branch Naming

- `feature/<feature-name>` — New features
- `fix/<issue-description>` — Bug fixes
- `refactor/<scope>` — Code improvements
- `hotfix/<critical-fix>` — Production-critical fixes
- `chore/<scope>` — Build, deps, tooling, release prep

Examples:

- `feature/user-auth`
- `fix/token-expiry-handling`
- `refactor/split-user-repository`
- `chore/bump-tokio-1.42`

## Commit Convention

### Format

```
type(scope): short description

Optional body explaining WHY, not WHAT.

Co-Authored-By: Name <email>
```

### Types

| Type | When |
|------|------|
| `feat` | New feature or capability |
| `fix` | Bug fix |
| `test` | Adding or updating tests |
| `docs` | Documentation only |
| `refactor` | Code change that doesn't fix or add |
| `style` | Formatting, linting (no logic change) |
| `chore` | Build, deps, tooling, release prep |
| `perf` | Performance improvement with measurable evidence |

### Examples

```bash
feat(auth): add JWT token refresh middleware

fix(user): handle duplicate email on registration

test(auth): add proptest case for token-validation edge cases

refactor(repo): extract common query builder behind trait

perf(parser): replace String::push_str with write! — 2x throughput on bench
```

## Pull Request Format

```bash
gh pr create --title "feat(scope): short description" --body "$(cat <<'EOF'
## Summary
- What was done and why

## Changes
- List key files/modules modified

## Test plan
- [ ] `cargo fmt --all -- --check` clean
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean
- [ ] `cargo test --all-targets --all-features` passes
- [ ] `cargo test --doc` passes
- [ ] `cargo doc --no-deps` builds without warnings
- [ ] Manual verification of [specific behavior]

## Notes
- Any context reviewers need
EOF
)"
```

## Cargo.lock Discipline

Two rules, based on crate kind:

- **Binary crates / applications** — `Cargo.lock` IS committed. Reproducible builds for the deployed artifact require the exact dependency tree.
- **Library crates (published to crates.io)** — `Cargo.lock` IS NOT committed (add to `.gitignore`). Downstream consumers must be free to resolve their own dependency tree.
- **Mixed workspaces (binary + libraries)** — `Cargo.lock` IS committed (the workspace contains a binary).

Drift detection in CI:

```bash
cargo check --locked --all-targets --all-features
```

`--locked` fails when `Cargo.lock` would need changes — surfaces uncommitted dependency updates before merge.

## Semver Discipline

Public API changes require a version bump:

- **Major** (`1.x.y → 2.0.0` / `0.x.y → 0.(x+1).0` pre-1.0) — breaking changes to public types, function signatures, trait signatures, public-API behavior
- **Minor** (`1.0.0 → 1.1.0`) — backward-compatible additions: new `pub` items, new trait methods with default impls
- **Patch** (`1.0.0 → 1.0.1`) — backward-compatible bug fixes, internal refactors, documentation

Detection tool (recommended for release-prep cycles):

```bash
cargo install cargo-semver-checks
cargo semver-checks
```

Catches unintentional breaking changes that a manual eyeball review would miss.

## Version Bump — Keep Sources In Sync

When a PR introduces a change that justifies a new version, bump **every** version source the project uses in the same commit:

- `Cargo.toml` `version` field (for the crate being released)
- `Cargo.lock` regenerated via `cargo build` (commit alongside)
- Workspace `[workspace.package].version` when using workspace inheritance
- `CHANGELOG.md` section header `## [X.Y.Z] - YYYY-MM-DD`
- A `vX.Y.Z` git tag (tagged AFTER the bump commit lands on the default branch)

All numeric values MUST match. CI typically rejects main-bound merges where `Cargo.toml` version is not strictly greater than main's.

For workspaces with independent crate versions, bump only the affected crate(s). For workspaces sharing a single version, bump the workspace root and all member crates simultaneously.

## Common Mistakes and Fixes

| Mistake | Fix |
|---------|-----|
| Committing `target/` | Add to `.gitignore` (almost always already there) |
| Committing `Cargo.lock` from a library crate | Remove + add to `.gitignore` (libraries float deps) |
| Omitting `Cargo.lock` from a binary crate | Commit it — reproducible builds require pinned deps |
| PR includes unrelated formatting | Run `cargo fmt` separately from feature commits |
| Branch from feature branch | Always `git checkout main && git pull` first |
| Giant PR (50+ files) | Split by crate or concern into smaller PRs |
| Commit message says "fix stuff" | Use conventional format: `fix(scope): what` |
| Bumped `Cargo.toml` without regenerating `Cargo.lock` | Run `cargo build` to refresh `Cargo.lock` and recommit |
| Released without `cargo semver-checks` | Run before tagging; catches accidental breaking changes |

## Pre-Commit Hook (Recommended)

```bash
#!/usr/bin/env bash
set -euo pipefail

cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo check --locked --all-targets --all-features
cargo test --all-targets --all-features
```

Drop in `.git/hooks/pre-commit` and `chmod +x`. For a project-wide hook (versioned, shared), use [`cargo-husky`](https://crates.io/crates/cargo-husky) or [`lefthook`](https://github.com/evilmartians/lefthook).

---

## Quality Checklist

Before creating a PR:

- [ ] Compiles: `cargo check --all-targets --all-features`
- [ ] Tests pass: `cargo test --all-targets --all-features`
- [ ] Doc tests pass: `cargo test --doc`
- [ ] Format clean: `cargo fmt --all -- --check`
- [ ] Lints clean: `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] Docs build: `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`
- [ ] `Cargo.lock` regenerated if `Cargo.toml` changed (and committed for binary crates)
- [ ] No dead code introduced (no `#[allow(dead_code)]` outside test scaffolding)
- [ ] Commit messages follow convention
- [ ] PR description explains WHY, not just WHAT
- [ ] If version was bumped: every version source the project uses (`Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, eventual git tag) shows the same `X.Y.Z`
- [ ] For library crates: `cargo semver-checks` ran clean (or breaking change is intentional + major bump applied)
