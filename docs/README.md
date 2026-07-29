# proef — documentation index

**proef** (Dutch: *test, trial* — and *tasting*) is a declarative, modular, multi-engine
end-to-end test runner, mostly Rust. Tests are Gherkin `.feature` files in business prose;
macro packs bind the prose to executable steps; a pluggable engine runs each step batch —
**embedded Hurl for API testing** (the seam admits future engines; none are scheduled).

This folder is the project corpus: the product requirements, the decision log, the
normative technical spec, the milestone plan, and the testing strategy. Written
2026-07-28 from a validated research round (a working spike ran 5/5 scenarios green
under both a prototype native runner and stock hurl 8.0.1 on identical generated
artifacts); implementation has since delivered milestones M0–M5 — the plan and the
repo-root `CLAUDE.md` carry the live status.

## Reading order

| # | Document | What it answers | Audience |
|---|---|---|---|
| 0 | [GETTING-STARTED.md](GETTING-STARTED.md) | Your first suite in ten minutes | test authors |
| 0 | [AUTHORING.md](AUTHORING.md) | The pack/feature reference from the author's seat | test authors |
| 0 | [TROUBLESHOOTING.md](TROUBLESHOOTING.md) | Exit codes, glyphs, frequent failures, digging into runs | test authors |
| 0 | [CONFIG.md](CONFIG.md) | Every `proef.toml` key with defaults | test authors |
| 0 | [DIAGNOSTICS.md](DIAGNOSTICS.md) | The greppable index of every diagnostic code | test authors |
| 0 | [EVENTS.md](EVENTS.md) | The `events.jsonl` wire schema for CI consumers | CI engineers |
| 1 | [PRD.md](PRD.md) | What are we building, for whom, and how do we know it works? | everyone |
| 2 | [adr/](adr/) — ADR-0001…0011 | Why is it built this way? Each decision, alternatives, consequences | engineers |
| 3 | [TECH-SPEC.md](TECH-SPEC.md) | How exactly is it built? Types, pipeline, schemas, verified seam facts | implementers |
| 4 | [IMPLEMENTATION-PLAN.md](IMPLEMENTATION-PLAN.md) | In what order, with what acceptance criteria? M0–M6 task breakdown, risks, runbooks | implementers |
| 5 | [TESTING-STRATEGY.md](TESTING-STRATEGY.md) | How is every layer verified? | implementers |
| — | [RELEASING.md](RELEASING.md) | Versioning policy and the release runbook | maintainers |
| — | [../CONTRIBUTING.md](../CONTRIBUTING.md) | Setup, gates, and the rules that are easy to trip over | contributors |
| — | [../SECURITY.md](../SECURITY.md) | Threat model and vulnerability reporting | everyone |
| — | [../CLAUDE.md](../CLAUDE.md) | Repo-root guidance for Claude Code: constraints, seam facts, commands, status | coding agents |

## Decision log (ADR index)

| ADR | Decision | Status |
|---|---|---|
| [0001](adr/ADR-0001-embed-hurl-as-api-engine.md) | Embed hurl's crates in-process as the API engine | Accepted |
| [0002](adr/ADR-0002-multi-engine-architecture.md) | Multi-engine core: `EngineFactory`/`EngineSession` seam, step-kind routing, batching | Accepted |
| [0003](adr/ADR-0003-upstream-tracking-thin-fork.md) | Exact pins + thin zero-diff fork as patch vehicle + upgrade canary | Accepted |
| [0004](adr/ADR-0004-pack-format-yaml-plus-raw-hurl.md) | Packs = YAML skeleton + embedded raw Hurl blocks | Accepted |
| [0005](adr/ADR-0005-two-tier-variables-and-world.md) | `${…}` author-time / `{{…}}` run-time variables; World; secrets | Accepted |
| [0006](adr/ADR-0006-sync-dyn-traits.md) | Engine traits are sync + dyn; no async machinery in v1 | Accepted |
| [0007](adr/ADR-0007-cancellation-model.md) | Cooperative cancellation at batch boundaries + budgets (hurl has none) | Accepted |
| [0008](adr/ADR-0008-event-spine-and-reporters.md) | Serde event enum = run record; decorator reporter stack; libtest-mimic mode | Accepted |
| [0009](adr/ADR-0009-error-taxonomy-exit-codes.md) | User/TestFailure/System → exit 2/1/3; miette at the CLI edge | Accepted |
| [0010](adr/ADR-0010-artifacts-as-contract.md) | Emitted `.hurl` artifacts are the executed input (same bytes) + sidecars | Accepted |
| [0011](adr/ADR-0011-fixture-server-tiny-http.md) | Fixture server is synchronous `tiny_http`, not axum (tokio-runtime ban) | Accepted |

## Naming & identifiers

Project/binary **`proef`** · crates **`proef-core`**, **`proef-engine-hurl`**, **`proef-cli`**,
**`proef-fixture`**, **`proef-harness`**
· run records **`.proef-runs/`** · persistent
World **`.proef-state.json`** · config **`proef.toml`**. The crates.io names
`proef`, `proef-core`, and `proef-engine-hurl` are published and owned.

## Provenance

Grounded in two verified sources: (1) **hurl
master** (Orange-OpenSource) — source-level verification of every library seam used, with
file:line citations in TECH-SPEC §5; (2) a **working spike** proving the front end and
artifact contract end to end (the spike predates this repo). Ecosystem practices are drawn from
cargo-nextest, cucumber-rs, sqlx, rustls, probe-rs, and current (2026) Rust guidance.
