# Testing Patterns — proef

> Grounded in `docs/TESTING-STRATEGY.md` (normative) and `CLAUDE.md` (Testing
> expectations). Those sources WIN on any conflict. Read by `tester`, `programmer`,
> `code-reviewer`, and `verifier`.

Everything is **device-free and network-free except the fixture-server integration
suite**. No test merges without covering the new behavior (definition of done).

## The layers

- **Unit** (every crate): matcher tokenization/matching edges; resolver escapes + depth
  cap; Then-merge (`expect:` → previous entry) rules; batch segmentation boundaries;
  sidecar math; World error → exit-code mapping. Unit tests live in
  `#[cfg(test)] mod tests` at the bottom of the source file — never a separate file.
- **Property (proptest):** matcher never panics on arbitrary pattern×text and round-trips
  captures; resolver `$${…}` escape round-trips, is idempotent once resolved, and always
  terminates at the depth cap; **secret-mask invariant** — for arbitrary events/reports
  containing a known secret, rendered output never contains it (ADR-0005); World
  snapshot/restore is an involution.
- **Fuzz (cargo-fuzz):** `fuzz_match_pattern`, `fuzz_resolve`, `fuzz_pack_load` (YAML bytes
  → the loader must error, never panic). Smoke in PR CI, full nightly.
- **Snapshot (insta):** emitter golden corpus (features+packs → artifacts+sidecars,
  byte-stable — the canonical-format surface, ADR-0010); rendered miette diagnostics for
  the seeded error corpus; `proef schema` output; event-stream JSONL for a reference run.
- **Integration (fixture server):** the sync `tiny_http` `proef-fixture` crate (ADR-0011 —
  axum's tokio requirement conflicts with the runtime ban). Covers the green path (the
  reference-corpus features), capture chaining, World/global across scenarios, `optional:`
  warn-and-continue, cancellation-within-budget, parallel `--jobs` determinism, and the
  artifact↔execution same-bytes assertion.
- **Corpus:** `--dry-run` over every `.feature` in `tests/` — the suite's own features are
  the regression corpus.
- **CLI (assert_cmd):** exit codes `0/1/2/3` pinned per command and failure class;
  `--output json` schema-checked; `--junit` round-parsed with quick-junit.

## Test data

- `tests/features/` — the real suite (also corpus input).
- `tests/errors/` — one directory per diagnostic code, name = the expected code; dry-running
  it **fails by design**. A new diagnostic code adds a case here (where reachable) plus a
  `docs/DIAGNOSTICS.md` row.
- insta snapshots live next to their suites (e.g. `crates/proef-cli/tests/snapshots/`),
  reviewed via `cargo insta review` — never blind-accepted.

## Determinism & the flake rule (structural, not cultural)

Core purity (no IO/clock/rand — inject `now`/`run_id`/env) makes every non-integration
layer bit-deterministic. In the integration layer: fixture delays are **token-driven**
(visibility timestamps), not sleep-raced. **Assert attempt counts** and **normalized event
order** — wall-clock only as a generous upper bound, never raw interleaving. Any test
needing "now" receives it as a parameter.

## What is deliberately NOT tested here

Hurl's own HTTP semantics (asserts, filters, templating execution) — that is upstream's
surface (ADR-0001). proef tests the **adapter contract** (options mapping, variable
bridging, span mapping, segmentation) against the fixture, not hurl's behavior.

## Running tests

```bash
cargo nextest run                             # all tests (preferred; skips doctests)
cargo nextest run -p proef-core <substring>   # one crate / one test
cargo test --doc                              # doctests
cargo insta test --review                     # snapshots
cargo run -p xtask -- fixture                 # standalone fixture server for the integration suite
```

## Anti-patterns

- Weakening or deleting a test/gate/assertion to make a run pass — fix the code (see
  `golden-rules.md` §14). Snapshot/property/exit-code suites are load-bearing.
- Wall-clock or raw-interleaving assertions in parallel/retry tests.
- Blind `cargo insta accept` — every snapshot diff is justified.
- New `#[serde(untagged)]` enum carrying numbers in test fixtures (arbitrary_precision
  hazard). Time-dependent tests using `SystemTime::now()` instead of an injected value.
