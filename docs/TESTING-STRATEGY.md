# proef — Testing Strategy

**Status:** normative · **Date:** 2026-07-28 · tools: cargo-nextest, insta, proptest,
cargo-fuzz, assert_cmd, tiny_http fixture. Everything below is device-free and CI-green with
no external network.

## 1. The layers

**Unit** (every crate): matcher tokenization/matching edge cases; resolver escapes and
depth cap; Then-merge rules; batching/segmentation boundaries; sidecar math; World
error→exit-code mapping.

**Property (proptest):** matcher — arbitrary patterns/text never panic, valid
pattern+generated text round-trips captures; resolver — `$${…}` escape round-trip,
resolution is idempotent once fully resolved, depth cap always terminates; **secret-mask
invariant** — for arbitrary events/reports containing a known secret value, rendered
output never contains it (ADR-0005); World — snapshot/restore is an involution.

**Fuzz (cargo-fuzz, nightly job + PR smoke):** `fuzz_match_pattern` (pattern×text),
`fuzz_resolve` (template strings), `fuzz_pack_load` (YAML bytes → loader must error,
never panic). Parser-adjacent hand-written code is exactly where fuzzing pays.

**Snapshot (insta):** emitter — golden corpus of (features + packs) → artifacts +
sidecars, byte-stable (the canonical-format compatibility surface, ADR-0010);
diagnostics — rendered miette output for the seeded error corpus (every validation pass
in TECH-SPEC §4.1 has at least one golden failure); `proef schema` output; event-stream
JSONL for a reference run (with injected clock/run-id — core purity makes this
deterministic).

**Integration (fixture server):** synchronous `tiny_http` dev crate
(`proef-fixture`) modeled on the spike's fixture — *not* axum as originally
written: axum's tokio requirement conflicts with the workspace's no-async-runtime
ban (ADR-0006/0007 + `deny.toml`), and a sync fixture keeps that invariant
binary-wide (errata 2026-07-28, M3). Endpoints, extended: bearer-auth endpoints, search, create (201/422 paths), delayed
push-visibility (exercises `retry` for real), cookie-setting endpoints (exercises
SessionState round-trip), slow endpoint (exercises budgets/watchdog), malformed-JSON
endpoint. Suite covers: green path (the four 500-series features), capture chaining,
World/global across scenarios, `optional:` warn-and-continue, cancellation (token
cancel mid-run completes within budget, reports written), parallel `--jobs` determinism
(event Normalize), artifact↔execution same-bytes assertion (hash the emitted file and
the text handed to `parse_hurl_file`).

**Corpus:** `--dry-run` over every `.feature` in `tests/` —
the suite's own features are the regression corpus.

**CLI (assert_cmd):** exit codes 0/1/2/3 pinned per command and failure class;
`--output json` schema-checked; `--junit` well-formed (quick-junit round-parse).

**Canary (M4):** scheduled + on-release job builds against the *next* hurl version and
replays the integration suite; red = issue with behavior diff, pins never auto-move
(runbook: IMPLEMENTATION-PLAN §7).

## 2. What is deliberately NOT tested here

Hurl's own HTTP semantics (asserts, filters, templating execution) — that is upstream's
test surface; proef tests the *adapter contract* (options mapping, variable bridging,
span mapping, segmentation) against the fixture instead of re-verifying hurl. This is a
direct consequence of ADR-0001 and the reason the differential-oracle harness from the
research phase was retired.

## 3. CI matrix & gates

Linux (ubuntu-latest, prereqs pre-baked) + macOS on every PR; Windows weekly (vcpkg
libs) while the port stabilizes, then per-PR (port green 2026-07-28: VCPKG_ROOT export,
hurl's crates.io-missing icon supplied in CI, `/`-normalized path identifiers). Gates: fmt, clippy `-D warnings`, nextest (all crates),
doctests, rustdoc `-D warnings`, deny, cargo-machete, zizmor (workflow static
analysis), snapshot check (`insta test`), fuzz smoke
(30 s/target), corpus dry-run, CLI suite. Nightly: full fuzz (10 min/target), canary, cargo-audit (advisories against
unchanged code — deny covers PRs).
Coverage: `cargo llvm-cov` report published as PR comment (informational, no hard gate
pre-1.0).

## 4. Test data management

`tests/features/` — the real suite (also corpus input). `tests/errors/` — seeded broken
features/packs, one file per diagnostic code, name = expected code (golden snapshots).
Insta snapshots live next to their suites (`crates/proef-cli/tests/snapshots/`),
reviewed via `cargo insta review`. Fixture data is
generated in-process (no committed binary blobs beyond one JPEG for multipart, M5).

## 5. Determinism rules (make flakes structural, not cultural)

Core purity (no IO/clock/rand — TECH-SPEC §4) means every non-integration layer is
bit-deterministic by construction. Integration layer: fixture delays are token-driven
(visibility timestamps), not sleep-raced; retry tests assert attempt *counts*, wall time
only as generous upper bounds; parallel tests assert on Normalized event order, never
raw interleaving. Any test needing "now" receives it as a parameter.
