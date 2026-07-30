# Review Checklist — proef

> Grounded in `CLAUDE.md` (Hard constraints) and `docs/` (ADRs, TECH-SPEC). Those sources
> WIN on any conflict. The `code-reviewer` reads this alongside `coding-standards.md` and
> `golden-rules.md`.

Severity: **CRITICAL** blocks merge · **HIGH** requires revision unless explicitly accepted
· **MEDIUM** requires justification · **LOW** auto-fix.

## CRITICAL — proef invariants

- [ ] **hurl pins bumped** (`=8.0.1` / `hurl_core`) outside the canary + runbook — reject
      (ADR-0003).
- [ ] **`proef-core` does IO / reads clock / env / randomness** — breaks determinism;
      values must be injected (ADR-0005, TECH-SPEC §4).
- [ ] **`proef-core` imports an engine or engine-specific type** — the seam is one-way
      (engines → core).
- [ ] **miette used outside `proef-cli`** — typed errors in core/engines, miette only at
      the edge (ADR-0009).
- [ ] **Secret reaches an artifact, event, log, report, or the persistent World** — or
      `saveAs: global` accepts a secret-valued capture (ADR-0005). Redaction is
      property-tested; keep it green.
- [ ] **Emitted `.hurl` bytes ≠ bytes handed to `parse_hurl_file`** — artifacts are the
      executed input (ADR-0010).
- [ ] **New `#[serde(untagged)]` enum carrying numbers** — arbitrary_precision breaks it;
      use hand-rolled scalar visitors.
- [ ] **Another engine's vocabulary introduced** (web/CDP, adb/tablet, browser) — hurl is
      the only engine (`[[hurl-engine-only]]`).

## CRITICAL — soundness & errors

- [ ] `unsafe` block without a `// SAFETY:` comment; `unsafe fn` without a `# Safety`
      section (rare in proef — scrutinize any new `unsafe`).
- [ ] `unwrap()`/`expect()` in a library path (`proef-core`, engines) outside proven
      invariants — CLI `main` excepted.
- [ ] **Swallowed error**: `let _ = result;` without warning or classification into a
      fault. Poisoned `Mutex` recovered wrongly (recover via `PoisonError::into_inner` only
      when no cross-invariant was broken; else System fault).
- [ ] `Box<dyn Error>` in a library public API — callers can't match variants.

## HIGH — contracts & schema

- [ ] **Event schema change that is not additive**, or a second run-record format invented
      alongside the JSONL stream (ADR-0008).
- [ ] **Exit-code mapping changed** without updating the assert_cmd suite (`0/1/2/3`,
      ADR-0009).
- [ ] **Snapshot diff (artifacts/sidecars/diagnostics/events) blind-accepted** — each must
      be `insta review`ed and justified.
- [ ] **`${…}` vs `{{…}}` confused**: `${…}` resolves at lower time; `{{…}}` must pass
      through core untouched (ADR-0005).
- [ ] **`retry:` without a finite count** — the finite-retry lint exists for a reason
      (ADR-0007).
- [ ] **`proef-core` public API widened** without regenerating `public-api.txt`
      deliberately.
- [ ] **Architectural change without a new/superseding ADR** in the same PR; `CLAUDE.md`
      Status not updated.
- [ ] **New diagnostic code** without a `docs/DIAGNOSTICS.md` row + `tests/errors/` case
      where reachable.

## HIGH — dependencies & idioms

- [ ] Banned dep introduced: `reqwest`, `async-trait`/`maybe-async`, a tokio runtime,
      `chrono` (ours), `serde_yaml`/`serde_yml`. `notify` not `=8.2.0`.
- [ ] `WriteMode::Immediate` in a library path (must be `Buffered` — interleaves under
      threads).
- [ ] Diagnostics: `LineCol.column` (char-counted) used in byte math; span not treated as
      0-based byte offset.
- [ ] Bare primitives where a newtype carries identity (`StepKindId`, `EngineId`, …);
      excessive `.clone()` where borrowing works.

## MEDIUM — quality, docs, tests

- [ ] Public item missing rustdoc (`#![warn(missing_docs)]` is on); `# Errors`/`# Panics`
      missing where needed; broken intra-doc link.
- [ ] New behavior without unit + (where applicable) snapshot/property coverage.
- [ ] Parallel/retry test asserting wall-clock or raw interleaving instead of attempt
      counts + normalized event order.
- [ ] A gate/test/assertion weakened to pass (see `golden-rules.md` §14).
- [ ] Comment narrates the code instead of stating a constraint the code can't show.

## LOW — style

- [ ] `cargo fmt --all --check` clean · `cargo clippy … -D warnings` clean.
- [ ] No `#[allow(...)]` without a `// Why:` justification.

## Diagnostic commands

```bash
cargo clippy --all-targets --all-features -- -D warnings
cargo run -p proef -- test tests/features --dry-run    # bind+lower+emit+parse
cargo run -p proef -- test tests/errors --dry-run      # must fail by design
cargo insta test --review                              # snapshot review
cargo run -p xtask -- public-api                       # core surface drift
```
