# ADR-0003 — Upstream tracking: exact pins, thin zero-diff fork, upgrade canary

**Status:** Accepted · **Date:** 2026-07-28

## Context

Requirement (stated): "use a hurl fork with as few changes as possible — I want to keep
using hurl as it improves over time; a wrapper/Gherkin adapter over hurl." Verified:
hurl ships breaking crate-API changes in minor releases (#3846; maintainers advise
`cargo install --locked`); the release cadence is multiple per year; `run_entries` is
`#[doc(hidden)]`. One concrete patch need is already identified (ADR-0010 / TECH-SPEC
§5): `run_entries` creates its HTTP client internally, so accepting `&mut Client` would
erase per-segment connection costs — a verified two-call-site change.

## Decision

Three-tier policy, in order of preference. (1) **Steady state:** depend on published
crates with exact pins (`hurl = "=8.0.1"`, `hurl_core = "=8.0.1"`), build `--locked`;
the GitHub fork exists but carries **zero diff**. (2) **Patch vehicle:** when a release
breaks the seam or a small change is needed, carry a *minimal-diff branch* on the fork,
consumed via Cargo `[patch."crates-io"]` (or a git-tag dep), rebased onto each upstream
release. (3) **Upstream everything:** every patch is PR'd upstream so the fork's diff
trends back to zero. An **upgrade-canary CI job** (weekly + on upstream release) builds
against the next hurl version and replays the full suite; pins move only after it is
green, via the runbook in IMPLEMENTATION-PLAN §7.

## Consequences

"Keep using hurl as it improves" becomes a scheduled chore, not a gamble; the wrapper
never becomes a divergent fork; breakage is discovered pre-pin-bump. Costs: upgrade PRs
are deliberate work per release; the fork must be rebased when (and only when) it
carries a patch; MSRV follows upstream (hurl master already 1.97.1 — neutralized by the
project's always-latest-stable toolchain rule).

## Alternatives considered

Track `master` via git dependency — unvetted breakage flows in continuously. Vendor the
hurl source into the repo — a divergent fork in disguise; loses provenance and cadence.
Caret/tilde version ranges — semver is demonstrably not honored for the library surface;
exact pins are the only safe mode.
