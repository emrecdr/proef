# Coding Standards — proef

> Grounded in `CLAUDE.md` (Toolchain, Hard constraints) and `docs/TECH-SPEC.md` §14–15.
> Those sources WIN on any conflict. Read by `code-review-guide`, `complexity-assessment`,
> and `architect`.

## Toolchain

- **Always latest stable Rust** (1.97.1 at writing), pinned via `rust-toolchain.toml`.
- **Edition 2024**, workspace **`resolver = "3"`**, virtual workspace with
  `workspace.{package,dependencies,lints}` inherited by members.
- Workspace lints table is strict (clippy all=warn + a curated pedantic slice) and is
  applied verbatim — do not loosen it per-crate without justification.

## Dependencies — what to reach for, what is banned

- YAML: **`serde_norway`** (never `serde_yaml` archived, never `serde_yml` bad fork).
- Datetime in our code: **`jiff`** (never `chrono` — hurl's internal chrono is its own
  business). Note: datetime does not yet enter our code; the rule stands for when it does.
- Concurrency: `tokio-util` with `default-features = false` for `CancellationToken` only.
  **No tokio runtime.** Traits are sync + dyn — **no `async-trait`/`maybe-async`**
  (ADR-0006).
- **No `reqwest`** — the embedded hurl engine supersedes it.
- `notify` pinned **`=8.2.0`** (9.x is a prerelease).
- hurl / hurl_core pinned **`=8.0.1`**, `--locked` — never bump outside the canary
  (ADR-0003). New deps: prefer std; a new crate for trivial functionality is a smell.

## Error handling

- Library crates (`proef-core`, engines) define typed errors with `thiserror`; every
  public variant captures its source (`#[from]`/`#[source]`). Never `Box<dyn Error>` in a
  library public API.
- Only **`proef-cli`** uses miette (rendering at the edge — ADR-0009). Errors map to the
  fault taxonomy → exit codes `0/1/2/3`.
- **Never silently swallow an error.** `let _ = result;` is banned: warn, or classify it
  into a fault. Poisoned `Mutex` → recover via `PoisonError::into_inner` when no
  cross-invariant was violated, else surface a System fault.
- `unwrap()`/`expect()` are banned outside `tests`, CLI `main`, and proven-invariant
  scopes. Every `expect("…")` states the invariant.

## Core purity

`proef-core` performs no IO and reads no clock/env/randomness. `run_id`, timestamps, and
env snapshots are injected. Do not add `std::fs`, `SystemTime::now()`, `std::env::var`, or
an RNG to core — thread the value in from the caller.

## Serde hazards

hurl feature-unifies `serde_json/arbitrary_precision` into every build. **Never add a new
`#[serde(untagged)]` enum that carries numbers** — arbitrary_precision routes numbers
through a private token map that breaks untagged numeric variants. Use hand-rolled scalar
visitors (see `proef_core::world::Value`); internally-tagged enums are fine.

## Variables & secrets discipline (ADR-0005)

`${…}` is lower-time (resolved in core, recursive depth ≤ 8, `$${` escape). `{{…}}` is
hurl run-time — pass it through core untouched, never resolve it. External config
variables `${url:key}` / `${vars:key}` are lower-time too, injected as
`LowerCtx::config_vars` from `proef.toml` (`[url]`/`[vars]` deep-merged with `[env.<name>]`
via `--env`/`PROEF_ENV`) — the CLI reads the file, core stays sans-IO (ADR-0012). Secrets
flow through `insert_secret` and must never reach artifacts, events, logs, reports, or the
persistent World.

## Naming & modules

- `snake_case` items/modules/files; `PascalCase` types/traits; `SCREAMING_SNAKE_CASE`
  consts. Newtypes for identity (`StepKindId`, `EngineId`, `StepRef`), not bare primitives.
- Prefer `<name>.rs` over `<name>/mod.rs`. Unit tests in `#[cfg(test)] mod tests` at the
  bottom of the source file — never a separate file. Integration tests in `tests/`.
- Minimize `pub`; default private, `pub(crate)` across sibling modules. `proef-core`'s
  public surface is snapshot-locked (`crates/proef-core/public-api.txt`).

## Engine-hurl specifics

Always `WriteMode::Buffered` in library paths (`Immediate` interleaves under threads).
Batch maximally — `run_entries` builds its client per call. Per-entry `[Options]` override
batch `RunnerOptions` by clone-then-override.

## Comments & docs

- Comments state a constraint the code can't show (a verified seam fact, an invariant a
  reviewer must preserve) — not narration of what the next line does.
- `#![warn(missing_docs)]` on library crates; public items get a doc comment. The
  `RUSTDOCFLAGS="-D warnings" cargo doc` gate makes broken intra-doc links and missing docs
  errors.

## Formatting & lint gates

`cargo fmt --all --check` and `cargo clippy --all-targets --all-features -- -D warnings`
must pass. Any `#[allow(...)]` carries a `// Why:` justification.
