# ADR-0011 — Fixture server is synchronous `tiny_http`, not axum

**Status:** Accepted · **Date:** 2026-07-28 (decision made at M3; recorded as an ADR
during the post-M5 hardening pass — it had been noted only as inline errata in
TECH-SPEC §14 and TESTING-STRATEGY §2)

## Context

TESTING-STRATEGY as originally written named **axum** for the integration fixture
server (`proef-fixture`). Axum requires a tokio runtime. The workspace bans the tokio
runtime outright (ADR-0006/ADR-0007: engines and core are sync; only `tokio-util` with
`default-features = false` enters the tree, for `CancellationToken`). A dev-dependency
would not leak into shipped binaries, but it *would* put a full multithreaded async
runtime into every `cargo nextest` process, contradict the "no async machinery" line we
enforce with `cargo deny` bans, and normalize exactly the dependency the ADRs exclude.

The fixture's needs are modest: a handful of JSON endpoints, per-env state isolation,
deterministic token-driven delayed visibility, cookies, one deliberately slow route and
one deliberately malformed one — all exercised by at most a dozen concurrent scenarios.

## Decision

`proef-fixture` is built on **`tiny_http`** (synchronous, dependency-light) with a
plain `std::thread` accept loop and an `Arc<Mutex<_>>` state map keyed by the
`X-Proef-Env` header. It starts on an ephemeral port (`Server::http("127.0.0.1:0")`),
reports its base URL, and shuts down via an `AtomicBool` + `recv_timeout` poll.
Portability note: address introspection uses `ListenAddr::to_ip()` — the `Unix`
variant of `ListenAddr` exists only on unix targets, so matching on it breaks the
Windows build.

## Consequences

- The workspace stays runtime-free end to end; the deny-list ban on tokio stands
  without a dev-dependency exception.
- The fixture is one file, debuggable with a thread dump, and starts in microseconds —
  each integration test spawns its own isolated instance.
- No HTTP/2, no TLS, no streaming in the fixture — acceptable: the engine's HTTP
  behavior is hurl/libcurl's concern, not the fixture's; the fixture only scripts
  responses.

## Alternatives considered

- **axum (as originally specced):** rejected — drags in the banned tokio runtime;
  every capability the fixture needs is available synchronously.
- **hyper in blocking mode / raw `std::net`:** more code for no additional fidelity.
- **Out-of-process fixture binary:** slower startup, port coordination, and a
  lifetime-management problem tests would have to solve; in-process `tiny_http` gives
  free isolation per test.
