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
output never contains it, **nor any of its derived encoded forms** (base64 both
alphabets ± padding, hex both cases, percent-encoding, JSON-string escape —
ADR-0005 as amended), with a companion property pinning that text free of the
secret and its forms passes through untouched; World — snapshot/restore is an involution;
**fragment scanner** (`proef-engine-hurl`) — over generated hurl files, every
reported line lies inside the file, entries are accounted for exactly once, starts
are ordered and distinct, and no fragment's text runs into the entry after it.
That last one is the entry-boundary arithmetic's whole job, and it is asserted
because a draft without it passed while the boundary was deliberately broken.
The scanner is proptested rather than fuzzed on purpose: it needs `hurl_core`, and
cargo dependencies are package-level, so putting it in `fuzz/` would compile hurl
for every target there and drag native libraries into a job that has none.

**Fuzz (cargo-fuzz, nightly job + PR smoke):** `fuzz_match_pattern` (pattern×text),
`fuzz_resolve` (template strings), `fuzz_tag_expr` (tag expressions),
`fuzz_pack_load` (YAML bytes → loader must error, never panic), and
`fuzz_fragment_binding` (a pack against a real corpus: `ref:` resolution, unread
`bind:` keys, a `bind:` colliding with a variable the fragment supplies itself).
Parser-adjacent hand-written code is exactly where fuzzing pays.

**Both loops take their target list from `cargo fuzz list`**, never a list written
into a workflow. The names used to be spelled out in `ci.yml` *and* `nightly.yml`,
so a target ran nowhere until both were edited and nothing failed to say so.

`fuzz_fragment_binding` is **structure-aware** — it builds a well-formed pack and
corpus from the input rather than hoping the fuzzer discovers one. That is a
measured choice: a byte-oriented version never resolved a single `ref:` in 1.45
million runs, because reaching those rules means finding valid YAML and a matching
corpus name at once. When adding a target that needs structure, verify it *reaches*
the code by probe — panic on the condition under test, run briefly, confirm it
fires — because a target that compiles and finds nothing reads exactly like a
target that compiles and finds no bugs.

`fuzz/` is its own workspace (the root `Cargo.toml` excludes it, since fuzzing
needs nightly), so no root-workspace command compiles it: a changed
`proef-core` signature breaks the targets while every *root-workspace* gate
stays green, leaving the fuzz jobs as the only signal.
`cargo check --manifest-path fuzz/Cargo.toml --all-targets` runs on the pinned
stable toolchain in seconds, so the gates job carries it — earlier than the
fuzz smoke, and on both gate platforms.

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

**Fragments (ADR-0018):** `crates/proef-cli/tests/fragments.rs` builds a **self-contained
project per test** — its own `proef.toml`, corpus and pack in a temp dir — because the
reference corpus under `tests/` is config-independent by design (several tests run it from
a temp cwd with settings passed by environment variable and no `proef.toml` in scope), so
anything needing `[run] fragments` cannot live there. That is also why four diagnostic
codes are covered here rather than in `tests/errors/` (DIAGNOSTICS.md says which).

The headline case runs **one file under both runners**: `proef test` against the fixture,
then stock `hurl` invoked on the same bytes with an equivalent variables file, asserting
the corpus comes back byte-identical. The engine is embedded, so a `hurl` binary is not a
build requirement — that half skips with a printed note when none is on `PATH` rather than
being faked. Provenance is asserted at both ends: the JSONL record for the event-driven
readers, and `--junit` for the `RunSummary`-driven ones, since those are fed by a second
copy that a green suite would not otherwise exercise.

**LSP over stdio (`crates/proef-cli/tests/lsp_stdio.rs`):** the real binary, spoken to as
an editor does. This is the only place `proef.toml` → `DiskSourceProvider` → document URI
is exercised end to end: the `proef-lsp` unit tests inject absolute source names through a
fake provider, so a config-layer change can break every go-to-definition while they stay
green — which has happened. These tests **canonicalize their temp root**, because on macOS
a tempdir is `/var/…` whose real path is `/private/var/…`, and without that any
cwd-relative path logic silently no-ops and the test passes without reaching the behaviour.

**Corpus:** `--dry-run` over every `.feature` in `tests/` —
the suite's own features are the regression corpus.

**Documentation (`xtask docs-check` + `crates/proef-cli/tests/docs.rs`):** the docs make
claims a machine can settle, so they are settled mechanically rather than by review.
`docs-check` reads files, and does six things: every workspace crate appears in
TECH-SPEC §2 and CLAUDE.md; every ADR file appears in the decision log; every diagnostic
code the workspace emits has a `DIAGNOSTICS.md` row and vice versa; every release names
each kind of change **once** (repeats accumulate by appending, which is how a changelog
gets written); every relative link resolves; and every fenced `toml`/`yaml` example parses
**with the product's own parsers**, so the check means "proef would accept this", not
"some parser would". `tests/docs.rs` needs a built binary and therefore lives
with `assert_cmd`: it asks clap whether every documented command and long flag exists.

The split is a rule, not an accident, and each half states it in its own header — a check
that reads files belongs in `docs-check` even when a test would be easier to write, because
the doc-only CI step is the fast one and a file-reading check placed in the test suite
silently stops running there.

Both were written against defects that had already shipped — an ADR whose first example
could not load, and a row marked *shipped* that named a `--html` flag which never
existed. In each the surrounding prose was correct, which is precisely what a careful
reader does not catch. Two scoping rules keep them honest rather than noisy: only the
**indexed** corpus is linted (`docs/superpowers/` is a dated archive, and editing history
to satisfy a checker is the wrong direction), and command detection is restricted to
**code spans and fenced blocks** — prose says "proef discovers packs", and treating that
as an invocation produced sixty false positives against four real ones. Names the docs
discuss as *proposals* are listed explicitly in `tests/docs.rs`, so adding one is a
decision rather than the check going quietly soft.

**CLI (assert_cmd):** exit codes 0/1/2/3 pinned per command and failure class;
`--format json` schema-checked; `--junit` well-formed (quick-junit round-parse).

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
analysis), `xtask docs-check`, `proef doctor` smoke, public-api snapshot, fuzz smoke
(30 s/target), corpus dry-run, CLI suite, and — on a pull request — the changelog-entry
check (§8). The complexity ratios run as their own step, alone, for the reason §7 gives. Snapshot tests (`insta`) run inside nextest —
a drifted snapshot fails there, no separate step. Nightly: full fuzz (10 min/target),
canary, cargo-audit (advisories against unchanged code — deny covers PRs).
Coverage: **not measured in CI today** (filed as P13). A `cargo llvm-cov` PR comment
remains the intended shape when it lands — informational, no hard gate pre-1.0; this
paragraph previously described it as already running.

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

## 6. Every diagnostic code is named by a test

`DIAGNOSTICS.md` calls codes "a contract: they never change meaning". A contract
with nothing holding it to it is a wish — 23 of 75 were in that state when the
rule was written: reachable in production, documented, and exercised by nothing
at all, not even an assertion on their message text.

`source_guards.rs` enforces it. A code counts as covered when either a seeded
`tests/errors/<area>__<name>/` directory exists (the corpus driver dry-runs it,
so the *rendered* diagnostic is exercised end to end) or the literal code string
appears in a test. **Naming the code, not matching the prose** — the wording is
expected to improve, while the code is the part that promises not to change.

Two codes are exempted by name with recorded reasons (`source::unreadable`,
`config::unreadable` need a file the process may stat but not read, which CI
runners do not reproduce because they run as root). The guard checks its own
exemption list too: an exemption that outlives its code silently excuses
nothing.

Reaching a defensive guard is worth the effort rather than a reason to skip it.
`lower::kind_unrouted` fires only when the engine registry and pack validation
disagree, so its test makes them disagree; `lower::expansion_too_deep` sits
behind pack validation's identical limit, so its test bypasses validation with
`load_collecting` — the only way to hand lowering a graph validation would have
stopped, and therefore the only way to prove the second line of defence still
works.

## 7. Complexity claims are asserted as ratios, never as benchmarks

A published performance claim is a claim like any other, and this project has now
watched four separate ones decay in prose. The guard for a *shape* claim — "linear
in the macro count", "~2× per doubling" — is a **ratio between two input sizes**,
not a stopwatch against a threshold:

- A ratio tests what was actually promised. The claim is a shape; a shape is a ratio.
- The separation is wide enough to be safe. `validation_cost_stays_linear_in_the_macro_count`
  observes ~2.05× against a bound of 3.0; restoring the pre-#138 quadratic shape
  measures 4.01×. Take the **minimum** of several interleaved samples — scheduler
  noise only ever adds, so the fastest observation is the closest to the work
  actually done — and assert the smaller load was slow enough to time at all, or
  the ratio is meaningless.

**A timing test runs alone, or it does not run.** These are `#[ignore]`d and have
their own CI step and `just perf`; nothing else shares the machine. The first
version of this section claimed the opposite — that a ratio "survives a shared
runner" because load inflates both sides and cancels — and shipped a test that
failed on its second full-suite run. Measurement: **2.05× alone, 3.09× under
nextest's full parallelism.** The larger input has the larger working set, so
memory-bandwidth contention penalises it more; the ratio drifts rather than
cancelling, and interleaving cannot fix a systematic effect. nextest's
`test-groups` bound concurrency *within* a group, which does not isolate one
from the rest of the suite — so `#[ignore]` plus a dedicated invocation is the
only mechanism that actually delivers isolation.

Benchmark frameworks were considered and are deliberately absent. `iai-callgrind`
is the right tool for gating in CI, because instruction counts ignore runner noise
entirely — but it needs valgrind, making it a gate the maintainer cannot reproduce
on macOS. `criterion` and `divan` measure wall time, which is the same noise regime
as the ratio test while also adding a dependency tree to a workspace that audits
every edge.

## 8. A change that lands records itself

`RELEASING.md` states that every landed change adds an `[Unreleased]` line in the
commit series that lands it. Nothing enforced that, and the rule was broken exactly
once — by the series that added the guard for the changelog's *shape*. A rule whose
only enforcement is a sentence in another document is a rule with a known decay rate,
which is the same finding this suite keeps re-deriving.

So a pull-request job asks one question: did any `crates/**` or `xtask/**` `.rs` file
change without `docs/CHANGELOG.md` changing too? If so it fails, naming the files and
quoting the rule. `[no changelog]` in the PR title waives it.

**It was sized before it was written**, because a gate with a high false-positive rate
trains people to reach for the waiver and is then worse than nothing. Across the 21
source-touching merges preceding it the rule would have fired *once* — on the one
commit that actually broke it. Pure-test and pure-performance changes all carried an
entry already, so "source changed" tracks "worth recording" closely here. That is a
measurement of this repository's habits rather than a general law, and the waiver
exists for where it stops holding.

The check reads a diff rather than files, so it is neither a `docs-check` task nor a
`tests/docs.rs` test — it lives in the workflow, which is the only place the base
commit is known. The PR title reaches it through `env`, never interpolated into the
shell body: a title is attacker-controlled text, and `${{ … }}` inside `run:` is a
template injection that `zizmor` flags.
