# Golden Rules — proef

> Grounded in this repo's `CLAUDE.md` "Hard constraints" and `docs/` (ADRs + TECH-SPEC).
> Those sources WIN on any conflict. Read by every agent — these are non-negotiable.

These encode invariants that break proef (subtly, at a distance) when violated. Get one
wrong and the seam, the artifacts, or the secret guarantees fail.

## 1. hurl pins are exact and sacred

`hurl = "=8.0.1"` and `hurl_core = "=8.0.1"`, built `--locked`. NEVER bump them in a
normal change — the crates break API in minor releases. Upgrades go only through the
canary + runbook (ADR-0003, IMPLEMENTATION-PLAN §7); fork patches ride
`[patch."crates-io"]` and get PR'd upstream. A pin bump in a feature/fix diff is a bug.

## 2. proef-core is pure (sans-IO lite)

`proef-core` does no IO, reads no clock/env, and generates no randomness. `run_id`,
timestamps, and env snapshots are **injected values**. This is what makes snapshots and
property tests deterministic. Do not reach for `std::fs`, `SystemTime::now()`,
`std::env::var`, or an RNG in core for convenience — thread the value in instead.

## 3. hurl is the only engine

proef tests APIs by embedding hurl — that is the whole product. The `EngineFactory`/
`EngineSession` seam (ADR-0002) is architectural readiness, not a scheduled feature.
Never introduce another engine's vocabulary (web/CDP, adb/tablet, browser, …) anywhere:
not in types, names, docs, or examples. See `[[hurl-engine-only]]`.

## 4. Two-tier variables, and secrets never leak (ADR-0005)

`${…}` resolves at **lower time** (recursive, depth ≤ 8, `$${` escapes). `{{…}}` is
**hurl run-time** and must pass through core untouched. External config variables
`${url:key}` / `${vars:key}` are lower-time too — sourced from `proef.toml` `[url]`/`[vars]`
deep-merged with the active `[env.<name>]` (`--env`/`PROEF_ENV`) and injected into the
sans-IO core as `LowerCtx::config_vars`; the CLI does the file IO, not core (ADR-0012).
Secrets go through `VariableSet::insert_secret` and never appear in artifacts, events,
logs, reports, or the persistent World. `saveAs: global` must refuse a secret-valued
capture. This is a property-tested invariant — keep it green.

## 5. Artifacts are the executed input (ADR-0010)

The emitted `.hurl` text and the bytes handed to `parse_hurl_file` must be identical
(hash-asserted). The canonical format is snapshot-locked. Emitter changes require a
deliberate `cargo insta review` — never blind-accept a snapshot diff.

## 6. Events are an additive-only versioned schema (ADR-0008)

The serde `Event` enum carries a `schema` field; changes are additive-only. The JSONL run
record IS the event stream — do not invent a second record format.

## 7. Exit codes are a contract (ADR-0009)

`0` ok · `1` test failure · `2` user error · `3` system error — a typed enum pinned by
assert_cmd tests. Error variants map through the fault taxonomy. Only `proef-cli` uses
miette; `proef-core` and engines return typed errors.

## 8. Never add an untagged serde enum carrying numbers

hurl feature-unifies `serde_json/arbitrary_precision` into every build, which breaks
`#[serde(untagged)]` enums on numeric variants. Write hand-rolled visitors for scalars
(see `proef_core::world::Value`); internally-tagged enums survive, untagged-with-numbers
do not. `value_json_forms_round_trip` pins this.

## 9. Banned dependencies

No `reqwest` (superseded by the embedded engine), no `async-trait`/`maybe-async`
(ADR-0006 — traits are sync + dyn), no tokio **runtime** (only `tokio-util` with
`default-features = false` for `CancellationToken`). Datetime is `jiff`, never `chrono`
(in our code). YAML is `serde_norway`, never `serde_yaml`/`serde_yml`. `notify` pinned
`=8.2.0`.

## 10. Diagnostics use byte offsets

gherkin `Span` = 0-based **byte** offsets, end-exclusive → miette `SourceSpan`. The
parser appends a trailing newline when missing (normalize/clamp). `LineCol.column` is
char-counted — never use it in byte math.

## 11. No `unwrap`/`expect` in library paths

Library code (`proef-core`, engines) propagates via `?` and typed errors; CLI `main` is
the only excepted path. Never silently swallow an error — `let _ = result;` is banned:
warn or classify it (map to a fault). Poisoned `Mutex`: recover via
`PoisonError::into_inner` when no cross-invariant was broken, else surface a System fault.

## 12. Always `WriteMode::Buffered` in library paths

`Immediate` interleaves output under threads. Scenario-per-thread execution relies on
buffered writes.

## 13. A new architectural decision needs a new ADR in the same PR

Add `docs/adr/ADR-00NN-*.md` (next number, same format) whenever you diverge from or
extend a decision. Diverging from an ADR without a superseding ADR is a bug. Keep the
`CLAUDE.md` Status list current as milestones land.

## 14. Never weaken a test, gate, or assertion to make a run pass

Fix the code, not the test. Snapshots, property invariants, the error corpus, and the
exit-code suite are load-bearing. If a test is genuinely wrong, change it visibly and say
why — silent drops are caught by diffing test counts against the baseline.

## 15. One way to do one thing

Every capability has exactly **one** canonical implementation, command, code path, and
format — never two mechanisms that reach the same outcome. This is the single-seam
discipline generalized: one engine seam (ADR-0002), one run record (ADR-0008), one
artifact format (ADR-0010), one mock backend (`proef-fixture`, ADR-0011), one config
precedence chain. When a second way appears — a duplicate helper, a parallel command, a
second server, an alternate on-disk format — **unify onto one and delete the other in the
same change**; never leave both. New redundancy is a defect even when both paths work:
it doubles the surface that can drift, and drift between two "equivalent" paths is a bug
generator. Prefer generalizing the one mechanism over adding a special-case second.
