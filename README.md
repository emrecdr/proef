# proef

[![CI](https://github.com/emrecdr/proef/actions/workflows/ci.yml/badge.svg)](https://github.com/emrecdr/proef/actions/workflows/ci.yml)
[![windows](https://github.com/emrecdr/proef/actions/workflows/windows.yml/badge.svg)](https://github.com/emrecdr/proef/actions/workflows/windows.yml)
[![nightly](https://github.com/emrecdr/proef/actions/workflows/nightly.yml/badge.svg)](https://github.com/emrecdr/proef/actions/workflows/nightly.yml)
[![crates.io](https://img.shields.io/crates/v/proef)](https://crates.io/crates/proef)
[![Rust](https://img.shields.io/badge/rust-1.97-B7410E?logo=rust&logoColor=white)](rust-toolchain.toml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](LICENSE-MIT)
[![Gherkin](https://img.shields.io/badge/BDD-Gherkin-23D96C?logo=cucumber&logoColor=white)](https://cucumber.io/docs/gherkin/)
[![Hurl](https://img.shields.io/badge/engine-Hurl-FF0288)](https://hurl.dev)
[![E2E](https://img.shields.io/badge/testing-end--to--end-8A2BE2)](docs/README.md)

**proef** (Dutch: *test/trial* — and *tasting*) is a declarative, modular,
multi-engine end-to-end test runner. Tests are Gherkin `.feature` files in plain
business prose; YAML macro packs (with embedded raw [Hurl](https://hurl.dev) blocks)
bind that prose to executable steps; an engine-agnostic core batches the steps and
dispatches them through a stable engine seam. The engine embeds hurl **in-process**
for API testing.

## What it looks like

A scenario is authored for humans — this one is trimmed from
[`tests/features/500_api_note.feature`](tests/features/500_api_note.feature),
the repo's own example suite:

```gherkin
Feature: API — note sync
  Scenario: A note posted via the API appears on the board
    Given the workspace is provisioned
    And the activity channel is activated and ready
    When a member posts a note to the board
    Then the board shows the note
```

A macro pack ([`tests/features/packs/api.yaml`](tests/features/packs/api.yaml))
binds each sentence to real HTTP work — raw hurl, parse-validated at load. The
runner matches every sentence against the `match:` patterns of the loaded
packs, and a step's `hurl:` key names the engine that executes it:

```yaml
macros:
  provisionEnvironment:
    match: the workspace is provisioned
    steps:
      - name: provision the environment
        hurl: |
          POST ${url:base}/api/v1/env/provision
          Authorization: Bearer ${secret:apiToken}
          {"run": "${run:id}"}
          HTTP 201
          [Captures]
          envId: jsonpath "$.id"
  search:
    params: [term]
    match: the operator searches for {term}
    steps:
      - hurl: |
          GET ${url:base}/search
          [Query]
          q: ${term}
          HTTP 200
```

`search` binds a sentence with a placeholder: every `{capture}` in `match:` must
be a declared `params:` entry, and quoted arguments in the feature sentence
shed their quotes once captured.

Running it validates everything statically, executes against the embedded engine, and
leaves a complete record:

```console
$ proef test tests/features --jobs 4
running 12 scenario(s) with 4 job(s) — run 019f…

  Scenario: A note posted via the API appears on the board (tests/features/500_api_note.feature)
    ✓ tests/features/500_api_note.feature:11 — the workspace is provisioned (9ms)
    ✓ tests/features/500_api_note.feature:16 — the board shows the note … (312ms, 2 attempts)

summary: 12 passed · 0 failed · 0 skipped
```

## Why proef

- **Prose is the test.** Feature files stay readable by non-engineers; the binding
  lives in packs, not in the prose.
- **Artifacts are the contract.** Every scenario emits a canonical `.hurl` file that
  is *byte-identical* to what the engine executed (hash-asserted), replayable with
  stock `hurl --test` — plus sidecars mapping artifact lines back to feature lines.
- **One record.** The JSONL event stream *is* the run record: live progress,
  per-attempt retries, JUnit/GitHub summaries and `proef explain` all derive from it.
- **Deterministic by construction.** The core does no IO, reads no clocks or env, and
  generates no randomness — run ids, timestamps, and env are injected. Snapshots and
  property tests stay stable; `${fake:*}` data is seeded from the run id.
- **Secrets stay secret.** Encrypted at rest, injected through hurl's redaction,
  value-redacted once at the event-sink boundary, never persisted by captures —
  property-tested end to end. CI decrypts a committed store via `PROEF_KEY`.
- **Engine-agnostic core.** Adding an engine leaves `proef-core` diff-empty; that is
  the acceptance test for the seam.

## What proef deliberately isn't

These are settled non-goals, not gaps awaiting a contribution:

- **No importing or round-tripping hand-written hurl files.** Artifacts flow
  outward only: `proef artifacts` emits `.hurl` you can run with stock hurl,
  and nothing reads `.hurl` back in.
- **No API mocking or contract testing**, and no load testing.
- **No second engine.** The factory/session seam exists for dependency
  hygiene (ADR-0002), not as a roadmap.
- **No desktop dashboard or server mode**, and no dynamic plugin loading.

**Already have a hurl corpus?** The supported path is pasting your existing
request bodies into a pack's `steps[].hurl` blocks — they are raw hurl,
validated by the real parser at pack load, so they carry over unmodified.

## Installation

Pick whichever fits your machine:

```bash
# Homebrew (macOS or Linuxbrew, arm64 or x86_64):
brew install emrecdr/proef/proef

# Prebuilt binary via cargo-binstall (any Rust dev environment):
cargo binstall proef

# From source via crates.io:
cargo install proef --locked
```

Or grab a prebuilt archive from a [GitHub Release](https://github.com/emrecdr/proef/releases/latest)
— five targets ship per tag (macOS arm64/x86_64, Linux arm64/x86_64-gnu, Windows
x86_64-msvc), each with SLSA build provenance
(`gh attestation verify <archive> --owner emrecdr`). The Windows zip bundles the
libcurl/libxml2 DLLs the binary needs; Linux binaries expect the distro's
`libcurl4` and `libxml2` (present on virtually every system, or
`apt install libcurl4 libxml2`).

Building from source needs the native headers — Linux:
`apt install build-essential pkg-config libssl-dev libcurl4-openssl-dev libxml2-dev libclang-dev`;
macOS: Xcode Command Line Tools; Windows: vcpkg (see
`.github/workflows/windows.yml`). Verify any install with `proef doctor`.

## Quick start

Fastest path: `proef init` scaffolds a working suite (`proef.toml`, one
`.feature`, one pack) into the current directory and prints the next command —
no secret to store, since the scaffold's pack references none. Building one
by hand:

A suite is one directory; `proef test <dir>` discovers everything in it by two
conventions — no configuration points files at each other:

```
suite/                  # any name, anywhere — this is the path you pass to proef
  checkout.feature      # every *.feature under the tree is a test file…
  flows/
    refunds.feature     # …at any depth: feature discovery is recursive
  packs/                # every *.yaml|*.yml directly inside a `packs` directory
    api.yaml            # is a macro pack (the packs dir may sit at any depth)
```

All discovered packs — plus a small pack built into the binary — merge into one
vocabulary shared by every feature file, so any sentence may be bound by any
pack in the tree. [`tests/features/`](tests/features/) in this repo is a
complete working suite in exactly this shape.

```bash
mkdir -p suite/packs
# 1. write suite/case.feature (prose) and suite/packs/api.yaml (bindings);
#    every sentence needs a binding — extend the snippets above the same way,
#    or copy tests/features/ wholesale as a working start
# 2. store secrets the packs reference
proef secret set apiToken            # or: export PROEF_SECRET_APITOKEN=…
# 3. validate everything without touching the network
proef test suite --dry-run
# 4. run
proef test suite --jobs 4
```

Writing scenarios against a vocabulary somebody else maintains:
[`docs/WRITING-SCENARIOS.md`](docs/WRITING-SCENARIOS.md) · ten-minute
walkthrough: [`docs/GETTING-STARTED.md`](docs/GETTING-STARTED.md) · full
authoring reference: [`docs/AUTHORING.md`](docs/AUTHORING.md).

## CLI

| Command | Purpose |
|---|---|
| `proef init [dir]` | scaffold a minimal working suite — `proef.toml`, one feature, one pack, schema wired up; never overwrites an existing file |
| `proef test [path]` | validate + execute (`--env`, `--dry-run`, `--tags`, `--scenario`, `--scenario-file`, `--jobs`, `--junit`, `--output json`, `--watch`); path optional — defaults to `[run] suite`, then `tests/` |
| `proef flows [path]` | list scenarios with anchors and tags (`--env`, `--output json` feeds the nextest harness) |
| `proef macros [path]` | list every macro with its call count, flagging pattern macros no scenario binds (`--env`, `--output json`) |
| `proef artifacts [path] -o DIR` | emit canonical `.hurl` + sidecars (+ referenced file assets) for CI hand-off (`--env`) |
| `proef explain [run-id]` | summarize a run from its event record |
| `proef diff [base] [new]` | compare two run records — regressions, fixes, flakiness, perf deltas (`--fail-on-regression` for CI gating) |
| `proef report [run-id]` | write a self-contained HTML report for a run (`-o FILE`) |
| `proef schema` | print/install the pack JSON Schema (engine fragments included) |
| `proef secret set\|list\|rm` | encrypted secret store (names listed, values never) |
| `proef fmt <path>` | canonicalize raw hurl blocks inside packs (`--check` for CI) |
| `proef doctor` | native library, environment, and secret store/key checks |
| `proef lsp` | language server over stdio — diagnostics, go-to-definition, completion, references (see [`docs/EDITORS.md`](docs/EDITORS.md)) |

**Exit codes are a contract:** `0` ok · `1` test failure (incl. cancelled runs) ·
`2` user error · `3` system error.

## Configuration

`proef.toml` in the project root (see [`docs/CONFIG.md`](docs/CONFIG.md) /
`proef.toml.example`) holds runner settings **and** suite variables, so test files stay
pure prose:

- **Runner** — `[run]` (`jobs`, `runs-dir`, `suite` = the default test path) and
  `[http]` (`timeout-ms`, `follow-location`).
- **Variables** — `[url]` and `[vars]`, referenced in packs as `${url:base}` /
  `${vars:apiVersion}`. Secrets stay in the encrypted store (`${secret:…}`), never here.
- **Environments** — `[env.<name>.<section>]` deep-merges per-environment overrides over
  the base tables; `proef test --env prod` (or `PROEF_ENV`) selects one.

Precedence: defaults < `proef.toml` base < active `[env.<name>]` < flags (ADR-0012).
Secrets resolve `PROEF_SECRET_<NAME>` env overrides before the encrypted store.
Run records land under `.proef-runs/<run-id>/` (events.jsonl, run.log, artifacts;
200-run rotation); the persistent World lives in `.proef-state.json`.

## Workspace

```
crates/proef-core/         engine-agnostic front end, IR, emitter, orchestrator, events
crates/proef-engine-hurl/  the API engine over embedded hurl (pinned =8.0.1)
crates/proef-cli/          the `proef` binary
crates/proef-lsp/          language server over the sans-IO core (`proef lsp`)
crates/proef-fixture/      dev-only in-process fixture API server
crates/proef-harness/      one nextest/IDE test per scenario (libtest-mimic)
xtask/                     fixture runner, hurl-upgrade canary, automation
```

## Development

```bash
cargo nextest run          # all tests
cargo test --doc           # doctests (nextest doesn't run them)
just gates                 # every CI gate locally (fmt, clippy -D, tests, doc, deny, machete, docs-check)
just audit                 # security advisories (nightly in CI, on demand locally)
cargo insta test --review  # snapshot changes are reviewed, never blind-accepted
cargo run -p xtask -- fixture   # local fixture API server on :8787
cargo run -p xtask -- canary    # build+test against the next hurl release
```

Author-facing guides: [`docs/WRITING-SCENARIOS.md`](docs/WRITING-SCENARIOS.md)
(scenario authors), [`docs/GETTING-STARTED.md`](docs/GETTING-STARTED.md),
[`docs/AUTHORING.md`](docs/AUTHORING.md),
[`docs/TROUBLESHOOTING.md`](docs/TROUBLESHOOTING.md),
[`docs/CONFIG.md`](docs/CONFIG.md),
[`docs/DIAGNOSTICS.md`](docs/DIAGNOSTICS.md), and — for CI consumers of the
run record — [`docs/EVENTS.md`](docs/EVENTS.md).
The maintainer corpus lives in [`docs/`](docs/README.md): PRD, ADR decision log
(ADR-0001…0011), TECH-SPEC, IMPLEMENTATION-PLAN, TESTING-STRATEGY. Architectural
changes require a new ADR in the same PR.

## Releases

Releases follow [SemVer](https://semver.org) and are tagged; every release has a
[`CHANGELOG.md`](docs/CHANGELOG.md) entry. The policy and runbook live in
[`docs/RELEASING.md`](docs/RELEASING.md).

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT License](LICENSE-MIT) at your option. Unless you explicitly state otherwise,
any contribution intentionally submitted for inclusion in the work by you, as
defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
