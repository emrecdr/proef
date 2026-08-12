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

## Release runbook

From a clean, green `main` (all gates local + CI).

> **`main` is protected — the release commit goes through a pull request, and the
> tag is pushed only after it merges.** Do not `git push origin main`, and do not
> tag before the merge. `git push --follow-tags` is **not atomic**: git pushes
> refs independently, so a protected-branch rejection stops the branch while the
> tag still lands — and a tag is exactly what `release.yml` triggers on. That
> combination starts a release build from a commit that is not on `main`. It
> happened cutting 0.10.0; the run was cancelled and the tag deleted before
> anything published, but the recovery is avoidable and this ordering avoids it.

```bash
# 1. On a release branch, cut the changelog: move [Unreleased] → [X.Y.Z] - date
#    with a short parenthetical theme, and leave a fresh empty [Unreleased].
#    (There is no link-reference section at the bottom of CHANGELOG.md — nothing
#    to update there.)
git switch -c release/vX.Y.Z
# 2. Bump the version in the root Cargo.toml — BOTH places:
#      [workspace.package] version = "X.Y.Z"       (the crates' own version)
#      [workspace.dependencies] proef-core / proef-engine-hurl / proef-lsp
#        version = "X.Y.Z"
#        (the inter-crate pins — belt-and-suspenders for independent crates.io
#         publish; a stale pin no longer satisfies the bumped version and fails
#         resolution, so these move in lockstep with the line above).
cargo build --workspace                            # refreshes Cargo.lock versions
# fuzz/ is a separate workspace with its own committed lock, and the gates job
# checks it with --locked: refresh it too or that gate goes red on the release
# commit.
cargo check --manifest-path fuzz/Cargo.toml --all-targets
# 3. Full gates:
cargo nextest run && cargo test --doc
cargo clippy --all-targets --all-features -- -D warnings && cargo fmt --all --check
cargo deny check && cargo audit
# 4. Commit and open the release PR (no tag yet):
git commit -am "release: vX.Y.Z"
git push -u origin release/vX.Y.Z
gh pr create --base main --title "release: vX.Y.Z"

# 5. After CI is green and the PR is MERGED, tag the *merged* commit and push
#    only the tag. The squash merge creates a new commit, so tagging the branch
#    would leave the tag off `main`'s history.
git switch main && git pull --ff-only
git describe --tags --exact-match HEAD 2>/dev/null && echo "already tagged — stop"
git tag -a vX.Y.Z -m "proef X.Y.Z"
git push origin vX.Y.Z            # this, and only this, starts release.yml
```

If the release commit was made on `main` locally before branching, `git pull
--ff-only` refuses afterwards: the squash merge superseded it. Confirm the merged
commit carries the version bump, check `git diff --quiet HEAD origin/main`, then
`git reset --hard origin/main`.

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
per crate in dependency order (core → engine-hurl → lsp → proef — `proef-lsp`
before `proef`, which depends on it non-optionally) and is **not** automated.

Manual because it is the one step nothing undoes: a published version can be
yanked, never replaced or re-uploaded. So publish from the tag, not from a
working tree that merely resembles it:

```bash
git describe --tags --exact-match HEAD     # must print vX.Y.Z
git status --short                         # must be empty
cargo publish -p proef-core --dry-run --locked
cargo publish -p proef-core --locked
cargo publish -p proef-engine-hurl --locked
cargo publish -p proef-lsp --locked
cargo publish -p proef --locked
```

`--locked` throughout, so what ships is what the committed lockfile resolves.
Only these four go: `[workspace.package] publish = false` is the default and each
publishable crate overrides it, so `proef-fixture`, `proef-harness` and `xtask`
are excluded by construction rather than by remembering to skip them. Each
command waits for the registry before returning, which is what makes the next
one resolvable.

## History

- `v0.1.0` — initial release (fresh history baseline, 2026-07-29)
- `v0.2.0` — deep-review correctness blockers (duplicate-request, body
  corruption, delay budget), panic containment, the output contract, and the
  author guides — breaking: `--output json` stream split, empty selections
  exit 2, event schema grew additively
- `v0.2.1` — review P0 (header grammar, pipe, filters, name dedup, secrets
  perms) + failure UX (hurl expected/actual, true error-line anchoring)
- `v0.3.0` — data-safety blockers (asset copy, run rotation, zero-entry
  false green), Then-step visibility with exact attribution, UserInput
  taxonomy (user mistakes exit 2), option caps + repeat budget, atomic
  locked stores — breaking: proef-core API pruned, `when:` skips on
  literal false, zero-entry packs fail validation
- `v0.3.1` — secret hardening: `secret rm`, `PROEF_KEY` CI override, the
  saveAs-vs-secret promotion guard, doctor store/key health, corrupt-store
  recovery, warned-step reasons on the console
- `v0.4.0` — external config & environments (`proef.toml` `[url]`/`[vars]`/
  `[env.<name>]`, `${url:}`/`${vars:}`, `--env`/`PROEF_ENV`, ADR-0012), default
  suite path, and the competitive-review breadth pass — breaking: the pack root
  key `templates:` became `macros:` with no alias (ADR-0004 amendment)
- `v0.5.0` — the `proef-lsp` language server: diagnostics, completion,
  go-to-definition and references over the sans-IO core (ADR-0017)
- `v0.5.1` — LSP correctness: process-leak, malformed-request crash,
  broken-pack degradation, root-at-suite, overlay keying; `use:`/`match:`
  go-to-definition
- `v0.5.2` — CLI correctness: diff step-collision, truncated-run gate,
  setup double-run, the first EPIPE guard, overflow hardening, bare-filename
  path resolution, exit-130 documentation
- `v0.5.3` — closed-pipe safety: every remaining raw `eprintln!` in
  `proef-cli` routed through the EPIPE-safe guard (with a source-scanning
  drift test), and `proef-lsp`'s panic-recovery notice no longer kills the
  server it just rescued
- `v0.6.0` — first-run UX & run-record correctness: `proef init`, a
  did-you-mean for unset config variables, a next-command nudge; one
  `run_started`/`run_finished` pair per record with suite-only totals,
  truncated-record banners in `report`/`explain`, a real worker slot index —
  breaking: a scenario with no steps is now an error
- `v0.7.0` — record & artifact integrity: `run_finished` is the record's last
  line again (a watchdog-abandoned scenario no longer appends past it),
  `${fake:…}` values no longer repeat across a scenario's steps, `.map.json`
  stops listing captures that were never made and stops dropping real ones,
  and a whitespace-only `expect:` is rejected instead of emitting an inverted
  span — breaking: `proef_core::resolve::resolve` takes a caller-owned
  occurrence counter and `Resolution::fakes` is gone
- `v0.8.0` — CLI output & exit integrity: an unreadable `PROEF_KEY`/`PROEF_ENV`/
  `PROEF_SECRET_<NAME>` is a loud user error instead of reading as unset, the
  `run.log` tee no longer duplicates bytes on a short write, `proef fmt` keeps a
  file's own line endings, `report -o` writes artifact links that resolve, and
  `diff` stops inventing flakiness for a step with no baseline — breaking: a
  failed stdout write now exits 3 where it exited 0, and a malformed environment
  variable exits 2 where it was silently ignored
- `v0.9.0` — tool-surface integrity & authoring guidance: values interpolated
  into LSP snippets, GitHub annotations and job-summary tables are escaped,
  `proef fmt` refuses a file that is not a pack and stops trimming the YAML
  skeleton, `--sarif` carries `startLine` so annotations land, `--watch`
  retriggers on `proef.toml`, `--dry-run`'s nudge echoes the run that was
  actually validated, a templated `retry:` stops under-counting the batch
  budget, `--output json` reports the real exit, a truncated record counts its
  warned scenarios, a failing run says when the scaffold's routes are still
  placeholders, `macros` prints the sentence an author needs, `proef lsp`
  adopts the client's workspace root, and AUTHORING documents docstring
  placeholders and the validation-catalogue pattern — breaking:
  `proef secret set --value` was removed in favour of `--stdin` (a secret in
  argv is visible to `ps`), and `proef macros --output json`'s `pattern` field
  changed from a boolean to `string|null`