# Common Code Smells — proef

> Grounded in `CLAUDE.md` (Hard constraints) and `docs/` (ADRs, TECH-SPEC). Those sources
> WIN on any conflict. Anti-patterns to detect and fix during review and development.
> Each entry: the smell, why it bites, the fix.

## Bumping the hurl pins in a normal change
`hurl`/`hurl_core` moved off `=8.0.1` in a feature/fix PR. The crates break API in minor
releases and the seam is verified against exactly this version (TECH-SPEC §5). **Fix:**
revert; upgrades go only through the canary + runbook (ADR-0003).

## IO / clock / env / rand inside `proef-core`
`std::fs`, `SystemTime::now()`, `std::env::var`, or an RNG in core destroys the determinism
snapshots and property tests depend on (TECH-SPEC §4). **Fix:** inject the value
(`run_id`, `now`, env snapshot) from the caller.
Detect: `grep -rn "SystemTime::now\|std::env::var\|std::fs::" crates/proef-core/src`.

## A new `#[serde(untagged)]` enum carrying numbers
hurl feature-unifies `serde_json/arbitrary_precision` workspace-wide; numbers then flow
through a private token map that breaks untagged numeric variants. **Fix:** hand-rolled
scalar visitors (see `proef_core::world::Value`); internally-tagged enums are fine.
`value_json_forms_round_trip` pins it.

## Secret material escaping into an artifact / event / log / World
A secret value in `.hurl`, a sidecar, an event, a report, or `.proef-state.json`; or
`saveAs: global` accepting a secret-valued capture. Violates the property-tested redaction
invariant (ADR-0005). **Fix:** route secrets through `insert_secret`; emit
`{{secret_name}}` placeholders (names only); refuse (warn) a secret-valued `saveAs: global`.

## Confusing `${…}` with `{{…}}`
Core code resolving `{{…}}`, or leaving `${…}` for run time. `${…}` is lower-time (core,
recursive depth ≤ 8, `$${` escape); `{{…}}` is hurl run-time and passes through core
untouched (ADR-0005). **Fix:** resolve only `${…}` in core; forward `{{…}}` verbatim.

## Blind-accepting an insta snapshot
`cargo insta accept` (or committing `.snap.new`) without reading the diff. Artifact bytes,
sidecars, diagnostics, and event streams are the contract (ADR-0010); a silent change ships
a format break. **Fix:** `cargo insta review` each diff and say why it changed.

## Emitting `.hurl` text that differs from what gets parsed
Post-processing the emitted `.hurl` before `parse_hurl_file` (or vice-versa). The two must
be identical bytes, hash-asserted (ADR-0010). **Fix:** emit once; parse and execute the
same bytes.

## Banned dependency creeping in
`reqwest`, `async-trait`, `maybe-async`, a tokio **runtime**, `chrono` (ours),
`serde_yaml`/`serde_yml`, or `notify` off `=8.2.0`. Each is banned for a documented reason
(superseded / ADR-0006 sync-dyn traits / runtime ban / archived-or-bad-fork / prerelease).
**Fix:** hurl engine for HTTP; sync dyn traits; `tokio-util` `CancellationToken` only;
`jiff`; `serde_norway`.
Detect: `grep -rn "reqwest\|async-trait\|maybe-async\|serde_yaml\|serde_yml\| chrono" Cargo.toml crates/*/Cargo.toml`.

## Another engine's vocabulary
web/CDP, adb/tablet, browser, or any non-hurl engine terms in types, names, docs, or
examples. hurl is the only engine; the seam is architectural readiness, nothing more
(`[[hurl-engine-only]]`, ADR-0002). **Fix:** keep everything hurl-shaped — the structural
test is that adding an engine leaves `proef-core` diff-empty.

## `WriteMode::Immediate` in a library path
Interleaves output under scenario-per-thread execution. **Fix:** always
`WriteMode::Buffered` in library paths (TECH-SPEC §5); `Immediate` is only for the CLI's own
stdout path.

## `LineCol.column` in byte math
Adding `LineCol.column` to a byte offset when building a `SourceSpan`. gherkin spans are
0-based **byte** offsets; `LineCol.column` is char-counted (TECH-SPEC §9). **Fix:** use byte
offsets; attach the normalized (trailing-newline) source; clamp.

## `retry:` without a finite count
hurl allows infinite retries and has no cancellation, so an unbounded step means a runaway
scenario (ADR-0007). **Fix:** finite `count` — the finite-retry lint enforces it; budgets +
watchdog are the backstop.

## `unwrap()` / `expect()` in a library path
In `proef-core` or an engine (outside a proven invariant) a panic crashes a scenario thread
instead of mapping to an exit code. **Fix:** propagate via `?` into a typed error; CLI
`main` is the only excepted path.
Detect: `grep -rn "\.unwrap()\|\.expect(" crates/proef-core/src`.

## Silently swallowing an error
`let _ = result;` on a fallible call with no comment makes a failure that should classify
into a fault (→ exit code) disappear. **Fix:** warn, or map into the fault taxonomy
(ADR-0009). Poisoned `Mutex`: recover via `PoisonError::into_inner` only when no
cross-invariant was broken, else a System fault.

## Weakening a test to make the run pass
Deleting/skipping/loosening a test, snapshot, property, or the exit-code suite ships the gap
as missing or buggy behavior; count-diffing catches silent drops. **Fix:** fix the code. If
a test is genuinely wrong, change it visibly and say why.

## Comment that narrates instead of constrains
A comment restating the next line, the signature, or change history is noise the moment the
PR merges. **Fix:** comment only to state a constraint the code can't show — a verified seam
fact, an invariant a reviewer must keep.
