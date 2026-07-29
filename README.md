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

A scenario is authored for humans:

```gherkin
Feature: API — message sync
  Scenario: A message sent via the API appears in the client feed
    Given the client environment is provisioned
    And the client feed is activated and ready
    When the relative sends a message to the client
    Then the client feed shows the message from the relative
```

A pack binds each sentence to real HTTP work — raw hurl, parse-validated at load:

```yaml
templates:
  provisionEnvironment:
    match: the client environment is provisioned
    steps:
      - name: provision the environment
        hurl: |
          POST ${baseURL}/api/v1/env/provision
          Authorization: Bearer ${secret:apiToken}
          {"run": "${run:id}"}
          HTTP 201
          [Captures]
          envId: jsonpath "$.id"
```

Running it validates everything statically, executes against the embedded engine, and
leaves a complete record:

```console
$ proef test tests/features --jobs 4
running 12 scenario(s) with 4 job(s) — run 019f…

  Scenario: A message sent via the API appears in the client feed (tests/features/500_api_message.feature)
    ✓ tests/features/500_api_message.feature:11 — the client environment is provisioned (9ms)
    ✓ tests/features/500_api_message.feature:15 — the client feed shows the message … (312ms, 2 attempts)

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
  value-redacted once at the event-sink boundary — property-tested end to end.
- **Engine-agnostic core.** Adding an engine leaves `proef-core` diff-empty; that is
  the acceptance test for the seam.

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

Ten-minute walkthrough: [`docs/GETTING-STARTED.md`](docs/GETTING-STARTED.md) ·
full authoring reference: [`docs/AUTHORING.md`](docs/AUTHORING.md).

```bash
mkdir -p suite/packs
# 1. write suite/case.feature (prose) and suite/packs/api.yaml (bindings)
# 2. store secrets the packs reference
proef secret set apiToken            # or: export PROEF_SECRET_APITOKEN=…
# 3. validate everything without touching the network
proef test suite --dry-run
# 4. run
proef test suite --jobs 4
```

## CLI

| Command | Purpose |
|---|---|
| `proef test <path>` | validate + execute (`--dry-run`, `--tags`, `--scenario`, `--jobs`, `--junit`, `--output json`, `--watch`) |
| `proef flows <path>` | list scenarios with anchors and tags (`--output json` feeds the nextest harness) |
| `proef artifacts <path> -o DIR` | emit canonical `.hurl` + sidecars (+ referenced file assets) for CI hand-off |
| `proef explain [run-id]` | summarize a run from its event record |
| `proef schema` | print/install the pack JSON Schema (engine fragments included) |
| `proef secret set\|list` | encrypted secret store (names listed, values never) |
| `proef fmt <path>` | canonicalize raw hurl blocks inside packs (`--check` for CI) |
| `proef doctor` | native library / environment checks per engine |

**Exit codes are a contract:** `0` ok · `1` test failure (incl. cancelled runs) ·
`2` user error · `3` system error.

## Configuration

`proef.toml` in the project root (see `proef.toml.example`): `[run] jobs`, `runs-dir`,
`[http] timeout-ms`, `follow-location`. Precedence: defaults < `proef.toml` < flags.
Secrets resolve `PROEF_SECRET_<NAME>` env overrides before the encrypted store.
Run records land under `.proef-runs/<run-id>/` (events.jsonl, run.log, artifacts;
200-run rotation); the persistent World lives in `.proef-state.json`.

## Workspace

```
crates/proef-core/         engine-agnostic front end, IR, emitter, orchestrator, events
crates/proef-engine-hurl/  the API engine over embedded hurl (pinned =8.0.1)
crates/proef-cli/          the `proef` binary
crates/proef-fixture/      dev-only in-process fixture API server
crates/proef-harness/      one nextest/IDE test per scenario (libtest-mimic)
xtask/                     fixture runner, hurl-upgrade canary, automation
```

## Development

```bash
cargo nextest run          # all tests
cargo test --doc           # doctests (nextest doesn't run them)
just gates                 # every CI gate locally (fmt, clippy -D, tests, doc, deny, audit)
cargo insta test --review  # snapshot changes are reviewed, never blind-accepted
cargo run -p xtask -- fixture   # local fixture API server
cargo run -p xtask -- canary    # build+test against the next hurl release
```

Author-facing guides: [`docs/GETTING-STARTED.md`](docs/GETTING-STARTED.md) and
[`docs/AUTHORING.md`](docs/AUTHORING.md).
The maintainer corpus lives in [`docs/`](docs/README.md): PRD, ADR decision log
(ADR-0001…0011), TECH-SPEC, IMPLEMENTATION-PLAN, TESTING-STRATEGY. Architectural
changes require a new ADR in the same PR.

## Releases

Releases follow [SemVer](https://semver.org) and are tagged; every release has a
[`CHANGELOG.md`](CHANGELOG.md) entry. The policy and runbook live in
[`docs/RELEASING.md`](docs/RELEASING.md).

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT License](LICENSE-MIT) at your option. Unless you explicitly state otherwise,
any contribution intentionally submitted for inclusion in the work by you, as
defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
