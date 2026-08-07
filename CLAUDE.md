# CLAUDE.md

This file guides Claude Code when working in this repository. It is the entry point, not
the spec — the authoritative corpus lives in `docs/` and wins on any conflict.

## What this is

**proef** (Dutch: *test/trial* — and *tasting*) is a declarative, modular, multi-engine
end-to-end test runner, mostly Rust. Tests are Gherkin `.feature` files in business
prose; YAML macro packs (with embedded raw Hurl blocks) bind prose to executable steps;
an engine-agnostic core dispatches step batches to pluggable engines. The first engine
embeds **hurl** in-process for API testing — the only engine; the seam is the
sanctioned extension point (architectural readiness only, nothing scheduled).

## Read before large changes

1. `docs/README.md` — corpus index and ADR decision log.
2. The relevant **ADR** (`docs/adr/ADR-0001` onward) — decisions with alternatives and
   consequences. Diverging from an ADR without writing a superseding ADR is a bug.
3. `docs/TECH-SPEC.md` — normative types, pipeline, pack schema, and the **verified
   hurl seam facts with file:line citations** (§5). Do not re-derive these from priors.
4. `docs/IMPLEMENTATION-PLAN.md` — the milestone you are implementing, its acceptance
   criteria, and the global definition of done.
5. `docs/PRD.md` — the user stories (US-1…US-12) that milestone acceptance criteria
   cite, and the product scope boundaries.

New architectural decision → add `docs/adr/ADR-00NN-*.md` (next number, same format)
in the same PR. Keep the **Status** section at the bottom of this file current as
milestones land.

## Toolchain (pinned)

Always **latest stable Rust** (1.97.1 at writing), pinned via `rust-toolchain.toml`;
edition 2024; workspace `resolver = "3"`. Tools to install once:
`cargo install cargo-nextest cargo-deny cargo-audit cargo-insta just`
(plus `cargo-fuzz` for fuzz targets, `cargo-llvm-cov` for coverage).

## Common commands

```bash
cargo build                                   # whole workspace
cargo nextest run                             # all tests (preferred)
cargo nextest run -p proef-core <substring>   # one crate / one test
cargo test --doc                              # doctests (nextest doesn't run them)
cargo insta test --review                     # snapshot tests (emitter/diagnostics/events)

cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo deny check && cargo audit
# fuzz/ is its own workspace (root `exclude`), so nothing above compiles it —
# a changed proef-core signature breaks the fuzz targets with every gate green:
cargo check --manifest-path fuzz/Cargo.toml --all-targets

cargo run -p proef -- test tests/features --dry-run  # validate: bind + lower + emit + parse
cargo run -p proef -- test tests/features --jobs 4   # execute (secrets: store via `proef secret set`, or PROEF_SECRET_<NAME> env)
cargo run -p proef -- test --env staging             # path-less (default [run] suite / tests/) + a proef.toml [env.<name>] profile
cargo run -p proef -- artifacts tests/features -o out/ --run-id ci  # emit .hurl + sidecars
cargo run -p proef -- flows tests/features           # list scenarios with anchors + tags
# tests/errors/ is the seeded broken corpus (one dir per diagnostic code) — dry-running it fails by design
cargo run -p proef -- doctor                   # native libs / env checks
cargo run -p xtask -- fixture                      # local fixture API server on :8787 (dev; falls back if busy)
cargo run -p xtask -- canary                       # build+test against next hurl release
```

Build prerequisites (Linux): `apt install build-essential pkg-config libssl-dev
libcurl4-openssl-dev libxml2-dev libclang-dev`. macOS: Xcode CLT suffices. Only
`proef-engine-hurl` needs these; `proef-core` is pure Rust.

## Workspace architecture

```
crates/
  proef-core/         engine-agnostic: gherkin parse, packs, binding, lowering, IR,
                      emit, dispatch, World/state, events, errors, reporters
    helpers/          built-in macro packs (embedded at build time)
  proef-engine-hurl/  the API engine: EngineFactory/EngineSession over embedded hurl
  proef-cli/          bin `proef`: clap, engine registry assembly, miette rendering
  proef-fixture/      dev-only: in-process sync fixture API server (tiny_http, ADR-0011)
  proef-harness/      libtest-mimic bridge: one Trial per scenario (US-12)
  proef-lsp/          language server: SourceProvider + collect-all analyze_suite over core
xtask/                automation as Rust (fixture, canary, docs-check, public-api); `just` = thin aliases
```

**The central seam** (ADR-0002): `EngineFactory` (id, `step_kinds()` pack-schema
contribution, `doctor()`, `open`) + `EngineSession` (`run_batch`, `finish`), both sync,
used as `Box<dyn …>`. Routing: a macro step's kind names its engine (`hurl:` →
engine-hurl; other kind prefixes reserved — ADR-0002 errata). The core batches **contiguous same-engine
steps** and dispatches in order; the **World** (typed vars + persistent global store)
threads captures between batches and engines. Registry lives in `proef-cli` (one line
per engine, cargo-feature-gated). **Dependency rules:** engines depend on core; core
depends on no engine and no engine-specific type; only `proef-cli` uses miette; engines
never import each other. The structural acceptance test: *adding an engine leaves
`proef-core` diff-empty*.

## Hard constraints (these override training priors — get them wrong and it breaks)

- **hurl pins are exact and sacred:** `hurl = "=8.0.1"`, `hurl_core = "=8.0.1"`, built
  `--locked`. NEVER bump casually — the crates break API in minor releases. Upgrades go
  through the canary + runbook (IMPLEMENTATION-PLAN §7); patches go on the thin fork via
  `[patch."crates-io"]` and get PR'd upstream (ADR-0003).
- **Verified hurl seam facts** (TECH-SPEC §5 has file:line): `run_entries` builds its
  HTTP client *per call* → **batch maximally**; on forced splits chain
  `HurlResult.variables` and round-trip cookies via Netscape temp file. hurl has **no
  cancellation** and allows infinite retries → finite-retry pack lint + batch budgets +
  watchdog (ADR-0007). Per-entry `[Options]` override batch `RunnerOptions`
  (clone-then-override). Always `WriteMode::Buffered` in library paths — `Immediate`
  interleaves under threads.
- **hurl feature-unifies `serde_json/arbitrary_precision` into every workspace build**
  (discovered at M0): numbers then reach serde's content-buffered paths as a private
  token map, which **breaks `#[serde(untagged)]` enums** on numeric variants
  (internally-tagged enums survive). Write hand-rolled visitors for scalar types
  (see `proef_core::world::Value`) and never add new untagged enums that carry
  numbers; `value_json_forms_round_trip` pins the behavior.
- **YAML = `serde_norway`**, never `serde_yaml` (archived) or `serde_yml` (bad fork).
  **Datetime = `jiff`** in our code, never chrono (hurl's internal chrono is its
  business). **`notify` pinned 8.2.0** (9.x is a prerelease). **Banned:** reqwest
  (superseded by the embedded engine), `async-trait`/`maybe-async` (ADR-0006),
  tokio runtime (only `tokio-util` with `default-features = false` for
  `CancellationToken`). **Also banned — raw print macros in `proef-cli`:**
  `println!`/`eprintln!` panic when the write fails, so a closed pipe
  (`proef … | head`) aborts with 101, outside the typed exit contract; use
  `crate::render::outln!` / `errln!`, which a source-scanning test enforces.
- **Core purity (sans-IO lite):** `proef-core` does no IO, reads no clocks/env, and
  generates no randomness — `run_id`, timestamps, and env snapshots are injected values.
  This is what makes snapshots/properties deterministic; do not break it for
  convenience.
- **Two-tier variables (ADR-0005):** `${…}` resolves at lower time (recursive, depth ≤ 8,
  `$${` escape — captured step args may themselves contain `${…}`); `{{…}}` is hurl
  run-time and must pass through core untouched. External config variables
  `${url:key}` / `${vars:key}` come from `proef.toml` (`[url]`/`[vars]`, deep-merged with
  the active `[env.<name>]` via `--env`/`PROEF_ENV`), injected into the sans-IO core as
  `LowerCtx::config_vars` — the CLI does the file IO, not core (ADR-0012). Secrets go through
  `VariableSet::insert_secret` and **never** appear in artifacts, events, logs,
  reports, or the persistent World — `saveAs: global` refuses secret-valued
  captures (property-tested invariant — keep it green; events carry capture
  *names* only).
- **Artifacts are the executed input (ADR-0010):** the emitted `.hurl` text and the text
  handed to `parse_hurl_file` must be the *same bytes* (hash-asserted in tests). The
  canonical format is snapshot-locked — emitter changes require deliberate
  `cargo insta review`, never blind acceptance.
- **Diagnostics:** gherkin `Span` = 0-based **byte** offsets (end-exclusive) → miette
  `SourceSpan`; the parser appends a trailing newline when missing (normalize/clamp);
  `LineCol.column` is char-counted — never use it in byte math.
- **Events (ADR-0008):** the serde `Event` enum is a versioned schema (`schema` field);
  changes are additive-only; the JSONL run record *is* the event stream — no second
  record format.
- **Exit codes are a contract:** `0` ok · `1` test failure · `2` user error · `3` system
  error — typed enum, pinned by assert_cmd integration tests; error variants map via the
  fault taxonomy (ADR-0009).
- **Pack rules (ADR-0004):** the pack root key is `macros:` (renamed from `templates:`,
  ADR-0004 amendment — no alias); hurl steps use raw `hurl:` block scalars (validated by
  `parse_hurl_file` at load); structured payloads are reserved for future engines;
  `expect:` macros merge asserts into the *previous* request entry; `retry:` must be
  finite; `use:` nesting is cycle-checked, depth ≤ 32.

## Testing expectations (`docs/TESTING-STRATEGY.md` is normative)

Every layer device-free and network-free except the fixture-server integration suite.
Unit + proptest (matcher, resolver, secret-mask, World) + cargo-fuzz (matcher, resolver,
pack loader; smoke in PR CI, full nightly) + insta snapshots (artifacts, diagnostics,
schema, event streams) + fixture integration (green path = the reference-corpus features;
retry/cookies/optional/World/cancellation cases) + `--dry-run` corpus over `tests/` +
assert_cmd CLI/exit-code suite + the hurl upgrade canary. Flake rule: assert attempt
counts and normalized event order, never wall-clock or raw interleaving.

## Status (update as milestones land)

- [x] M0 — foundations: workspace, gates, seam traits, error/exit model, doctor
- [x] M1 — front end: packs + validation, matcher, gherkin, lowering, `--dry-run`
- [x] M2 — IR, canonical emitter, sidecars, `artifacts`, snapshot corpus
- [x] M3 — engine-hurl execution: adapter, World bridge, parallelism, budgets, console+JSONL
- [x] M4 — upstream tracking: real canary, thin-fork rehearsal, upstream PR #1, JUnit/GH summary
- [x] M5 — breadth: bodies (multipart/form/docstring), watch, explain, secrets CLI, fakes, libtest-mimic harness, `proef fmt`
- [x] post-M5 — external config & environments (`proef.toml` `[url]`/`[vars]`/`[env.<name>]`, `${url:}`/`${vars:}`, `--env`/`PROEF_ENV`, ADR-0012); default suite path (`[run] suite`); pack root key `templates:` → `macros:` (ADR-0004 amendment); dev fixture binds default port 8787 + versioned `/health` (ADR-0011 amendment)
- [ ] M6 — future engines (none scheduled; acceptance: zero `proef-core` diff)

Milestone detail, acceptance criteria, and the definition of done: `docs/IMPLEMENTATION-PLAN.md`.
