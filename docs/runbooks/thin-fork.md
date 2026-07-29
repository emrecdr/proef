# Runbook — thin-fork patching (ADR-0003 tier 2)

Rehearsed 2026-07-28 (M4). The steady state carries **zero diff**; this runbook
is the mechanics for the moment a release breaks the seam or a small change is
needed before upstream accepts it.

## The drill (as rehearsed, local-path variant)

1. Obtain the pinned source (fork tag in the real flow; the vendored registry
   copy suffices for a drill):

   ```bash
   cp -R ~/.cargo/registry/src/index.crates.io-*/hurl-8.0.1 /tmp/hurl-fork
   # apply the minimal patch (one commit on the fork branch in the real flow)
   ```

2. Wire the override at the **workspace root** (`Cargo.toml`):

   ```toml
   [patch.crates-io]
   hurl = { path = "/tmp/hurl-fork" }               # drill
   # hurl = { git = "https://github.com/<org>/hurl", tag = "8.0.1-proef.1" }  # real
   ```

3. Verify cargo resolves the fork, then build and run the full suite:

   ```bash
   cargo tree -p proef-engine-hurl | grep "hurl v"   # must show the path/git source
   cargo build -p proef-engine-hurl && cargo nextest run
   ```

4. Revert: remove the `[patch.crates-io]` block, `git checkout Cargo.lock`,
   confirm `cargo tree` shows the registry source again.

Rehearsal result: override resolved, engine + suite built green against the
patched path, revert restored registry resolution. Elapsed ≈ one hurl rebuild.

## The real flow (when a patch is actually needed)

1. Fork `Orange-OpenSource/hurl`; branch `proef-patches-<version>` off the
   release tag; apply the **minimal** diff (one commit per logical patch).
2. Tag `X.Y.Z-proef.N`; consume via the git `[patch.crates-io]` form above.
3. Open the corresponding upstream PR immediately (tier 3 — the fork's diff
   must trend back to zero); note the patch in ADR-0003's log.
4. On each upstream release: rebase the branch onto the new tag, drop merged
   patches, re-run the canary, move the pins per IMPLEMENTATION-PLAN §7.

## Currently drafted patches

- `docs/upstream/0001-run-entries-reusable-client.patch` — `run_entries`
  accepts `&mut Client` (verified two-call-site change; erases per-segment
  connection + cookie costs, deletes proef's `SessionState` cookie
  round-trip once adopted). Applies cleanly to 8.0.1 and compiles
  (verified in the M4 drill). PR text: `docs/upstream/0001-PR-DESCRIPTION.md`.
