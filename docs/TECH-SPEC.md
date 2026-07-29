# proef — Technical Specification

**Status:** normative for M0–M5 · **Date:** 2026-07-28 · decisions referenced as ADR-XXXX.
Verified upstream facts cite hurl master @ `03fcb84c` (2026-07-27) as `file:line`.

## 1. System overview

```
 .feature files          macro packs (YAML + raw hurl blocks)
      │                        │
      ▼                        ▼
 ┌─────────────────────────────────────────┐
 │ proef-core (pure: no IO, no clock, no   │
 │ rand — values injected)                 │
 │  parse ─ bind ─ lower ─┬─ emit          │
 │  (gherkin) (matcher)   │  (.hurl +      │
 │                        │   sidecars)    │
 │  dispatch: contiguous  │                │
 │  same-engine batches ──┼── events ──────┼──► reporter stack
 └────────────┬───────────┴────────────────┘    (console/JUnit/JSONL/GH)
              ▼ Box<dyn EngineSession>
   ┌──────────────────┐  ┌──────────────┐  ┌──────────────┐
   │ proef-engine-hurl│  │ engine-web   │  │ engine-adb   │
   │ parse_hurl_file +│  │ (future, CDP)│  │ (future, adb)│
   │ run_entries      │  └──────────────┘  └──────────────┘
   └──────────────────┘
        World (typed vars + persistent global store) threads through every batch
```

Scenario = unit of isolation, parallelism, retry, and artifact emission. The orchestrator
(in core, driven by cli) owns threads, the token, the World, and the event stream.

## 2. Workspace

```
proef/
  rust-toolchain.toml      # channel "1.97.1" (policy: always latest stable), rustfmt+clippy
  Cargo.toml               # virtual workspace, resolver="3", workspace.{package,dependencies,lints}
  deny.toml  .config/nextest.toml  proef.toml.example
  .github/workflows/ci.yml           # fmt, clippy -D warnings, nextest, doc -D warnings,
                                     # deny, audit, canary (ADR-0003)
  xtask/                   # automation as Rust (dist, fixture codegen, canary driver); just aliases
  crates/
    proef-core/            # engine-agnostic: gherkin parse, packs, binding, lowering, IR,
      helpers/             #   emit, dispatch, World/state, events, errors, reporters
    proef-engine-hurl/     # EngineFactory/EngineSession impl over embedded hurl
    proef-cli/             # bin `proef`: clap, registry assembly, miette rendering
    proef-fixture/         # dev-only: in-process synchronous fixture API server (ADR-0011)
    proef-harness/         # libtest-mimic bridge: one Trial per scenario (US-12)
  tests/                   # .feature corpus + fixtures
  docs/                    # this corpus
```

Dependency rules: engines depend on core; core depends on no engine; cli depends on both
and is the only miette user (ADR-0009). Engines sit behind cargo features in cli
(`engine-hurl` default-on; future `engine-web`, `engine-adb`). Only `proef-engine-hurl`
carries native build prereqs; `proef-core` is pure Rust. Lints/conventions: a strict
workspace lints table verbatim (clippy all=warn + curated pedantic slice), `publish =
false` initially (reserve names with 0.0.0 placeholders), MIT OR Apache-2.0.

## 3. Core domain types (sketches; signatures normative, field lists indicative)

```rust
// world.rs — typed variable scope (ADR-0005)
pub enum Value { String(String), Number(f64|i64…), Bool(bool), Null }   // mirrors hurl Value subset
pub struct World { scenario: BTreeMap<String, Value>,
                   global: GlobalStore /* .proef-state.json, atomic, snapshot/restore */ }

// step.rs — lowered, engine-agnostic
pub struct StepRef { pub file: Arc<str>, pub line: usize, pub text: Arc<str> }  // feature anchor
pub struct LoweredStep { pub step: StepRef, pub kind: StepKindId, pub payload: StepPayload,
                         pub optional: bool, pub retry: Option<Retry>, pub when: Option<Guard> }
pub enum StepPayload { HurlEntries(String /* lowered hurl text */), Structured(serde_json::Value) }
pub struct StepBatch { pub engine: EngineId, pub steps: Vec<LoweredStep> }

// engine.rs — the seam (ADR-0002); see ADR text for EngineFactory/EngineSession
pub struct StepKindSpec { pub prefix: &'static str, pub schema: &'static str /* JSON-Schema frag */ }
pub struct DoctorCheck { pub name: &'static str, pub run: fn() -> DoctorResult }
pub struct BatchResult { pub steps: Vec<StepOutcome>, pub error: Option<EngineError> }
pub struct StepOutcome { pub step: StepRef, pub status: Status, pub attempts: u32,
                         pub duration: Duration, pub detail: Option<String>,
                         pub artifact_span: Option<(u32, u32)> }

// events.rs — the spine (ADR-0008); serde, versioned
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event { RunStarted{..}, ScenarioStarted{..}, BatchStarted{..},
                 StepFinished{..}, ScenarioFinished{..}, RunFinished{..} }
pub struct EventSink(Arc<dyn Fn(&Event) + Send + Sync>);   // borrowed events
```

## 4. Pipeline (all in `proef-core`; pure — inputs include injected `run_id`, `now`, env snapshot)

**4.1 Load packs.** Discover embedded `helpers/` + project `packs/`; serde_norway with
`deny_unknown_fields`; validation passes: (1) `match:` guard rails — must contain literal
text, no adjacent captures, unclosed braces rejected; (2) params/defaults coverage;
(3) duplicate macro names across packs → error (qualify `pack.yaml#name`); (4) `use:`
cycle + depth ≤ 32; (5) unknown `with:` keys → "did you mean" (edit distance); (6)
**finite-retry lint** — `retry:` requires a finite count (ADR-0007); (7) hurl blocks:
lower a probe instantiation with placeholder params and `parse_hurl_file` it — syntax
errors reported with block-relative spans mapped to pack file/line; (8) engine kinds:
every step kind must be claimed by a registered engine's `StepKindSpec`.

**4.2 Parse features.** `gherkin` 0.16 (`Feature::parse`); `# key: value` directive
comments before `Feature:` (resolved through `${…}` immediately); tags from
Feature/Rule/Scenario accumulate. i18n via `# language:` honored by the crate.

**4.3 Bind.** For each step (keyword stripped): first macro whose `match:` pattern
matches wins; ambiguity (2+ matches) is an error listing candidates. Captured `{name}`
values: trimmed, surrounding quotes shed (quotes preserve inner spaces/commas). Data
table rows `| key | value |` merge into args; key set by both capture and table → error.
Defaults fill; missing required params → error. Unbound step → exit-2 error with
closest-pattern suggestion (edit distance over pattern literals).

**4.4 Lower.** Outline expansion (parser does not do it): per Examples row, substitute
`<col>` in scenario name, step text, docstrings, table cells; ragged rows / unknown
placeholders → parse-time error with line. Background steps prepend to every scenario.
Macro expansion: params bound, `${…}` resolved **recursively, depth ≤ 8** (captured args
may contain `${…}` — spike-verified necessity); `$${` escapes; `{{…}}` passes through
untouched. Assert-only macros (`expect:`) merge into the *previous* request entry —
error if none (Then-before-When). Product: `Vec<StepBatch>` per scenario (contiguous
same-engine runs; **batch maximally** — split only at `optional:` boundaries and engine
changes, ADR-0010).

**4.5 Emit.** Canonical `.hurl` per scenario (stable formatting, snapshot-tested):
header comment per entry `# <file>:<line> — <step text>`; `# optional` markers; sidecar
`<slug>.map.json` (schema: `{ entries: [{hurl_lines: [a,b], feature: {file,line,text},
optional, captures: [names], batch: n}], schema: 1 }`); `<slug>.vars` when `${global:}`
or `${secret:}` referenced (secrets as names only). Artifact dirs: `.proef-runs/<id>/
artifacts/` (per-run) and `proef artifacts -o <dir>` (stable hand-off).

**4.6 Dispatch.** Per scenario thread: check token → `factory.open(ctx)` lazily per
engine on first batch → `session.run_batch(batch, world, events, token)` in order →
merge outcomes/World → `finish()` all sessions (reverse open order) → emit events.
`optional:` batch failure → warnings + continue; else fail-fast within the scenario.

## 5. proef-engine-hurl internals (verified seam facts inline)

**Adapter.** Per batch: seed `VariableSet` from World (`insert`; secrets via
`insert_secret`) → `parse_hurl_file(&batch_text)` → `run_entries(&file.entries,
&batch_text, Some(&input), &runner_options, &variables, &mut stdout_buf,
Some(&listener), &mut logger)` with `WriteMode::Buffered` terms (upstream's own
threading mode: `parallel/worker.rs:76,124-133`) → map `EntryResult`s to `StepOutcome`s
via the sidecar (SourceInfo spans → feature lines) → merge `HurlResult.variables` back
into the World (typed).

**RunnerOptions mapping.** Batch-level `RunnerOptionsBuilder` from config/directives
(timeouts, follow-location, insecure, user-agent, context_dir…); per-entry `[Options]`
override batch defaults by clone-then-override (`runner/options.rs:43-58`), `variable=`
inserts persist for the rest of the call — verified semantics, relied upon.

**Client lifetime (verified).** `run_entries` constructs `http::Client::new()`
internally per call (`runner/hurl_file.rs:169`) — fresh libcurl handle: connection
cache and cookie jar do not survive across calls. Consequences implemented: batch
maximally (§4.4); on forced splits, chain variables via `HurlResult.variables`
(lossless) and, when cookies are in play, round-trip `HurlResult.cookie_store` →
`CookieStore::to_netscape()` → temp file → next batch's
`RunnerOptionsBuilder::cookie_input_file` (`http/cookie_store.rs:66-72`,
`runner_options.rs:242-244`) behind a `SessionState` struct. Upstream patch #1
(ADR-0003): `run_entries(&mut Client)` — two internal call sites
(`hurl_file.rs:124`, `worker.rs:124`); adopt when accepted, delete `SessionState`
cookie path.

**Thread-safety (verified).** No global mutable state in hurl (`static mut`/
lazy_static/OnceLock-mutable: zero hits); libcurl init is `Once`-guarded pre-main by
the curl crate; sole FFI global write is libxml2's error handler set idempotently per
XPath eval — exercised concurrently by upstream itself. Scenario-per-thread is safe.

**Cancellation & budgets (ADR-0007).** No interrupt support exists upstream (verified);
engine computes batch budget = Σ(timeout × (retry+1)) + intervals + margin; watchdog
abandons over-budget scenario threads; token checked between batches only.

**Timings/curl debug.** `EntryResult.calls[].timings` (libcurl `*_t`) surface into
`StepFinished` detail; `curl_cmd` exposed under `--output json` for debugging.

## 6. Pack schema v1 (normative field reference)

```yaml
templates:
  <macroName>:                # unique across packs; qualify as pack.yaml#name on clash
    params: [a, b]            # required unless defaulted
    defaults: { b: "x" }      # optional params
    match: "…{a}…"            # step-definition pattern; omit → not Gherkin-reachable
    description: "…"          # docs + desktop palette later
    tags: [Domain]
    steps:                    # request steps (each lowers to ≥1 hurl entry)
      - name: "…"             # entry label (events/console)
        optional: true|false  # failure → warning (segments the batch)
        when: "${expr}"       # skip guard: step runs iff non-empty after resolution
        retry: { count: N, interval_ms: M }   # finite only (lint); → [Options] retry
        saveAs: { captureName: global }        # promote capture(s) into the World
        hurl: |               # PRIMARY form (ADR-0004): raw hurl, ${…} lowered first,
          …                   # {{…}} left for run time; validated by parse_hurl_file
        # OR structured payload (reserved for future non-hurl engines):
        # <kind>: { … }  (a future engine's structured payload)
      - use: pack.yaml#other  # composition, with:/inline args; cycle+depth checked
        with: { a: "${a}" }
    expect:                   # assert-only macro (Then-steps): merges into previous entry
      - status: "${status}"   # or raw hurl assert lines: hurl: |‐style fragment (M5)
```

JSON Schema is schemars-derived from these serde types **plus** engine-contributed
`StepKindSpec` fragments; `proef schema --add-to` injects the editor modeline (a proven
mechanism).

## 7. Gherkin mapping reference

One scenario = one flow/run-record/artifact set. `Background:` prepends. `Rule:` groups
pass through (tags accumulate). Outline/Examples per §4.4. Data tables per §4.3.
Docstrings: reserved for raw request bodies in generic steps (M5). Tags: `@tag` →
flow tags, `--tags` filters (csv, OR semantics). Directives: `# baseURL:`,
`# app:` (+ open set stored into the directive scope). Step keywords: And/But resolve to
the previous primary keyword (gherkin crate `StepType`); keyword itself is not matched
against patterns.

## 8. Variables reference (ADR-0005)

Author-time (`${…}`, resolved in §4.4, recursive ≤ 8): `${param}` · `${env:NAME}` /
`${env:NAME:-default}` · `${run:id}` (uuid-v7-derived, injected) · `${global:key}`
(World read at lower time of the scenario) · `${secret:NAME}` (encrypted store; emits
`{{secret_name}}` + `insert_secret`) · `${fake:kind}` (deterministic from run id; port
deterministic NL generators) · `$${…}` literal escape. Run-time (`{{…}}`): hurl captures and
secret placeholders — resolved by the engine. Resolution order within a scope: step args
> macro defaults > directives > flow config.

## 9. Diagnostics (ADR-0009)

gherkin `Span` = 0-based byte offsets, end-exclusive → `SourceSpan::new(start,
end-start)`; parser appends a trailing newline when missing — attach the *normalized*
source text to diagnostics (or clamp); never use `LineCol.column` (char-counted) in byte
math. Pack YAML: serde_norway error locations; schema-path → YAML-location pass for
lint findings. Engine failures render: feature line + step text + assert detail +
artifact path:span (from sidecar). Every diagnostic carries a stable code
(`proef::pack::adjacent_captures`, …) for greppability.

## 10. CLI reference (v1)

```
proef test <file|dir> [--dry-run] [--tags csv] [--jobs N] [--junit path|auto]
                      [--output json] [--watch] [--scenario NAME]
proef flows [dir]              proef artifacts <file|dir> -o DIR
proef schema [--add-to FILE…]  proef secret set|list
proef explain [run-id]         proef doctor
proef fmt <file|dir> [--check]
```

Exit codes: 0 ok · 1 test failure · 2 user error · 3 system error (typed enum,
assert_cmd-pinned). Config precedence: built-in defaults < `proef.toml` < flags;
secrets additionally resolve `PROEF_SECRET_<NAME>` env overrides before the store
(there is no generic `PROEF_*` config layer). `--dry-run` = §4.1–4.5 including artifact
parse-validation; no engine sessions, no network.

## 11. State & files

`.proef-runs/<run-id>/` → `events.jsonl` (the record, ADR-0008), `run.log` (console tee),
`artifacts/*.hurl|.map.json|.vars`, `report.junit.xml` (when requested); 200-run
rotation. `.proef-state.json` — persistent World: atomic temp+rename,
snapshot/restore around scenario retries, 0600. `proef.toml` — project config
(base timeouts, jobs default, artifact dir, engine settings).

## 12. Parallelism & cancellation

Scenario-per-OS-thread, `--jobs` bounded (default: available_parallelism, capped by
scenario count); events funneled through the sink (Normalize reporter repairs
interleaving); one CancellationToken per run, child per scenario; Ctrl-C graceful /
second Ctrl-C hard (ADR-0007). Global-World writes serialize through the store lock;
scenario ordering within a file is preserved for artifact naming, not execution order.

## 13. Security

Secrets: encrypted at rest (chacha20poly1305), surfaced only
via `insert_secret`; redaction invariants (never in artifacts/events/reports/logs)
property-tested; sensitive files 0600. `context_dir` confines file bodies (hurl's own
sandbox option). No telemetry.

## 14. Dependencies (exact at M0; policy: latest stable at adoption, Renovate-managed)

Engine: `hurl =8.0.1`, `hurl_core =8.0.1` (`--locked`; ADR-0003). Core: `gherkin 0.16`,
`serde 1`, `serde_json 1`, `serde_norway 0.9`, `schemars 1`, `thiserror 2`,
`tokio-util 0.7` (default-features = false; CancellationToken only). CLI: `clap 4`
(derive, env), `miette 7` (fancy), `uuid 1` (v7), `notify =8.2.0`, `ctrlc`,
`chacha20poly1305 rpassword base64`, `quick-junit`, `toml`. Fixture/harness (dev):
`tiny_http` (ADR-0011 — axum conflicts with the tokio-runtime ban), `libtest-mimic`.
Dev: `insta assert_cmd proptest tempfile quick-xml` + `cargo-fuzz` targets. Synthetic
data (`${fake:*}`) is a dependency-free SplitMix64/FNV implementation in-core — the
`fake` crate was not needed. Datetime never enters our code (the jiff-not-chrono rule
stands for the day it does). Banned: serde_yaml/serde_yml, chrono (ours), reqwest
(superseded), maybe-async, async-trait (v1). Build prereqs (doctor-checked): Debian
`build-essential pkg-config libssl-dev libcurl4-openssl-dev libxml2-dev libclang-dev`;
macOS: Xcode CLT.

## 15. Conventions

Toolchain pinned latest stable (1.97.1 at writing); edition 2024; resolver 3; workspace
lints (the legacy suite table); CI gates: `cargo fmt --check`, `clippy --all-targets
--all-features -D warnings`, `cargo nextest run`, `cargo test --doc`,
`RUSTDOCFLAGS="-D warnings" cargo doc`, `cargo deny check`, `cargo audit`; commit gates
mirror CI. Automation in `xtask` (+ `just` aliases); no shell scripts for logic.
