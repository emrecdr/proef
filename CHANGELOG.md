# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning follows [SemVer](https://semver.org) (policy in `docs/RELEASING.md`).

## [Unreleased]

### Added

- `proef secret rm NAME` removes a stored secret (locked atomic rewrite;
  removing an absent name exits 2).

## [0.3.0] - 2026-07-29 (data-safety blockers, Then visibility, taxonomy)

### Fixed (v0.2.1 review — every finding reproduced before fixing)

- **Asset copy destroyed user files**: `proef artifacts -o` pointing at the
  suite truncated referenced assets to 0 bytes, and `..` references escaped
  the output directory. Copies now refuse absolute/`..` references (exit 2),
  never copy a file onto itself, and surface IO errors (exit 3).
- **Run rotation deleted arbitrary directories**: with `runs-dir` shared with
  user content, rotation could recursively delete user directories — and its
  own in-flight run. Only uuid-named run records rotate now, never the live
  run, and rotation happens before the new run dir exists.
- **Zero-entry payloads passed silently**: a comment-only `hurl:` block ran
  nothing while the scenario reported green. Load-time lint rejects it;
  the engine backstop emits Skipped outcomes for anything that slips through.
- `proef flows … | head` (and every other command) tolerates a closed pipe;
  a non-UTF-8 environment variable no longer aborts any command.
- Raw `[Options] retry:`/`repeat:` values are parsed and capped (10000), and
  `delay:` is capped at 1 hour in both typed and raw forms; `repeat:` now
  counts toward the batch budget so long repeats aren't blamed on the
  environment.
- Concurrent `proef secret set` calls no longer lose keys (advisory-locked,
  atomic 0600 temp+rename store; the key-creation race resolves to the
  winner's key). `proef fmt` and `schema --add-to` write atomically.
- `proef fmt` keeps fenced body bytes verbatim (blank lines and trailing
  whitespace inside ``` fences are the bytes the test sends).
- Nested suites now load their packs: pack discovery recurses like feature
  discovery (`packs/` directories at any depth); `proef fmt` shares the rule.
- Duplicate/empty Examples header columns are a named error instead of a
  silent last-value-wins; an empty `.feature` gets a plain-language error;
  a UTF-8 BOM is stripped instead of shifting every diagnostic span.

### Changed

- **Then steps are visible everywhere**: `expect:` macros now surface as
  their own step rows in console, events, JUnit, and `explain`, with assert
  failures attributed to the authored `Then` line — the host request no
  longer inherits its followers' assert failures. Artifact bytes are
  unchanged; sidecars gain one row per `Then` (schema-compatible).
- **Error taxonomy**: mistakes in the test's own text (undefined `{{var}}`,
  bad JSONPath/regex/URL/options, unreadable body file) exit 2 instead
  of 3, anchored on hurl's own assert-context flag.
- `when:` guards skip on a literal `false`/`0` as well as empty — an author
  writing `when: ${flag}` with `flag=false` means skip.
- `proef.toml` is no longer gitignored (it is documented, committed project
  config).
- proef-core public API: removed dead surface (`NormalizeReporter`, the
  never-populated `config` resolution tier, `StepOutcome.artifact_span`,
  `LoweredStep.retry`, `StepKeyword`, and friends); added
  `EngineErrorClass::UserInput`, `StepPayload::MergedAsserts`,
  `ScenarioOutcome.artifact_slug`, `Guard::skips`.

## [0.2.1] - 2026-07-29 (review P0 + failure UX)

### Fixed

- `[Options]` header detection follows hurl's token grammar — the injection
  can never land inside XML/JSON/prose bodies (class closed, unit-tested).
- `proef artifacts` survives a closed pipe (exit 0, best-effort writes).
- `--dry-run` honors `--scenario`/`--tags` with the same zero-match exit 2.
- Duplicate scenario names dedup feature-wide (`#N`): unique artifacts,
  console buffers, and events — no silent overwrite.
- `.proef-secrets.json` is created `0600`, gitignored, and documented.

### Changed

- Failure details surface hurl's computed expected/actual (`fixme`) anchored
  on the error's own artifact line, not the entry's first line.
- GETTING-STARTED uses `PROEF_BASE_URL`, points the reader at a runnable
  target, and frames sample output honestly.

## [0.2.0] - 2026-07-29 (correctness, output contract, author docs)

### Fixed (v0.1.0 deep-review follow-up — all three blockers reproduced first)

- `[Options]` injection is body-fence-aware: a `retry:`/`delay:` step whose
  body contains method-looking lines no longer gets options spliced into the
  body it sends.
- Step↔entry correlation is a partition anchored on each entry's request
  line: a comment-only step can no longer cause the next request to be sent
  twice (one authored POST is one POST, asserted via the event stream).
- `delay:` joins the watchdog budget (with saturating duration math
  throughout), so delayed steps are no longer killed as system errors;
  `retry.count` is capped at 10000 by the pack lint.
- A panicking scenario thread is contained (`catch_unwind`), reported as a
  System fault under its real identity immediately — never a budget timeout;
  abandoned scenarios keep their real file/name/line; steps in batches never
  reached report `Skipped` instead of vanishing from every report.

### Changed

- Output contract: `--output json` owns stdout exclusively (human report on
  stderr — pipeable into `jq`); `StepFinished` events carry a `detail`
  failure field (additive); `optional:` failures report `Warned` everywhere
  consistently; engine failure details use hurl's own error descriptions
  instead of Rust `Debug`; diagnostics drop ANSI when stderr is not a
  terminal; a filter selection matching nothing exits 2; failure output
  prints a ready-to-run `reproduce: hurl …` line; the artifact replay header
  names required `--secret` placeholders; the undocumented `.env` autoload
  was removed.

### Added

- Author-facing documentation: `docs/GETTING-STARTED.md` (first suite in ten
  minutes) and `docs/AUTHORING.md` (the full pack/feature reference).
- Mechanical alignment gates: `xtask docs-check` (crates and ADRs must appear
  in their indexes) runs in PR CI; `xtask public-api` snapshots
  `proef-core`'s public API surface (1.4k items) and fails CI on unreviewed
  changes — the mechanical form of the zero-core-diff invariant.

## [0.1.0] - 2026-07-29

Initial release.

### Added

- **Authoring:** Gherkin `.feature` files in plain business prose; YAML macro
  packs bind prose to executable steps via `match:` patterns, typed params,
  defaults, `use:` composition (cycle-checked), `expect:` assert-only macros,
  `optional:`, finite `retry:`, `delay:`, `when:` guards, and `saveAs: global`
  promotions.
- **Validation:** `proef test --dry-run` binds, lowers, emits, and
  parse-validates every scenario without touching the network; stable
  diagnostic codes with source-span rendering; a seeded error corpus pins
  every code; pack payloads are validated at load by the engine that claims
  them.
- **Execution:** the hurl engine runs artifacts in-process (exact-pinned
  `hurl 8.0.1`); contiguous same-engine steps batch maximally; variables and
  cookies chain across batch splits; per-entry `[Options]` override batch
  defaults; finite budgets with a watchdog bound every scenario; Ctrl-C
  cancels gracefully (twice = hard exit); parallel scenarios share a typed
  World with write-set-only merge-back and a persistent global store.
- **Artifacts as the contract:** every scenario emits canonical `.hurl` text
  that is byte-identical to what the engine executes, plus a sidecar map
  (entry ↔ feature anchors, explicit batch/step indices), `.vars`, and any
  referenced file assets — replayable with stock `hurl --test`.
- **Record & reporting:** a versioned JSONL event stream is the run record
  (live per-entry progress included); console BDD tree; JUnit XML; GitHub job
  summaries; `proef explain` replays the record; secrets are encrypted at
  rest, injected via hurl's redaction, and value-redacted once at the event
  sink — never present in artifacts, events, logs, or reports.
- **Tooling:** `proef flows`, `artifacts`, `schema` (merged JSON Schema with
  editor modelines), `secret set|list`, `fmt` (canonical hurl blocks),
  `doctor`, `--watch`; a libtest-mimic harness exposes one test per scenario
  to nextest/IDEs; `${fake:*}` deterministic synthetic data seeded from the
  run id.
- **Quality gates:** unit + property tests, fuzz targets, insta snapshot
  corpus (artifacts, diagnostics, events), fixture-server integration suite,
  assert_cmd CLI/exit-code suite (0/1/2/3 contract), cargo deny/machete/
  zizmor in CI, cargo audit nightly, a scheduled canary against the next
  hurl release, and CI on Linux, macOS, and Windows.
- **Distribution:** tagged releases build five targets (macOS arm64/x86_64,
  Linux arm64/x86_64-gnu, Windows x86_64-msvc) with `cargo auditable`, ship
  a Homebrew tap formula and a `cargo binstall`-compatible layout, and attest
  SLSA provenance once the repository is public.
