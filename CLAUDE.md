# CLAUDE.md

This file guides Claude Code when working in this repository. It is the entry point, not
the spec — the authoritative corpus lives in `docs/` and wins on any conflict.

## What this is

**proef** (Dutch: *test/trial* — and *tasting*) is a declarative, modular, multi-engine
end-to-end test runner, mostly Rust. Tests are Gherkin `.feature` files in business
prose; YAML macro packs (with embedded raw Hurl blocks) bind prose to executable steps;
an engine-agnostic core dispatches step batches to pluggable engines. The first engine
embeds **hurl** in-process for API testing — the only engine; the seam is the
sanctioned extension point (architectural readiness only, nothing scheduled).

## Read before large changes

1. `docs/README.md` — corpus index and ADR decision log.
2. The relevant **ADR** (`docs/adr/ADR-0001` onward) — decisions with alternatives and
   consequences. Diverging from an ADR without writing a superseding ADR is a bug.
3. `docs/TECH-SPEC.md` — normative types, pipeline, pack schema, and the **verified
   hurl seam facts with file:line citations** (§5). Do not re-derive these from priors.
4. `docs/IMPLEMENTATION-PLAN.md` — the milestone you are implementing, its acceptance
   criteria, and the global definition of done.
5. `docs/PRD.md` — the user stories (US-1…US-12) that milestone acceptance criteria
   cite, and the product scope boundaries.

New architectural decision → add `docs/adr/ADR-00NN-*.md` (next number, same format)
in the same PR. Keep the **Status** section at the bottom of this file current as
milestones land.

## Toolchain (pinned)

**Latest stable Rust, adopted at its `x.y.1` point release** (~3-4 weeks after
`x.y.0` — new minors wait out their first patch; 1.97.1 at writing), pinned via
`rust-toolchain.toml`;
edition 2024; workspace `resolver = "3"`. Tools to install once:
`cargo install cargo-nextest cargo-deny cargo-audit cargo-insta just`
(plus `cargo-fuzz` for fuzz targets, `cargo-llvm-cov` for coverage).

## Common commands

```bash
cargo build                                   # whole workspace
cargo nextest run                             # all tests (preferred)
cargo nextest run -p proef-core <substring>   # one crate / one test
cargo test --doc                              # doctests (nextest doesn't run them)
cargo insta test --review                     # snapshot tests (emitter/diagnostics/events)

cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo deny check && cargo audit
# fuzz/ is its own workspace (root `exclude`), so nothing above compiles it —
# a changed proef-core signature breaks the fuzz targets with every gate green:
cargo check --manifest-path fuzz/Cargo.toml --all-targets

cargo run -p proef -- test tests/features --dry-run  # validate: bind + lower + emit + parse
cargo run -p proef -- test tests/features --jobs 4   # execute (secrets: store via `proef secret set`, or PROEF_SECRET_<NAME> env)
cargo run -p proef -- test --env staging             # path-less (default [run] suite / tests/) + a proef.toml [env.<name>] profile
cargo run -p proef -- artifacts tests/features -o out/ --run-id ci  # emit .hurl + sidecars
cargo run -p proef -- flows tests/features           # list scenarios with anchors + tags
# tests/errors/ is the seeded broken corpus (one dir per diagnostic code) — dry-running it fails by design
cargo run -p proef -- doctor                   # native libs / env checks
cargo run -p xtask -- fixture                      # local fixture API server on :8787 (dev; falls back if busy)
cargo run -p xtask -- canary                       # build+test against next hurl release
```

Build prerequisites (Linux): `apt install build-essential pkg-config libssl-dev
libcurl4-openssl-dev libxml2-dev libclang-dev`. macOS: Xcode CLT suffices. Only
`proef-engine-hurl` needs these; `proef-core` is pure Rust.

## Workspace architecture

```
crates/
  proef-core/         engine-agnostic: gherkin parse, packs, binding, lowering, IR,
                      emit, dispatch, World/state, events, errors, reporters
    helpers/          built-in macro packs (embedded at build time)
  proef-engine-hurl/  the API engine: EngineFactory/EngineSession over embedded hurl
  proef-cli/          bin `proef`: clap, engine registry assembly, miette rendering
  proef-fixture/      dev-only: in-process sync fixture API server (tiny_http, ADR-0011)
  proef-harness/      libtest-mimic bridge: one Trial per scenario (US-12)
  proef-lsp/          language server: SourceProvider + collect-all analyze_suite over core
xtask/                automation as Rust (fixture, canary, docs-check, public-api); `just` = thin aliases
```

**The central seam** (ADR-0002): `EngineFactory` (id, `step_kinds()` pack-schema
contribution, `doctor()`, `open`) + `EngineSession` (`run_batch`, `finish`), both sync,
used as `Box<dyn …>`. Routing: a macro step's kind names its engine (`hurl:` →
engine-hurl; other kind prefixes reserved — ADR-0002 errata). The core batches **contiguous same-engine
steps** and dispatches in order; the **World** (typed vars + persistent global store)
threads captures between batches and engines. Registry lives in `proef-cli` (one line
per engine, cargo-feature-gated). **Dependency rules:** engines depend on core; core
depends on no engine and no engine-specific type; only `proef-cli` uses miette; engines
never import each other. The structural acceptance test: *adding an engine leaves
`proef-core` diff-empty*.

## Hard constraints (these override training priors — get them wrong and it breaks)

- **hurl pins are exact and sacred:** `hurl = "=8.0.1"`, `hurl_core = "=8.0.1"`, built
  `--locked`. NEVER bump casually — the crates break API in minor releases. Upgrades go
  through the canary + runbook (IMPLEMENTATION-PLAN §7); patches go on the thin fork via
  `[patch."crates-io"]` and get PR'd upstream (ADR-0003).
- **Verified hurl seam facts** (TECH-SPEC §5 has file:line): `run_entries` builds its
  HTTP client *per call* → **batch maximally**; on forced splits chain
  `HurlResult.variables` and round-trip cookies via Netscape temp file. hurl has **no
  cancellation** and allows infinite retries → finite-retry pack lint + batch budgets +
  watchdog (ADR-0007). Per-entry `[Options]` override batch `RunnerOptions`
  (clone-then-override). Always `WriteMode::Buffered` in library paths — `Immediate`
  interleaves under threads.
- **hurl feature-unifies `serde_json/arbitrary_precision` into every workspace build**
  (discovered at M0): numbers then reach serde's content-buffered paths as a private
  token map, which **breaks `#[serde(untagged)]` enums** on numeric variants
  (internally-tagged enums survive). Write hand-rolled visitors for scalar types
  (see `proef_core::world::Value`) and never add new untagged enums that carry
  numbers; `value_json_forms_round_trip` pins the behavior.
- **YAML = `serde_norway`**, never `serde_yaml` (archived) or `serde_yml` (bad fork).
  **Datetime = `jiff`** in our code, never chrono (hurl's internal chrono is its
  business). **`notify` pinned 8.2.0** (9.x is a prerelease). **Banned:** reqwest
  (superseded by the embedded engine), `async-trait`/`maybe-async` (ADR-0006),
  tokio runtime (only `tokio-util` with `default-features = false` for
  `CancellationToken`). **Also banned — raw print macros in `proef-cli`:**
  `println!`/`eprintln!` panic when the write fails, so a closed pipe
  (`proef … | head`) aborts with 101, outside the typed exit contract; use
  `crate::render::outln!` / `errln!`, which a source-scanning test enforces.
- **Core purity (sans-IO lite):** `proef-core` does no IO, reads no clocks/env, and
  generates no randomness — `run_id`, timestamps, and env snapshots are injected values.
  This is what makes snapshots/properties deterministic; do not break it for
  convenience.
- **Two-tier variables (ADR-0005):** `${…}` resolves at lower time (recursive, depth ≤ 8,
  `$${` escape — captured step args may themselves contain `${…}`); `{{…}}` is hurl
  run-time and must pass through core untouched. External config variables
  `${url:key}` / `${vars:key}` come from `proef.toml` (`[url]`/`[vars]`, deep-merged with
  the active `[env.<name>]` via `--env`/`PROEF_ENV`), injected into the sans-IO core as
  `LowerCtx::config_vars` — the CLI does the file IO, not core (ADR-0012). Secrets go through
  `VariableSet::insert_secret` and **never** appear in artifacts, events, logs,
  reports, or the persistent World — `saveAs: global` refuses secret-valued
  captures (property-tested invariant — keep it green; events carry capture
  *names* only).
- **Artifacts are the executed input (ADR-0010):** the emitted `.hurl` text and the text
  handed to `parse_hurl_file` must be the *same bytes* (hash-asserted in tests). The
  canonical format is snapshot-locked — emitter changes require deliberate
  `cargo insta review`, never blind acceptance.
- **Diagnostics:** gherkin `Span` = 0-based **byte** offsets (end-exclusive) → miette
  `SourceSpan`; the parser appends a trailing newline when missing (normalize/clamp);
  `LineCol.column` is char-counted — never use it in byte math.
- **Events (ADR-0008):** the serde `Event` enum is a versioned schema (`schema` field);
  changes are additive-only; the JSONL run record *is* the event stream — no second
  record format.
- **Exit codes are a contract:** `0` ok · `1` test failure · `2` user error · `3` system
  error — typed enum, pinned by assert_cmd integration tests; error variants map via the
  fault taxonomy (ADR-0009).
- **Pack rules (ADR-0004):** the pack root key is `macros:` (renamed from `templates:`,
  ADR-0004 amendment — no alias); hurl steps use raw `hurl:` block scalars (validated by
  `parse_hurl_file` at load); structured payloads are reserved for future engines;
  `expect:` macros merge asserts into the *previous* request entry; `retry:` must be
  finite; `use:` nesting is cycle-checked, depth ≤ 32.
- **Two body forms, chosen by capability (ADR-0018):** a step is `hurl: |` (lower-time
  **splicing**: `${…}` substitutes anything anywhere, including a multi-line
  `${docstring}` body — which no binding can express) **or** `ref: <name>` (run-time
  **binding**: one `# @proef <name>` entry of a real `.hurl` file, values supplied by
  `bind:` at pack/macro/step scope, most specific winning). Neither subsumes the other,
  and the inline path is backed by a measured 844-line corpus port — do not deprecate it.
  The annotation carries **a name and nothing else, permanently**; all orchestration
  stays in YAML. Fragment files are *inputs proef never writes*: `fmt` refuses them.
  Every `{{…}}` a fragment reads must be bound, captured by an earlier step, or supplied
  by the fragment's own `[Options] variable:` (which keeps the file runnable standalone —
  and which therefore *collides* with a `bind:` of that name, refused as
  `option_declared_twice`), because
  hurl's per-entry `variable:` **assigns into one shared set rather than scoping**. A
  bound `[Options] variable:` value *is* templated once before storage, so a bound
  `${url:…}` containing `{{captured}}` resolves — `eval_template` being non-recursive
  applies to rendering, not to the binding path.

## Testing expectations (`docs/TESTING-STRATEGY.md` is normative)

Every layer device-free and network-free except the fixture-server integration suite.
Unit + proptest (matcher, resolver, secret-mask, World) + cargo-fuzz (matcher, resolver,
pack loader; smoke in PR CI, full nightly) + insta snapshots (artifacts, diagnostics,
schema, event streams) + fixture integration (green path = the reference-corpus features;
retry/cookies/optional/World/cancellation cases) + `--dry-run` corpus over `tests/` +
assert_cmd CLI/exit-code suite + the hurl upgrade canary. Flake rule: assert attempt
counts and normalized event order, never raw interleaving. Wall time only as a
generous upper bound — and a *timing* assertion (the complexity ratios) is
`#[ignore]`d and runs alone, via `just perf` and its own CI step, because a ratio
measured beside the rest of the suite drifts rather than cancelling
(TESTING-STRATEGY §7). Every diagnostic code is named by a test, enforced by a
`source_guards.rs` scan (§6).

**The reference corpus (`tests/features`) is config-independent by design** — several
tests run it from a temp cwd with settings passed by environment variable and no
`proef.toml` in scope. Anything needing a config key (`[run] fragments`, …) belongs in a
self-contained test that builds its own project; `crates/proef-cli/tests/fragments.rs` is
the pattern, and it runs the same file under proef *and* under stock `hurl` (skipped, with
a printed note, when no `hurl` is on PATH — the engine is embedded, so the binary is not a
build requirement).

## Status (update as milestones land)

- [x] M0 — foundations: workspace, gates, seam traits, error/exit model, doctor
- [x] M1 — front end: packs + validation, matcher, gherkin, lowering, `--dry-run`
- [x] M2 — IR, canonical emitter, sidecars, `artifacts`, snapshot corpus
- [x] M3 — engine-hurl execution: adapter, World bridge, parallelism, budgets, console+JSONL
- [x] M4 — upstream tracking: real canary, thin-fork rehearsal, upstream PR #1, JUnit/GH summary
- [x] M5 — breadth: bodies (multipart/form/docstring), watch, explain, secrets CLI, fakes, libtest-mimic harness, `proef fmt`
- [x] post-M5 — external config & environments (`proef.toml` `[url]`/`[vars]`/`[env.<name>]`, `${url:}`/`${vars:}`, `--env`/`PROEF_ENV`, ADR-0012); default suite path (`[run] suite`); pack root key `templates:` → `macros:` (ADR-0004 amendment); dev fixture binds default port 8787 + versioned `/health` (ADR-0011 amendment)
- [x] correctness series (v0.6.0–v0.8.0) — three releases closing one bug class:
      *proef reported success while producing wrong, incomplete, or
      silently-ignored output*. `run_finished` is a record's last line again;
      `${fake:…}` no longer repeats across a scenario's steps; `.map.json`
      stops both inventing capture rows and dropping real ones; a
      whitespace-only `expect:` is rejected rather than emitting an inverted
      span; an unreadable `PROEF_*` variable is a loud error instead of
      reading as unset; a failed stdout write reaches the exit code; `fmt`
      keeps a file's line endings; `report -o` writes links that resolve;
      `diff` stops inventing flakiness. Breaking along the way:
      `proef_core::resolve::resolve` takes a caller-owned occurrence counter,
      a scenario with no steps is an error, and exit codes moved (stdout
      failure → 3, malformed env var → 2)
- [x] v0.9.0 — tool-surface integrity & authoring guidance (`fmt` refuses a non-pack and
      leaves the skeleton alone, pinned by properties; a truncated record counts its
      warned scenarios; AUTHORING gains the validation-error catalogue and the
      outline-into-docstring pattern). Breaking: `secret set --value` → `--stdin`;
      `macros --format json` `pattern` is `string|null`
- [x] named hurl fragments (ADR-0018) — a step may `ref:` one `# @proef <name>` entry of
      a real `.hurl` file, values supplied by `bind:` at pack/macro/step scope; the file
      stays valid hurl, so the same bytes run under stock `hurl` and under proef (pinned
      by `crates/proef-cli/tests/fragments.rs`, both runners against the fixture).
      `[run] fragments` names the scanned root; PRD §3's hurl non-goal was narrowed to
      *generation* in the same change. Inline `hurl: |` is unchanged and permanent.
      A `ref:` step records the fragment it ran as `file.hurl#name`, which `explain`,
      the console, TAP, JUnit, the GitHub summary and the HTML report all name on a
      failure (ADR-0018 accepted three-files-per-test only on condition tooling earned
      it back); the editor completes `bind:` keys from the fragment's own placeholders
      and jumps from `ref:` to the annotation. The corpus is read once per command and
      scanned lazily, at most once — `[run] fragments` resolves to an absolute path
      because `proef lsp` keys document identity on one, and the shortening that makes
      a record portable happens at the naming boundary, never at resolution.
      Breaking (library): `pack::load` takes a `&FragmentCorpus` (scans on first use),
      `PackSet::fragments` is an `Arc`, `LoweredScenario::secrets` is a map,
      `Prepared`/`ScenarioCtx` carry `secret_bindings`, and `LoweredStep`/`StepOutcome`/
      `Event::StepFinished` carry `fragment`
- [x] adoption response — the gaps a 97-entry corpus port hit, plus the scheduling
      primitive its suite needed. `proef fragments` lists the corpus and names both ways
      a fragment dies (no macro refs it; only a macro no scenario binds does), with
      unannotated entries by line and a `--check` gate (`--require-annotated` opts into
      failing on those, since an unannotated entry is inert *by design*). A `bind:` key
      nothing reads is refused with did-you-mean (`pack::unread_bind_key`), checked as a
      union over the scope so a pack-scope key serving one macro stays correct. `doctor`
      reports the corpus, `init` scaffolds both body forms, and `--config <path>` names
      the `proef.toml` to read (discovery only searches *up*, so a config beside the
      suite was unreachable). ADR-0007's value caps now apply to fragment text — they
      were inline-only, so byte-identical `[Options]` exited 2 in a `hurl:` block and 0
      behind a `ref:`. `[run] exclusive-tags` runs a matching scenario with the pool to
      itself (exclusion, not ordering — `[run] setup` already covers *before*).
      Breaking (library): `FragmentScanner` returns `ScannedFile { fragments,
      unannotated }`, `AnalyzeCtx` takes the corpus rather than building one (the LSP
      was re-reading and re-hurl-parsing it per request), `StepKindSpec` carries an
      `options` recogniser so option *spellings* live only in the engine that owns them,
      and `ScenarioSpec` carries `exclusive`
- [x] 0.11.1–0.14.0 — released hardening, then CI scale. 0.11.1 closes 0.11.0's
      gaps (output paths create the directories they name; `fragments` counts
      `[run] setup`/`teardown` usage; one `FragmentSupport::claims` predicate;
      `--config` reaches `lsp` and `--watch`). 0.12.0: the **one path rule**
      (a path written in `proef.toml` resolves against the config's directory,
      a flag against the cwd) and the watch feedback/staleness class closed for
      good (reruns register where they write, config is reread and matched by
      canonical path). 0.13.0: a record that travels — encoded reflections of
      secrets are redacted (base64/hex/percent/JSON-escape needles, ADR-0005
      amendment), the fragment corpus read is bounded, records stop naming the
      machine, `diff` takes a path, `[run] keep-runs` rotation. 0.14.0: CI
      scale — `--shard I/N` (frozen-hash bucketing), `--max-fail N`, `--rerun`
      continues a cancelled run instead of reporting a false green,
      `proef flaky` verdicts over run history, and the rendered docs site
- [x] validation rounds 17–18 + the Robot Framework capability audit
      (0.15.0) — two external review rounds validated claim-by-claim (the
      shard hash gained fmix64 after round 18 *proved* the parity collapse the
      round-17 refutation was blind to; every matrix re-deals), then a deliberate
      RF 7.x mining shipped in three waves: detail caps at the engine boundary,
      tag-atom globs, `flows` feature descriptions, `--shuffle` (seeded by the
      run id), `reproduce_hint` into the record; `@skip`/`@skip:reason` with
      reasons in every sink and the authored/mechanical split `--rerun` keys on
      (ADR-0019, quarantined failures now reach JUnit as skipped-with-message);
      tags + `exclusive` into the record with per-tag tables in the HTML report
      and GitHub summary; explicit run metadata `--meta`/`[meta]` +
      `run_started.env`/`shuffled` (ADR-0020 — harvested-vs-handed-over is the
      boundary); the rerun overlay (`rerun_of`: one JUnit and one report cover
      the whole suite, composition never a merged record); `--console
      dotted|quiet`; `[tag-links]`. Event schema stays 1 throughout —
      everything additive. Breaking (library): the JUnit identity change plus
      field additions across `ScenarioSpec`/`ScenarioOutcome`/`Event`,
      `render_html`/`write_junit`/`ConsoleReporter::new` signatures
- [x] the deep improvement report — waves 1–8 (#112–#142), one programme run
      to exhaustion: wave-1 correctness (#112–#116); CI-sink conformance —
      JUnit detail into element content, an XML-1.0 control-char boundary,
      real limits on the GitHub sinks (#118); UX — console colour, completions
      and a man page in every archive, a project-aware `doctor`, a linkable and
      filterable HTML report, and the **`--format` / `-o` split** (#119–#122,
      breaking); diagnostics — carets on the defect, parser prose from the
      parsers, `proef.toml` inside the diagnostic system, codes that lead
      somewhere (#123–#125); docs and distribution — an install page, `.sha256`
      sidecars, a README a prospect can run (#126–#128); the LSP wave —
      `lsp-types` → `gen-lsp-types`, one analysis per edit rather than per
      keystroke, quick-fix code actions off a structured `Diag::fix`, document
      symbols, hover, and a panic guard on both message-loop entry points
      (#129–#132, the analysis cache landing as #146 after a `--delete-branch`
      auto-closed its original); features — quarantine's own two failure modes,
      a whitespace-only diff note off hurl's structured `actual`/`expected`,
      `proef schema config`, `flaky --by` (#133–#137); performance and fuzz —
      pack validation made linear (65× at 3200 macros), a `--tags` expression
      that aborted the process on a stack overflow, the last unfuzzed parser
      (#138–#141)
- [x] round 19 (#143–#145, #147–#150) — the shape the earlier waves
      missed: the data model held the right information and the **surfaces**
      lost it. A step's authored
      `name:` reached the artifact and nothing else, so one sentence lowering to
      several engine steps printed as identical rows — the pinned event snapshot
      had been encoding the defect (#143). `proef report -o` wrote the author's
      home directory into the one output built to be shared, `--skip` failed
      WCAG AA (and every dark-mode pill did), and the page had one heading
      (#144). `explain`/`diff`/`doctor` gained `--format json`, so no consumer
      re-derives the fold proef's own two copies disagreed on three ways (#145).
      The worklist itself was stale in five places, including a "~290 lines of
      hurl grammar in core" that measures 19 in production (#148). Then
      `--console failed` (#147), redaction moved out of the reporter mutex
      (#149), and the 256 MiB record ceiling reached the two of four readers it
      had missed (#150). Breaking (library): `StepOutcome` and
      `Event::StepFinished` gain `label`; event schema stays 1 (additive)
- [x] ADR-0002's grammar boundary (#153–#155) — "core stays free of engine
      *types*" was always true; "free of engine *syntax*" never was. Measured:
      twelve literals across four files, sanctioned and closed by an ADR-0002
      amendment plus a guard that lexes whole files. The three items filed as
      charter questions turned out to need no ADR at all, and two had the wrong
      governing principle attached
- [x] the 0.16 survey (#156–#158 + the report ranking) — six findings validated
      against the tree and shipped, and **two of the six had premises that did
      not survive validation.** `[http]` gained the settings that describe an
      *environment* — `insecure`, `proxy`/`no-proxy`, `cacert`, client
      cert/key, `max-redirs`, `user-agent` — where before a self-signed staging
      cert or a corporate proxy could only be said by repeating `[Options]` in
      every macro; `insecure = true` warns every run naming the profile, a
      `client-key` without its cert is exit 2, and credentials stay in the
      secret store. **23 of 75 diagnostic codes had no test at all** (the
      catalogue itself measured exactly honest — 75 defined, 75 documented,
      zero drift), closed by 19 tests and a fifth `source_guards` rule. #138's
      "4× per doubling before, ~2× after" became a ratio test, and that test
      then had to be fixed: it read 2.05× alone and **3.09× under nextest's
      parallelism**, so timing assertions now run alone (`just perf`, its own CI
      step) and TESTING-STRATEGY §7 says why. The LSP tells the two variable
      tiers apart — `${…}` as `macro`, `{{…}}` as `variable` — which is the
      ADR-0005 distinction no generic grammar can see. `lower.rs` stopped
      threading a mutable trio through twelve functions (arity suppressions
      13 → 6, `lower.rs` at zero) — *not* by hoisting state into a `self`,
      which would have broken the borrow discipline the threading exists for.
      `--shard-weights` balances a matrix by measured duration from one shared
      `timings.json`; the natural per-machine design would have run scenarios
      twice or not at all while reporting green. And the HTML report finally
      answers "what is slowest", with the share of run time it accounts for.
      Breaking (library): `HttpDefaults` gains eight fields and loses `Copy`
- [x] the 2026-09 survey wave — a check-the-world round (upstream releases,
      standards movement, the worklist itself), validated then implemented:
      `[http] cookie-store = false` (hurl 8.0's `--no-cookie-store`; the one
      `[http]` key with no per-entry `[Options]` spelling at all, and proef is
      structurally immune to hurl's own on→off handle FIXME because clients
      are per-batch); `--ctrf` (CTRF JSON off the same fold as JUnit —
      quarantine parity per ADR-0019, flaky passes carry real
      `retryAttempts`); a disk filling *mid-run* reaches exit 3 (the deferred
      v0.6–v0.8 console-latch item, to its own written design);
      `emit::feature_stem`/`artifact_slug` define naming once (Q6 closed
      structurally; Q2 found already closed by the #146 cache). Validated as
      needing nothing: the hurl 8.0.1 pin is current, Rust 1.98.1 does not
      exist yet (policy adopts at `x.y.1`), notify 9 is still RC, and release
      engineering already ships attestations/tap/binstall. Breaking (library):
      `HttpDefaults` gains `cookie_store`
- [ ] M6 — future engines (none scheduled; acceptance: zero `proef-core` diff)

Milestone detail, acceptance criteria, and the definition of done: `docs/IMPLEMENTATION-PLAN.md`.
