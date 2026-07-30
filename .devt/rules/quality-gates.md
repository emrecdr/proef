# Quality Gates — proef

> Grounded in `CLAUDE.md` (Common commands), `docs/CONTRIBUTING.md`, `docs/TECH-SPEC.md` §15,
> and `docs/TESTING-STRATEGY.md` §3. Those sources WIN on any conflict. Read by the
> `tester`, `code-reviewer`, and `verifier` agents.

Each gate is a deterministic command. Run the exact command; a gate passes only on exit 0
with clean output. A gate that emits warnings without erroring is NOT a pass — report
`WARNINGS` and get explicit acknowledgement.

## The PR gate stack (all must be green)

```bash
cargo nextest run                                                          # 1. all tests (preferred runner)
cargo test --doc                                                           # 2. doctests (nextest skips them)
cargo clippy --all-targets --all-features -- -D warnings                   # 3. lints (warnings = errors)
cargo fmt --all --check                                                    # 4. formatting
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace   # 5. docs (broken links + missing docs = errors)
cargo deny check                                                           # 6. license / bans / advisories / sources
cargo machete                                                              # 7. unused dependencies
cargo run -p xtask -- docs-check                                           # 8. corpus indexes ↔ reality
cargo run -p xtask -- public-api                                           # 9. proef-core surface (nightly rustdoc)
```

`just` carries aliases for the common ones (`just gates`). CI additionally runs `zizmor`
(workflow static analysis), a **fuzz smoke** (~30 s/target), the corpus `--dry-run`, the
assert_cmd CLI suite, and the **hurl canary**. `cargo audit` runs on the **nightly**
schedule (deny covers advisories on PRs).

## proef-specific validation gates

```bash
cargo insta test --review                                # snapshots: artifacts, sidecars, diagnostics, event streams
cargo run -p proef -- test tests/features --dry-run      # bind + lower + emit + parse-validate, no network
cargo run -p proef -- test tests/errors --dry-run        # seeded broken corpus — MUST fail (one dir per diagnostic code)
cargo run -p proef -- doctor                             # native libs / env checks
```

- **Snapshots are deliberate** (ADR-0010): `cargo insta review` each diff and justify it.
  Never blind-accept. Artifact bytes and the text handed to `parse_hurl_file` must be
  identical (hash-asserted in tests).
- **`tests/errors/` is expected to fail** on dry-run — that is the point. Each directory
  name is the diagnostic code it exercises. A new diagnostic code adds a case here (where
  reachable) and a `docs/DIAGNOSTICS.md` row.
- **`proef-core` public API** is snapshot-locked; an intended change regenerates
  `crates/proef-core/public-api.txt` via `PROEF_PUBLIC_API_UPDATE=1 cargo run -p xtask -- public-api`.

## Fuzz & property targets

Property tests (proptest) cover the matcher, resolver, secret-mask invariant, and World.
Fuzz targets (`fuzz_match_pattern`, `fuzz_resolve`, `fuzz_pack_load`) run as a PR smoke and
fully nightly. Keep the secret-mask invariant green — rendered output must never contain a
known secret value (ADR-0005).

## Single-crate / single-test iteration

```bash
cargo nextest run -p proef-core <substring>   # one crate / one test
cargo run -p xtask -- fixture                 # local fixture API server for the integration suite
cargo run -p xtask -- canary                  # build+test against the next hurl release
```

## Determinism (why gates stay green)

Core purity (no IO/clock/rand) makes every non-integration layer bit-deterministic. The
integration suite is the only network user, and it is token-driven. Flake rule: assert
attempt **counts** and **normalized** event order — never wall-clock or raw interleaving.

## What counts as pass

| Gate | Pass criteria |
|---|---|
| `cargo nextest run` | exit 0, no failures |
| `cargo test --doc` | exit 0, doc-tests ok |
| `cargo clippy … -D warnings` | exit 0 |
| `cargo fmt --all --check` | exit 0 (no diff) |
| `cargo doc` (with `-D warnings`) | exit 0 |
| `cargo deny check` / `cargo machete` | exit 0, no violations |
| `xtask docs-check` / `xtask public-api` | exit 0 (no drift) |
| `insta test` | exit 0, no pending snapshots |
| `test tests/features --dry-run` | exit 0 |
| `test tests/errors --dry-run` | non-zero (fails by design) |
