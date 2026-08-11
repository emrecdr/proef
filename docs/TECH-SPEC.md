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
   ┌──────────────────┐  ┌──────────────────┐
   │ proef-engine-hurl│  │ future non-hurl  │
   │ parse_hurl_file +│  │ engine (seam-    │
   │ run_entries      │  │ ready, none      │
   └──────────────────┘  │ scheduled)       │
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
  xtask/                   # automation as Rust (fixture, canary, docs-check, public-api); just aliases
  crates/
    proef-core/            # engine-agnostic: gherkin parse, packs, binding, lowering, IR,
      helpers/             #   emit, dispatch, World/state, events, errors, reporters
    proef-engine-hurl/     # EngineFactory/EngineSession impl over embedded hurl
    proef-cli/             # bin `proef`: clap, registry assembly, miette rendering
    proef-fixture/         # dev-only: in-process synchronous fixture API server (ADR-0011)
    proef-harness/         # libtest-mimic bridge: one Trial per scenario (US-12)
    proef-lsp/             # language server: SourceProvider + collect-all analyze_suite over core
  tests/                   # .feature corpus + fixtures
  docs/                    # this corpus
```

Dependency rules: engines depend on core; core depends on no engine; cli depends on both
and is the only miette user (ADR-0009). Engines sit behind cargo features in cli
(`engine-hurl` default-on; any future non-hurl engine would be added the same way — none
scheduled). Only `proef-engine-hurl`
carries native build prereqs; `proef-core` is pure Rust. Lints/conventions: a strict
workspace lints table verbatim (clippy all=warn + curated pedantic slice), `publish =
false` at the workspace root — overridden to `true` by the four crates that publish
(`proef`, `proef-core`, `proef-engine-hurl`, `proef-lsp`), MIT OR Apache-2.0.

## 3. Core domain types (sketches; signatures normative, field lists indicative)

```rust
// world.rs — typed variable scope (ADR-0005)
pub enum Value { String(String), Number(f64|i64…), Bool(bool), Null }   // mirrors hurl Value subset
pub struct World { scenario: BTreeMap<String, Value>,
                   global: GlobalStore /* .proef-state.json, atomic temp+rename save */ }

// step.rs — lowered, engine-agnostic
pub struct StepRef { pub file: Arc<str>, pub line: usize, pub text: Arc<str> }  // feature anchor
pub struct LoweredStep { pub step: StepRef, pub kind: StepKindId, pub payload: StepPayload,
                         pub optional: bool, pub when: Option<Guard>,
                         // `file.hurl#name` for a `ref:` step, None for an inline block
                         // (ADR-0018) — qualified at lowering so a record stands alone
                         pub fragment: Option<String> }   // retry travels as baked [Options]
pub enum StepPayload { HurlEntries(String /* lowered hurl text */),
                       MergedAsserts { lines: usize /* expect: rows own the appended assert lines */ },
                       Structured(serde_json::Value) }
pub struct StepBatch { pub index: usize /* scenario-wide ordinal */, pub engine: EngineId,
                       pub steps: Vec<LoweredStep> }

// engine.rs — the seam (ADR-0002); see ADR text for EngineFactory/EngineSession
pub struct StepKindSpec { pub prefix: &'static str, pub schema: &'static str /* JSON-Schema frag */,
                          pub validate: Option<fn(&str) -> Result<(), PayloadProbeError>>,
                          // fragment files (ADR-0018): one Option, so extension and reader
                          // cannot disagree; discovery asks for the extension, never names one
                          pub fragments: Option<FragmentSupport> }
pub struct FragmentSupport { pub ext: &'static str /* "hurl" */, pub scan: FragmentScanner }
pub type FragmentScanner = fn(&str) -> Result<Vec<ScannedFragment>, FragmentScanError>;
// Everything here is *read* from the entry — nothing is declared twice, so nothing can drift.
// A scanner reports ONLY annotated entries: an unannotated one is not a fragment, and a
// foreign corpus is mostly those, so building them only to be discarded is the bulk of a scan
pub struct ScannedFragment { pub name: String /* from `# @proef <name>` */, pub text: String,
                             pub line: usize, pub placeholders: Vec<String> /* reads */,
                             pub declared_options: Vec<String> /* ⊆ OPTION_FAMILIES */ }
pub struct FragmentScanError { pub line: usize, pub column: usize, pub message: String }
// The vocabulary `declared_options` must use: matched by string equality against the pack's
// own option keys, so an engine-only spelling silences `option_declared_twice` rather than
// firing it. `MacroStep::declared_options()` derives the other half of that comparison.
pub const OPTION_FAMILIES: &[&str] = &["retry", "delay"];
// The one place `secret_bindings` (variable → secret) is joined with `secrets` (name → value).
// Yields borrows: an owned map would copy every secret value per scenario (ADR-0005)
pub fn secret_variables<'a>(bindings: &'a BTreeMap<String, String>,
                            secrets: &'a BTreeMap<String, String>)
                            -> impl Iterator<Item = (&'a str, &'a str)>;
pub struct DoctorCheck { pub name: &'static str, pub run: fn() -> DoctorResult }
pub struct BatchResult { pub steps: Vec<StepOutcome>, pub error: Option<EngineError> }
// `fragment` is carried here as well as on the event: JUnit, the job summary, the
// annotations, TAP and the console are built from RunSummary after the event stream has
// been written out, so they cannot read it back. Both are copies of one lowering-time
// source, so they cannot drift from each other.
pub struct StepOutcome { pub step: StepRef, pub status: Status, pub attempts: u32,
                         pub duration: Duration, pub detail: Option<String>,
                         pub attempt_details: Vec<String>, pub reproduce_hint: Option<String>,
                         pub fragment: Option<String> }

// events.rs — the spine (ADR-0008); serde, versioned
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event { RunStarted{..}, ScenarioStarted{..}, BatchStarted{..}, EntryRunning{..},
                 StepFinished{..}, ScenarioFinished{..}, RunFinished{.., cancelled} }
pub struct EventSink(Arc<dyn Fn(&Event) + Send + Sync>);   // borrowed events
```

## 4. Pipeline (all in `proef-core`; pure — inputs include injected `run_id`, `now`, env snapshot)

**4.1 Load packs.** The fragment corpus (`[run] fragments`) is read once per command
into a `pack::FragmentCorpus` and **scanned at most once**, lazily — only when some pack
actually carries a `ref:`, which is what makes "pointing at a corpus you did not write
costs nothing" true of the scan (CONFIG.md). One `proef test` loads packs up to four
times (the suite, then `[run] setup`/`teardown`, each validated and then run) against
the same corpus, so the memo belongs with the corpus rather than the caller — a caller
that scanned eagerly to share the result would trade the promise for the speed.
Discover embedded `helpers/` + project `packs/`; serde_norway with
`deny_unknown_fields`; validation passes: (1) `match:` guard rails — must contain literal
text, no adjacent captures, unclosed braces rejected; (2) params/defaults coverage;
(3) duplicate macro names across packs → error (qualify `pack.yaml#name`); (4) `use:`
cycle + depth ≤ 32; (5) unknown `with:` keys → "did you mean" (edit distance); (6)
**finite-retry lint** — `retry:` requires a finite count (ADR-0007); (7) hurl blocks:
lower a probe instantiation with placeholder params and `parse_hurl_file` it — syntax
errors reported with block-relative spans mapped to pack file/line; (8) engine kinds:
every step kind must be claimed by a registered engine's `StepKindSpec`.

Fragments (ADR-0018) add five: (9) a `ref:` must name a loaded fragment → "did you
mean" over the scanned names, or a pointer at `[run] fragments` when none loaded;
(10) duplicate fragment name across files → error (qualify `file.hurl#name`, the same
suffix matching `pack.yaml#name` uses); (11) a step is `ref:` **xor** a payload/`use:`;
(12) an option family declared both in the fragment's own `[Options]` and as the step's
YAML key → error (pass 6's twinned-option rule, applied across the file boundary);
(13) a fragment file the engine's `FragmentSupport::scan` could not read, or an
annotation it could not attach, positioned in the `.hurl` file itself.

Fragments **skip pass 7**: they parse as authored, so the probe instantiation has
nothing to guess. Whether every `{{…}}` a fragment reads is actually supplied is
checked at lower time (§4.4, `proef::lower::unbound_placeholder`) — only lowering
knows what the preceding steps captured, and a load-time half-check would be worse
than one complete one.

**4.2 Parse features.** `gherkin` 0.16 (`Feature::parse`); tags from
Feature/Rule/Scenario accumulate. Localized (`# language:`) features are
supported and test-covered: the crate strips the dialect keywords, proef
consumes the stripped step text, and a localized outline with `Examples`
expands like any other (outline detection keys on `Examples` presence, which is
dialect-independent). Caveat: a *localized* outline whose `Examples` block is
omitted cannot be told apart from a plain scenario (the crate's dialect keywords
are private), so it surfaces as an unbound-step error rather than the crisp
`no_examples` — a worse message, never a silent pass. `#` comment lines are plain
gherkin comments (no `# key:` directive mechanism — variables live in
`proef.toml`, ADR-0012).

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
optional, captures: [names], batch: n, step: n}], schema: 1 }`); `<slug>.vars` when `${global:}`
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

**RunnerOptions mapping.** Batch-level `RunnerOptionsBuilder` from config
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

**Failure detail.** Engine errors surface through hurl's own
`DisplaySourceError::description` into `StepFinished.detail` (additive event
field), the console, JUnit, and the GitHub summary.

## 6. Pack schema v1 (normative field reference)

```yaml
bind:                         # pack-scope fragment bindings (ADR-0018); macro and
  <var>: "${…}"               # step scope override, most specific winning
macros:
  <macroName>:                # unique across packs; qualify as pack.yaml#name on clash
    params: [a, b]            # required unless defaulted
    defaults: { b: "x" }      # optional params
    match: "…{a}…"            # step-definition pattern; omit → not Gherkin-reachable
    description: "…"          # docs + desktop palette later
    tags: [Domain]
    steps:                    # request steps (each lowers to ≥1 hurl entry)
      - name: "…"             # entry label (events/console)
        optional: true|false  # failure → warning (segments the batch)
        when: "${expr}"       # skip guard: skips when empty or literal false/0 after resolution
        retry: { count: N, interval_ms: M }   # finite only (lint); → [Options] retry
        saveAs: { captureName: global }        # promote capture(s) into the World
                              # (refused with a warning if the value equals a secret)
        bind: { <var>: "…" }  # step-scope fragment bindings (only with `ref:`)
        hurl: |               # PRIMARY form (ADR-0004): raw hurl, ${…} lowered first,
          …                   # {{…}} left for run time; validated by parse_hurl_file
        # OR structured payload (reserved for future non-hurl engines):
        # <kind>: { … }  (a future engine's structured payload)
      - ref: file.hurl#name   # ALTERNATE form (ADR-0018): one `# @proef <name>` entry
                              # of a scanned fragment file; `{{…}}` supplied by bind:,
                              # every one bound or captured by an earlier step
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
flow tags, `--tags` filters by a boolean expression (`and`/`or`/`not`/parens,
`proef_core::tags`). Step keywords: And/But resolve to
the previous primary keyword (gherkin crate `StepType`); keyword itself is not matched
against patterns.

## 8. Variables reference (ADR-0005)

Author-time (`${…}`, resolved in §4.4, recursive ≤ 8): `${param}` · `${env:NAME}` /
`${env:NAME:-default}` · `${url:key}` / `${vars:key}` (proef.toml `[url]`/`[vars]`, base +
active `[env.<name>]` deep-merged; injected — ADR-0012) · `${run:id}` (uuid-v7-derived, injected) · `${global:key}`
(World read at lower time of the scenario) · `${secret:NAME}` (encrypted store; emits
`{{secret_name}}` + `insert_secret`) · `${fake:kind}` (deterministic from run id and an
occurrence index; the index is an incrementing counter scoped to one scenario — shared
across every `${fake:…}` resolve in it, so independent references never collide regardless
of how many a step resolves; a step's `name:` label resolves from a rewound copy of the
counter so its Nth `${fake:…}` reference reuses the payload's/`when:`'s Nth occurrence by
*position*, not generator kind — reproducing the payload's own value exactly when the
label's references mirror the payload's in kind and order, otherwise surfacing that
occurrence's own-kind value instead — then restores the real counter to the high-water
mark the replay reached (never below it), so an extra fake the label alone introduces
still reserves its slot and is never reissued;
the counter resets to zero at the next scenario, so the same generator at the same position
in two different scenarios currently coincides; port deterministic NL generators) · `$${…}`
literal escape. Run-time (`{{…}}`): hurl captures and
secret placeholders — resolved by the engine. Resolution order within a scope: step args
> macro defaults.

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
proef init [dir]
proef test [file|dir] [--env NAME] [--dry-run] [--tags EXPR] [--jobs N] [--junit path|auto]
                      [--output json|tap] [--watch] [--scenario NAME] [--scenario-file FILE]
                      [--run-id ID] [--rerun] [--sarif PATH (with --dry-run)]
proef flows [file|dir] [--env NAME] [--output json]
proef macros [file|dir] [--env NAME] [--output json]
proef artifacts [file|dir] -o DIR [--env NAME] [--run-id ID]
proef schema [--add-to FILE…]  proef secret set|list|rm
proef explain [run-id]         proef doctor
proef diff [base] [new] [--fail-on-regression]
proef report [run-id] [-o FILE]
proef fmt <file|dir> [--check]
proef lsp
```

A path-less `test`/`flows`/`artifacts` resolves `[run] suite` then the `tests/`
convention (else exit 2). Exit codes: 0 ok · 1 test failure · 2 user error · 3
system error (typed enum, assert_cmd-pinned). A second interrupt (Ctrl-C) while
a `test`/`watch` run is cancelling forces an immediate hard exit with code
**130** (128+SIGINT, the shell convention) — deliberately outside the graceful
0/1/2/3 taxonomy, so it is not an `ExitCode` variant. Config precedence: built-in defaults
< `proef.toml` base tables < active `[env.<name>]` (selected by `--env`/`PROEF_ENV`)
< flags; suite variables `${url:key}`/`${vars:key}` resolve from `[url]`/`[vars]`
deep-merged with the active env (ADR-0012). Secrets additionally resolve
`PROEF_SECRET_<NAME>` env overrides before the store, and `PROEF_KEY` (base64)
overrides the key file — CI decrypts a committed ciphertext store without the key
ever touching disk. `--dry-run` = §4.1–4.5 including artifact parse-validation;
no engine sessions, no network.

## 11. State & files

`.proef-runs/<run-id>/` → `events.jsonl` (the record, ADR-0008), `run.log` (console tee),
`artifacts/*.hurl|.map.json|.vars`, `report.html`, `report.junit.xml` (when requested);
200-run
rotation (only uuid-named run records rotate; the in-flight run never does).
`.proef-state.json` — persistent World: atomic temp+rename, 0600. `proef.toml` — project config:
runner settings (`[run]` jobs/runs-dir/suite, `[http]` timeouts) + suite variables
(`[url]`/`[vars]`) + per-environment overrides (`[env.<name>]`); see docs/CONFIG.md, ADR-0012.

## 12. Parallelism & cancellation

Scenario-per-OS-thread, `--jobs` bounded (default: available_parallelism, capped by
scenario count); events funneled through the sink (the console reporter buffers per
scenario and replays contiguously); one CancellationToken per run, child per scenario; Ctrl-C graceful /
second Ctrl-C hard (ADR-0007). Global-World writes serialize through the store lock;
scenario ordering within a file is preserved for artifact naming, not execution order.

## 13. Security

Secrets: encrypted at rest (chacha20poly1305), surfaced only
via `insert_secret`; redaction invariants (never in artifacts/events/reports/logs)
property-tested; a `saveAs: global` capture whose value equals a known secret is
refused (warned) — `.proef-state.json` is plaintext and never receives
secret-derived material; sensitive files 0600; `proef doctor` reports store/key
health. `context_dir` confines file bodies (hurl's own
sandbox option). No telemetry.

## 14. Dependencies (exact at M0; policy: latest stable at adoption, Renovate-managed)

Engine: `hurl =8.0.1`, `hurl_core =8.0.1` (`--locked`; ADR-0003). Core: `gherkin 0.16`,
`serde 1`, `serde_json 1`, `serde_norway 0.9`, `schemars 1`, `thiserror 2`,
`tokio-util 0.7` (default-features = false; CancellationToken only). CLI: `clap 4`
(derive, env), `miette 7` (fancy), `uuid 1` (v7), `notify =8.2.0`, `ctrlc`,
`chacha20poly1305 rpassword base64`, `quick-junit`, `toml`. LSP: `lsp-server 0.7`,
`lsp-types 0.97` (proef-lsp's stdio transport, wired into `proef lsp`). Fixture/harness (dev):
`tiny_http` (ADR-0011 — axum conflicts with the tokio-runtime ban), `libtest-mimic`.
Engine runtime: `tempfile` (Netscape cookie round-trip between batches, §5).
Dev: `insta assert_cmd predicates proptest tempfile quick-xml` + `cargo-fuzz`
targets; `openssl-sys` rides as the engine's `vendored-openssl` feature carrier. Synthetic
data (`${fake:*}`) is a dependency-free SplitMix64/FNV implementation in-core — the
`fake` crate was not needed. Datetime uses `jiff`, never `chrono`, in our own code
(hurl's internal chrono is its business) — currently `jiff` is a dev-only dependency
of the fixture's `/health` identity; the sans-IO core still reads no clock (injected
timestamps only). Banned: serde_yaml/serde_yml, chrono (ours), reqwest
(superseded), maybe-async, async-trait (v1). Build prereqs (doctor-checked): Debian
`build-essential pkg-config libssl-dev libcurl4-openssl-dev libxml2-dev libclang-dev`;
macOS: Xcode CLT.

## 15. Conventions

Toolchain pinned latest stable (1.97.1 at writing); edition 2024; resolver 3; workspace
lints (the legacy suite table); CI gates (PR): `cargo fmt --check`, `clippy --all-targets
--all-features -D warnings`, `cargo nextest run`, `cargo test --doc`,
`RUSTDOCFLAGS="-D warnings" cargo doc`, `cargo deny check`, `cargo machete`, `zizmor`,
`xtask docs-check`, `proef doctor`, fuzz smoke + `xtask public-api` (nightly rustdoc);
`cargo audit` runs on the nightly schedule. Automation in `xtask` (+ `just` aliases); no shell scripts for logic.
