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

## Pin-bump checklist — hurl 8.1 watch items (recorded 2026-08-16)

Behavior changes already on hurl master that the canary **cannot see** —
compile-and-test stays green while a guarantee shifts. Work each of these when
the pins move to 8.1 (IMPLEMENTATION-PLAN §7); each names the fixture to add.

The structural reason these need a list at all: the fragment scanner matches
`OptionKind` **narrowly** (`if let OptionKind::Variable` — one arm, by design,
so a semver-allowed variant addition is not a compile break). New `[Options]`
in a fragment file are therefore silently *accepted*, not flagged. That is the
right default for options with local effect, and exactly wrong for the first
item below.

- **`variables-file:` (upstream #2021) — check the sandbox before accepting
  it.** Upstream opens the named file with a raw `File::open` against process
  CWD, no `ContextDir` confinement. A fragment corpus is *foreign by design*,
  so a corpus file saying `variables-file: ../../secrets.env` would read a
  file outside the project on proef's behalf. On bump: decide refuse-or-flag
  (`option_declared_twice`'s family machinery fits), and add a fixture — a
  `.hurl` with an escaping `variables-file:` must not silently read the target.
- **Cross-host cookie strip (upstream #5118, landed)** — a redirect across
  hosts stops forwarding cookies. Fixture: a fixture-server redirect pair
  asserting which cookies arrive, so the behavior flip shows up as a diff in
  *our* suite rather than as a user's broken auth flow.
- **`--file-root` resolution change (upstream #2830, watch)** — multipart
  asset paths may move from CWD-relative to hurl-file-relative. This is
  proef's multipart seam: artifacts are emitted to a different directory than
  the pack they came from, so relative asset paths are exactly the bytes that
  would change meaning. Fixture: a multipart scenario whose asset path only
  resolves under one of the two rules.
- **`Value::Duration` (upstream #3519, watch)** — a captured duration
  re-rendered into a template may change its string form. Fixture: capture a
  duration-typed value, splice it into a later request, snapshot the bytes.
- **New `[Options]` variants generally** (`no-header`,
  `http2-prior-knowledge`, `fail-with-body`, `no-jsonpath-coercion`, …): the
  scanner accepts them silently (above). On bump, sweep the new variants once
  and sort each into "local effect, fine" or "needs the `variables-file:`
  treatment".

## Currently drafted patches

- `docs/upstream/0001-run-entries-reusable-client.patch` — `run_entries`
  accepts `&mut Client` (verified two-call-site change; erases per-segment
  connection + cookie costs, deletes proef's `SessionState` cookie
  round-trip once adopted). Applies cleanly to 8.0.1 and compiles
  (verified in the M4 drill). PR text: `docs/upstream/0001-PR-DESCRIPTION.md`.
