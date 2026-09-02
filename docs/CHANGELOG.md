# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning follows [SemVer](https://semver.org) (policy in `docs/RELEASING.md`).

## [Unreleased]

### Added

- **The HTML report answers "what is slowest".** After "what failed", it is the
  question a test report is most often asked, and the page could not answer it:
  the timeline showed *that* workers were busy, never *which* scenarios to
  attack. Every number needed was already in the fold.

  A ranked section, slowest first, each row linking to its own block, with the
  heading reporting the **share of run time the listed scenarios account for** —
  "3 of 40 · 71% of run time" is a decision, where a column of durations is
  homework. Capped at eight: a ranking long enough to scroll has stopped
  answering the question.

  Cost is the **sum of a scenario's step durations**, the same definition
  `timings.json` uses for shard weights — one notion of what a scenario costs
  across the whole tool. Not the wall-clock span, which includes time waiting
  for a worker: a property of how the run was scheduled, and not something the
  reader can go and fix.

  Absent when there is nothing to rank — fewer than two timed scenarios, or a
  record with no injected durations at all.

- **`--shard-weights` balances a shard matrix by measured duration.** `--shard`
  assigns by a frozen hash, which guarantees that adding one scenario never
  re-buckets the others but cannot balance by *time* — and a CI matrix finishes
  when its slowest shard does, so a count-split routinely leaves runners idle.
  Every run now writes a small `timings.json` into its run directory; CI
  archives that one file and each matrix job points `--shard-weights` at the
  same copy.

  **The obvious design is silently wrong, and the module says so at length.**
  proef already retains records carrying every step's duration, so "weight by
  the newest local record" looks free. But matrix jobs run on *different
  machines*, each with its own (usually empty) `runs-dir` — every job would
  compute a different weight table, therefore a different assignment, and
  scenarios would run twice or not at all while the suite reported green.
  Nothing about that announces itself. One named file shared by every job is
  what makes the split a pure function of (selected scenarios, that file).

  Two rules place scenarios and they **partition** rather than compete: a
  scenario the file mentions goes through longest-processing-time-first
  placement, and one it does not mention falls back to the frozen hash. So a
  test added after the timings were captured still runs exactly once. That is
  pinned by a test that runs a whole three-way matrix — with a weights file
  covering only five of nine scenarios, so both rules are exercised at once —
  and asserts set equality both ways; mutating the placement by one bucket drops
  two scenarios and the test names them.

  The weight is the **sum of a scenario's step durations**, not its wall-clock
  span. The span includes time spent waiting for a worker, which is a property
  of the run's scheduling rather than of the scenario, and feeding it back would
  let one crowded run's queueing distort the next split.

  What this gives up is exactly what hash mode was chosen for: a balanced split
  is not stable under insertion. That is what balancing means, which is why the
  flag is opt-in. A missing or malformed weights file is exit 2 — falling back
  silently would hand back the unbalanced split the flag was passed to avoid.

### Changed

- **`lower.rs` stops threading the same three values through twelve
  functions.** `out`, `refs` and `sinks` travelled as separate parameters
  everywhere, and five functions — `expand_macro`, `expand_step`,
  `expand_ref_step`, `expand_payload_step`, `finish_step` — carried 8 to 11
  parameters each behind individual arity suppressions. Adding one piece of
  lowering state meant editing five signatures and five call sites, which is the
  shape of change that drops a parameter at one site.

  Two bundles, both of them types that were already implied by the code:
  `Emit { out, refs, sinks }` (the mutable outputs, always passed together and
  never independently), `StepScope { step_ref, ctx, at }` (what stays fixed for
  one authored step however deep expansion recurses), and a small `Finished`
  for the four values that describe a step being completed.

  **What was *not* done matters as much.** The obvious refactor — hoist the
  state into a `self` and make the five methods — would have broken the reason
  they are parameters at all: `resolve_in` and friends take them explicitly so
  they remain callable while other state is mutably borrowed, and a method on
  `&mut self` cannot be called while `self` is borrowed elsewhere. The threading
  discipline is load-bearing, so it stays; only the arity changes.

  Arity suppressions across the workspace: **13 → 6**, with `lower.rs` at zero.
  No behaviour change, and the 241 core tests say so.

### Added

- **The editor tells proef's two variable tiers apart.** A pack's `hurl: |` block
  is the centre of the authoring experience and, to every editor, a plain YAML
  scalar — inside which `${…}` (resolved at **lower time**, by proef, before any
  request exists) and `{{…}}` (resolved at **run time**, by hurl) look
  identical. That distinction is ADR-0005's whole model and the thing authors
  most often get wrong, and no generic grammar can see it: a YAML highlighter
  sees a string, and a hurl highlighter never runs because the block is not a
  file. proef is the only party that knows.

  The server now answers `textDocument/semanticTokens/full`, lighting `${…}` as
  **macro** — a substitution performed before execution, which is what a macro
  is — and `{{…}}` as **variable**. Both are coloured differently by every
  mainstream theme, so it works without anyone configuring anything. The `$${`
  escape stays dark, because telling an author proef will substitute text it
  will in fact leave alone is worse than no highlighting.

  The `${…}` scan is `proef_core::resolve::reference_spans`, walking the same
  `first_reference` the resolver itself uses — a second implementation of the
  escape rule would drift, and the drift would show as an editor confidently
  colouring literal text. The `{{…}}` scan lives in `proef-lsp` rather than
  core, because that spelling is the engine's and ADR-0002's amendment is that
  engine syntax does not accumulate in the core.

  Collapsing the seven-arm request dispatch behind a local macro came with it:
  the chain crossed clippy's line limit the moment an eighth feature landed, and
  the honest fix was to stop repeating an identical frame seven times rather
  than to suppress the lint that noticed.

### Fixed

- **The complexity guard added moments earlier was itself flaky, and now runs
  alone.** It shipped in the ordinary suite on the reasoning that a *ratio*
  cancels out runner load. Measurement disagreed on its second full-suite run:
  **2.05× isolated, 3.09× under nextest's full parallelism**, against a bound of
  3.0. The larger input has the larger working set, so memory-bandwidth
  contention penalises it more than the smaller one — the ratio drifts rather
  than cancelling, and interleaving the samples cannot fix a systematic effect.

  nextest's `test-groups` bound concurrency *within* a group and do not isolate
  one from the rest of the suite, so the only mechanism that actually delivers
  isolation is `#[ignore]` plus a dedicated invocation: a CI step of its own and
  `just perf`. The samples are interleaved as well, which removes the one skew
  that ordering alone creates.

  `TESTING-STRATEGY.md` §6 previously asserted the opposite in as many words —
  that a ratio "survives a shared runner" — and is corrected with the numbers.
  The claim was reasoning, not measurement, which is the failure this whole
  section of the changelog exists to record.

### Added

- **The linear-validation claim is now a test, not a sentence.** #138 made pack
  validation linear and recorded the result as a shape: *"the curve changed
  shape — 4× per doubling before, ~2× after"*. That number lived only in the
  changelog, where nothing could re-run it — so a future span locator scanning
  the whole pack file again would have restored the quadratic behaviour
  silently, a regression that costs seconds rather than correctness and which no
  gate measured.

  The guard asserts the **ratio** between 1000 and 2000 macros, because the
  claim *is* a ratio. It observes ~2.05× against a bound of 3.0; mutating
  `locate::MacroIndex` to re-index per lookup — the exact pre-#138 shape —
  measures **4.01×**, matching the changelog's own prediction of 4× and turning
  a 0.4-second test into a 73-second one. The failure message names the cause
  rather than reporting a number.

  A ratio rather than a benchmark, for a reason now written into
  `TESTING-STRATEGY.md` §6: load on a shared runner inflates both measurements
  together and cancels, where an absolute threshold has to be loosened until it
  means nothing. `iai-callgrind` would be the better CI gate — instruction
  counts ignore runner noise entirely — but it needs valgrind, so it would be a
  gate the maintainer cannot reproduce on macOS; `criterion` and `divan` sit in
  the same noise regime as this test while adding a dependency tree to a
  workspace that audits every edge. No new dependency was added.

- **Every diagnostic code is now named by a test, and a guard keeps it that
  way.** `DIAGNOSTICS.md` calls codes "a contract: they never change meaning".
  Twenty-three of seventy-five had nothing holding them to it — reachable in
  production, documented, exercised by nothing at all: not a seeded corpus
  directory, not a unit test, not even an assertion on their message text. They
  existed only at their definition site.

  The catalogue itself was found *exactly* honest — 75 codes defined, 75
  documented, and its corpus column matched disk in both directions with zero
  drift. The gap was never documentation; it was that a documented promise had
  no enforcement.

  Nineteen new tests close it, each reaching its code through a real path rather
  than constructing the diagnostic directly. Two of them exercise guards that
  are unreachable in normal operation and were therefore the most valuable to
  test: `lower::kind_unrouted` fires only when the engine registry and pack
  validation disagree, so the test makes them disagree on purpose; and
  `lower::expansion_too_deep` sits behind pack validation's identical depth
  limit, so the test bypasses validation with `load_collecting` — the only way
  to hand lowering a graph validation would have stopped, and therefore the only
  way to prove the second line of defence is still there.

  Two codes are exempted by name, with reasons recorded in the guard:
  `source::unreadable` and `config::unreadable` need a file the process may stat
  but not read, a permissions state CI runners do not reproduce because they run
  as root. The guard also checks its own exemption list, failing if an exempted
  code is deleted or renamed — an exemption that outlives its code silently
  excuses nothing.

  The guard joins the four in `source_guards.rs` and is mutation-verified:
  rewriting one test to match a code by *suffix* instead of naming it turns the
  guard red, which is the point — a test that matches the prose pins the
  wording, and only one that names the code pins the contract.

- **`[http]` now carries the settings that describe an environment: TLS, proxy
  and mTLS.** The table exposed two of hurl's runner options — `timeout-ms` and
  `follow-location` — while the embedded engine has supported the rest all
  along; `TECH-SPEC.md:235` even listed `insecure` among what `RunnerOptions`
  carries. So a suite that had to run against staging's self-signed certificate,
  or through a corporate proxy, or against an mTLS-protected API, could not say
  so anywhere: the only route was repeating an `[Options]` block inside every
  macro's raw hurl, which defeats environment profiles exactly where they are
  most useful, since these settings *are* the difference between environments.

  Eight new keys — `insecure`, `proxy`, `no-proxy`, `cacert`, `client-cert`,
  `client-key`, `max-redirs`, `user-agent` — each merging field-wise through the
  existing `[http]` < `[env.<name>.http]` chain, so a staging profile turns
  verification off without production inheriting it. No new concept: only more
  of one that already worked.

  Three deliberate edges. **`insecure = true` warns on every run**, naming the
  profile that set it — a suite that goes green without verifying a certificate
  has not proved what a green suite normally proves, and since the run record
  carries no config by design, the warning is the entire audit trail. **A
  `client-key` without a `client-cert` is exit 2** rather than a pass-through:
  libcurl accepts the pair and then presents nothing, so the failure would
  otherwise surface at the *server* as an authentication error naming nothing
  about the cause. And **credentials are excluded on purpose** — there is no
  `user` or `netrc` key, because a password belongs in the secret store where it
  is encrypted at rest and masked out of every sink.

  The three path-valued keys resolve against `proef.toml`, the one-path rule
  every other config path follows; core still reads no filesystem and receives
  them already resolved (ADR-0012). Each option is applied to hurl's builder
  only when actually set, so a project with no `[http]` table runs
  byte-identically to one built before the keys existed — pinned by a test.
  Per-entry `[Options]` still override all of them except `user-agent`, for
  which hurl has no per-entry option at all; that exception is documented rather
  than papered over.

  Breaking (library): `proef_core::engine::HttpDefaults` gains eight fields and
  **loses `Copy`** — it now carries `String`s. `Default` stays hand-written, and
  the reason is now stated in the type: a derive would make `timeout_ms` zero,
  which libcurl reads as *no timeout at all*, silently converting ADR-0007's
  budget into an unbounded wait at every existing `default()` call site.

### Changed

- **ADR-0002 now names the core's hurl entry grammar, and a guard keeps it
  closed.** "Adding an engine leaves `proef-core` diff-empty" was true of
  engine-*types* and never of engine-*syntax*: the core does text surgery on
  entries — splicing `[Options]` in, merging an `expect:` block's asserts into
  the previous entry — so it has to find an entry boundary in text hurl will
  later parse. The worklist carried the gap for two rounds as "~290 lines of
  hurl grammar in core", a figure that counted `#[cfg(test)]` fixtures.

  Measured: twelve literals across four files. Seven the core *writes*, four it
  *recognises* to find a boundary, and one it *quotes* — a hurl snippet inside
  a did-you-mean help string in `bind.rs`, which generates nothing and parses
  nothing but drifts like any other copy. The four boundary recognisers are
  already one shared `pub(crate)` set. proef's own pack keys are shaped like
  option lines and are excluded by name rather than listed as sanctioned rows.

  The guard lexes whole files. The first version scanned line by line and so
  could not see a literal that spans lines — which is where a *larger* piece of
  engine syntax would naturally be written, and where the one entry above that
  nobody had counted was in fact sitting.

  The amendment sanctions that set and closes it. Deferred with a named
  trigger — a second engine being scheduled — is moving the written half behind
  the seam, where the *reading* half already lives:
  `StepKindSpec::options` exists precisely so an engine's option spellings stay
  out of the core, and it covers recognising them only, so `retry:`,
  `retry-interval:`, `delay:` and `variable:` are still core literals. Until a
  second engine exists that migration relocates seven literals that exactly one
  implementation will ever supply, at the cost of a public-API break.

  `crates/proef-cli/tests/source_guards.rs` (renamed from `stderr_hygiene.rs`,
  which had not been only about stderr for two rules now) pins the set: a new
  token, or an existing one spreading to another core module, fails the test
  and names both remedies. A claim of this shape decays the moment it is only
  prose — this one already had, by an order of magnitude, in the direction that
  made it look worse than it is.

## [0.16.0] - 2026-08-31 (the surfaces tell the truth: an eight-wave improvement programme, and the round that found what it missed)

> Supersedes **0.15.0**, which was cut (`release: v0.15.0`, 2026-08-25) but never
> tagged or published — its changes are all here, and crates.io goes 0.14.0 →
> 0.16.0 with nothing skipped.

### Fixed

- **The record-size ceiling reached two of its four readers.** 0.13.0 bounded
  the run-record read at 256 MiB because records travel — `diff` reads a
  downloaded baseline, `flaky` reads every retained run — and the read, the
  line split and the parsed `Vec<Event>` are resident at once, so a corrupt or
  hostile file was an OOM rather than an error. The bound lives in
  `record::read_events`, and `explain` and `report` each opened
  `events.jsonl` with a bare `read_to_string` instead, so neither had it.
  `report` even used the guarded reader for the *base* record two dozen lines
  below the raw read of the primary one.

  Both now go through `read_events`, which returns the parsed events — exactly
  the read-once/parse-once its own comment asked for. A source-scanning test
  makes the next reader go through the same door, the shape this project
  already uses for the raw-print and malformed-plural rules: a guard added in
  one place and left for the next call site to rediscover is how it went
  missing the first time.

### Added

- **`explain`, `diff` and `doctor` speak `--format json`.** They were the three
  commands with no machine output, and the three a consumer reaches for
  *after* a run. A run directory is `artifacts/ + events.jsonl + run.log` and
  carries no structured summary, so anything analysing a run it did not launch
  — a CI job reading another job's artifact, a script, an agent — had to fold
  `events.jsonl` itself. That is the fold proef's own two internal copies
  disagreed on three ways before `report::suite_totals` unified them; handing
  the canonical answer over is cheaper than inviting everyone to re-derive the
  one proef got wrong.

  Each object mirrors its prose field for field rather than modelling a richer
  view — the prose is the contract a reader already knows, and a machine
  surface that says something *different* is a second answer to one question.
  `diff`'s `flaky`/`slower` stay the rendered sentences for the same reason.
  The flag is the existing single-variant `json` enum the listing commands
  already use, renamed from `ListFormat` to `JsonFormat` now that it serves
  non-listing commands too. Machine mode owns stdout: notes whose content the
  object already carries are suppressed rather than repeated on stderr.

  `doctor` needed a real change to get there — it printed each check as it ran,
  so the verdict was the only thing a caller could see. Checks are collected
  before rendering now, which makes the JSON a second *rendering* rather than a
  second walk: the failure mode where one surface gains a check the other never
  learns about.
- **`--console failed`** — the `full` BDD tree, but only for scenarios that
  failed or warned. A clean run prints the run line and the summary; a dirty
  one prints exactly what `full` would. The gap it fills is the CI one:
  `full` is a wall of green on a large suite, `dotted` drops the detail you
  need when something breaks, and `quiet` drops everything.

  Warned scenarios are shown, which the name does not say and the code
  explains: a warned scenario is one whose `optional:` step failed,
  `RunSummary::passed` counts it with the passes, and the summary line has no
  warned column — so a mode that showed only `Failed` would let a run in which
  something *did* fail print exactly what a spotless one prints. A fourth
  value on the existing flag rather than a new one.

### Fixed

- **`cargo deny` failed on a yanked transitive crate.** `rand 0.10.2` resolved
  `chacha20 0.10.1`, which was yanked from crates.io; the lock now takes
  `0.10.2`. Not the secret store's copy — `chacha20poly1305` pins `0.9.1`,
  which is unaffected — so nothing about encryption changed. Found by the
  gate, which is what it is for.

- **`proef report -o` wrote the author's home directory into the file built to
  be shared.** With the report inside the run dir the artifact links are a
  bare `artifacts/…`; with `-o` pointing anywhere else they were made
  *absolute*, which resolves only on the machine that produced them — and `-o`
  exists to put the report somewhere it will be published, which is exactly
  where that path is dead. 0.13.0 scrubbed machine identity out of the run
  record (R12-1); this put it back, twelve times over, in the HTML uploaded
  beside it. The href is now relative to the report, which resolves everywhere
  the absolute one did *plus* wherever report and artifacts travel together,
  and in the CI shape (`-o public/report.html`) names nothing outside the
  workspace. The href is built from path *components* joined with `/`, not
  from `Path::display` — Windows renders `\`, which is not a separator in a
  URL, so a Windows-generated report's links would have been dead either way
  (the absolute path it replaces had the same flaw). A report written somewhere
  sharing no ancestor with the run dir
  still names the directories between them — that is what a correct relative
  path from there is, and it is no worse than what it replaces.

- **The report's `--skip` colour failed WCAG AA, and every status pill failed
  it in dark mode.** `--skip` was the one palette token the dark block did not
  redefine: a grey chosen against `#0d1117` (5.48:1 there) left carrying white
  text on white at **3.45:1**, against a 4.5:1 threshold — on the status a
  reader scans for after an interrupted run. It is now `#59636e` (6.11:1).

  Writing the guard rather than the fix found a second defect nobody had
  measured: `.pill` painted `color:#fff` on the status colour, and the dark
  palette's colours are tuned as *text* on a dark ground, so all four dark
  pills sat between 2.52:1 and 3.45:1. The pill foreground is now a palette
  token — white on light, the page ground on dark — putting all four between
  5.48:1 and 7.5:1. A test asserts the ratio rather than the hex, so a future
  palette change is free to move a colour and not free to move it below AA,
  and a second test pins that both palettes define the same token set (the
  absence that caused this).

- **The HTML report had one heading and no outline.** The timeline carried an
  `<h2>`; the tag table and the scenario list — the body of the page — had
  none, so there was nothing to navigate by and no anchor to link a section
  with. Both gained one, sharing the class the timeline already used (renamed
  from `.timeline-h` to `.section-h`, since it now serves three). Pinned
  structurally, so a section added without a heading fails the test.

- **A step's `name:` label reached the artifact and nothing else.** A macro
  with more than one step turns one feature sentence into several engine
  steps, and they share a `StepRef` exactly — same file, same line, same
  text. The emitter has always written the authored `name:` into the
  artifact's entry comment, which is why the `.hurl` could tell them apart;
  `StepRef` never carried it, so the console, the HTML report, `JUnit`, TAP,
  the job summary and `explain` all printed the same sentence once per step,
  with nothing but the status glyph to distinguish a warning from the failure
  beside it. The reference corpus demonstrated it: three `step_finished`
  events for *the cookie session is exercised*, byte-identical in the pinned
  snapshot, are now `obtain the session cookie`, `optional probe (forces a
  split)` and `cookie survives the split`.

  `StepOutcome` and `step_finished` now carry `label`, exactly as they carry
  `fragment` — the two answer neighbouring questions (*which file did this
  request come from* / *which step of the sentence is this*) and travel the
  same channels. One `proef_core::report::step_label` renders it for every
  sink, so the six cannot drift. Additive on the wire: absent when a step has
  no `name:`, so every pre-existing record still parses and re-renders
  unchanged, and the event schema stays `1`.

  This retires two claims that were not true when written:
  `AUTHORING.md`'s "they anchor artifacts, events, and failure output" and
  `LoweredStep::label`'s own "(events/console)". Same class as
  `reproduce_hint` in the R18 wave — computed all along, printed all along,
  dropped by the record.

- **A fragment's text ran on into the comments introducing the entry below it.**
  hurl attaches the blank and comment lines above a request to *that* request,
  which is exactly what makes the `# @proef` binding reliable — but it also
  means an entry has two different starts: where its lines begin and where its
  request begins. The scanner used one value for both, ending each fragment at
  the next entry's request line, so every comment a corpus author wrote to
  introduce the next request was copied into the previous fragment and from
  there into the emitted `.hurl`. An artifact could carry
  `# Destructive. Operators only.` while containing no destructive request at
  all, and `trim_end` could not help — a comment is not whitespace. The same
  applied at the end of a file, where a trailing note became part of the last
  fragment. A fragment now runs from its annotation to the end of its own
  request and response; the gap between two entries documents the one below it
  and belongs to neither. Nothing executed differently, because hurl permits
  only comments and blanks between entries — which is why it survived: the only
  damage was to what the durable record says a request is.

  The property covering this asserted one request line per fragment, which is
  blind to comments; it now also asserts that no fragment holds any of the
  generator's inter-entry filler.


- **`explain` and the HTML report disagreed about a truncated run's totals.**
  A record with no tail `run_finished` — a run killed mid-flight — is
  reconstructed by counting, and each surface carried its own version of that
  fallback. On the same bytes they differed three ways: the report dropped
  `Warned` scenarios from every column, counted `[run] setup`/`teardown`
  scenarios into a headline its own page labels "excluded from totals above",
  and read a pre-0.6.0 record's per-phase totals as the suite verdict where
  `explain` correctly declined to. One `proef_core::report::suite_totals` now
  holds the rule — prefer the tail event unless it cannot be trusted, else
  count suite scenarios with `Warned` riding along with `Passed`, exactly as
  the live path reports — and both surfaces call it.

  Also un-splices three doc comments in `html.rs` that an earlier change had
  merged into one, leaving `render_tag_table` and `render_timeline` undocumented
  and `render_provenance_and_summary` carrying all three.


- **A parse error pointing at a non-ASCII character produced a span that split
  the codepoint.** gherkin reports a char-counted column, so the span's *start*
  was correct; its end added one **byte** to that, landing inside a multi-byte
  character whenever the error pointed at one — a span that is not a valid
  slice of its own source. Nothing crashed, which is how it survived: miette
  tolerated it and drew the caret slightly to the left, and the LSP's converter
  snaps to a boundary defensively, so every consumer defended itself instead of
  the producer being right. Found by the new `fuzz_feature_parse` target within
  a minute of first running.


- **A long `--tags` expression aborted the process instead of failing.**
  `and`/`or` chains parse iteratively, and the module said so as though that
  settled it — but an iterative parse still builds a left-leaning tree as deep
  as the chain is long, and both `eval` and the derived `Drop` walk that tree
  recursively. A `--tags` expression of roughly twenty thousand `and`-joined
  atoms therefore overflowed the stack and died on SIGABRT: a signal, not one
  of the four exit codes ADR-0009 promises, and well within what a command line
  accepts. Expressions are now capped at 512 tokens, which bounds the tree and
  so bounds both walks, and past the cap you get a message naming the limit.
  (The test that was meant to cover this built 5 000 atoms and asserted
  success — one order of magnitude below the cliff.)

### Changed

- **Secret redaction no longer runs inside the reporter mutex.** The sink
  masked each event while holding the lock that fans it out to the reporters,
  so every scenario thread queued behind work none of them share — and masking
  is the expensive half, a scan per text field per needle with roughly nine
  needles derived per secret. It reads the event and the needle set and writes
  neither, so it never needed the lock; the critical section now covers only
  the fan-out it exists for.

  Order is unaffected and the tests say why: a scenario is one thread, so its
  own events still reach the lock in the order it emitted them, and order
  *across* scenarios was never guaranteed. A new test emits from eight threads
  at once and asserts nothing is lost or doubled, everything arrives redacted,
  and each emitter's own events keep their order. No timing assertion — the
  flake rule forbids one, and the change is justified structurally rather than
  by a stopwatch.

- **Pack validation is linear in the macro count, not quadratic.** Every
  span locator scanned the whole pack file to find its macro's block, so
  validating N macros scanned the file N times. A single indexing pass
  (`locate::MacroIndex`) records each macro's name span and block region, and
  the locators became lookups into it. Measured on a release build over
  generated packs: 3200 macros went from **1.96 s to 0.03 s** (~65×), and the
  curve changed shape — 4× per doubling before, ~2× after — so 6400 macros now
  cost 0.06 s where the old scaling predicts ~8 s.

  It also fixes an inconsistency the split readers hid: `macro_span` accepted a
  quoted `"macro name":` header while the region scan behind every other
  locator accepted only the bare form, so a quoted macro got a caret on its
  name and silently no span for its `match:`, `use:`, `ref:` or payload lines.
  One reader now gives one answer.

### Added

- **`proef flaky --by <key>` splits flakiness by run context.** `--by env`, or
  any `[meta]`/`--meta` key (`--by runner`), folds the history per context
  instead of pooling it. A scenario that flaps in one environment and is solid
  in another is not flaky but *context-dependent* — the fix is in the
  environment, not the test — and a merged history cannot reach that
  conclusion, because pooled failures and passes look exactly like one flapping
  test. The command names the scenarios whose verdict changes with where they
  ran, which is the finding the flag exists for. A run that never set the key
  becomes its own `(unset)` bucket rather than being folded in with runs that
  did; the context also rides in `--format json`. Reads the `env`/`metadata`
  provenance the record has carried since ADR-0020 — no new recorded field.


- **`proef schema config` publishes the `proef.toml` JSON Schema.** TOML
  language servers (Taplo, tombi) validate against JSON Schema, so one file
  buys completion, hover documentation and typo detection in the config —
  before a run rather than after one. Generated from the same Rust model that
  parses the file, so it describes keys as they are *written* (`runs-dir`, not
  `runs_dir`) and inherits `deny_unknown_fields`, making an editor refuse
  exactly what proef refuses. `proef schema` keeps printing the pack schema, so
  one command answers "what may I write in this file?" for both authored
  formats rather than two verbs answering it once each.


- **An assertion that fails on values looking identical now says why.** When
  the actual and expected values differ *solely* in whitespace, the failure
  carries a note repeating both with every whitespace character drawn — `·` for
  a space, `\t`/`\r`/`\n` for the usual escapes, `\u{a0}` for the exotic ones.
  hurl's own message was already correct; the defect was simply invisible, so a
  trailing space, a CRLF fixture leaking `\r`, or a non-breaking space pasted
  out of a browser read as "the tool is wrong". Taken from hurl's structured
  `actual`/`expected` rather than parsed back out of its prose, and emitted per
  error so it sits beside the values it explains; silent whenever the
  difference is already visible.


- **`proef flaky` audits quarantine, which nothing else could.** A
  `@quarantine` scenario's failures gate nothing by design, so no exit code, no
  summary and no CI job reports them — which makes the tag's own failure mode
  invisible: a quarantined scenario failing *every* run has been switched off
  and left in the suite. It now reads `DISABLED` rather than sharing the
  `broken` verdict with untagged always-failures, which wrongly implies someone
  is watching. The opposite case gets its own verdict too: green throughout the
  window is `recovered`, a tag that outlived its problem and is now suppressing
  the next real regression. Both print what to do, and `--format json` carries
  `quarantined` plus the verdict key so a scheduled job can gate on either.

  This needed the record reader to stop dropping data it was already given:
  `scenario_finished` has carried `tags` since 0.15.0, but `ScenarioRun` never
  parsed them, leaving every record consumer tag-blind.


- **Document symbols and hover.** A feature outlines to its scenarios (with
  their tags), a pack to its macros (with the pattern each matches) — the
  vocabulary chosen by what discovery found in the file, never by its
  extension. Hover answers the question go-to-definition charges a round trip
  for: what a step binds, what a `use:` targets, what a `ref:` resolves to and
  which of its variables still need a `bind:`. Every fact is read from the same
  analysis the diagnostics come from, so a hover cannot contradict the squiggle
  on its own line. `SuiteAnalysis` gains a `scenarios` index, taken from the
  parse rather than from binding — an outline that hid exactly the scenarios you
  are debugging would be worse than no outline.

- **A panic no longer ends the editor session silently.** Only the recompute was
  guarded, so a panic inside completion, definition or references escaped the
  message loop and killed the server — leaving an editor that shows nothing,
  which reads as "proef has no opinion here" rather than as a failure. Both
  entry points (a request, the debounced recompute) now wrap everything they do,
  the request is *answered* with `InternalError` rather than dropped, and the
  user is told once per suite state through `window/showMessage` — the channel
  an editor surfaces, unlike the stderr line that was the only report before.
  The next edit clears the report, because whether the new state also fails is
  news.

- **The editor can apply a "did you mean", not just print it.** Every
  misspelled-name diagnostic that already suggested a nearest spelling now
  carries the structured half of that suggestion — a span and a replacement —
  and `proef lsp` serves it as a `quickfix` code action: `use:` and `ref:`
  targets, `with:` and `bind:` keys, step kinds, Examples placeholders, and
  data-table columns. The suggestion is computed **once** and rendered twice
  (prose for a reader, an edit for an editor), so the message and the fix can
  never disagree.

  A fix is attached only when the edit is certain: the suggested name is near
  enough, and the misspelling occurs exactly once, as a whole token, in the
  diagnostic's *own* file. Each of those failing means no fix rather than an
  approximate one — notably, a lowering error anchors on the feature step that
  invoked a macro while the typo lives in the pack, so it finds nothing to
  replace and offers nothing rather than editing the healthy file. The action
  is reachable from either the diagnostic or the token, because the two are
  regularly lines apart: a `use:` error carets the macro's name key.

### Fixed

- **EDITORS.md no longer under-promises on built-in macros.** It said the
  `expect*` family has "no jump target and no hover"; the first half is true and
  structural (their pack is compiled into the binary, so there is no file to
  open), the second is not — a built-in is in the analysis like any other macro,
  so hover answers with its pattern and params and names the pack as `builtin:…`,
  which is exactly *why* the jump is unavailable. Pinned by a test, since the
  page now claims it.

### Changed

- **The editor stops re-analysing the suite on every keystroke.** Completion,
  go-to-definition and find-references each ran the whole pipeline from
  scratch — read every pack and feature off the provider, parse, bind, lower —
  and threw the result away; between two keystrokes none of those inputs have
  changed, so the second run could only reproduce the first one's answer. The
  server now holds the analysis and drops it exactly where an edit lands (the
  same notification path that already marks the suite dirty), so one recompute
  serves the debounced diagnostics publish *and* every request until the next
  edit. Measured on the two-file test suite: 10 provider reads per request
  before, none between edits after — pinned by a read-counting provider rather
  than by timing, per the flake rule.


- **The LSP's type layer moved to the maintained generator**: `lsp-types`
  0.97 (unmaintained since; the crate that shipped its own `fluent-uri`
  `Uri` newtype) is replaced by `gen-lsp-types` 0.11 under the same
  `lsp_types::` name — rust-analyzer's own aliasing pattern, so every
  `use` path is unchanged. Its `url` feature aliases `Uri` to `url::Url`,
  which the embedded hurl engine already pulls in, so the swap adds **no
  new crate** and drops three (`lsp-types`, `fluent-uri`, `serde_repr`).
  `Url::from_file_path`/`to_file_path` are the native-path bridge
  `documents.rs` had to hand-roll under 0.97 — drive letters, segment
  joining, percent-encoding, ~90 lines — so the bridge is now a wrapper
  that only pins the source-name identity rule. Behaviour visible to an
  editor is unchanged; the one difference is what counts as a malformed
  URI (`url` percent-encodes a raw space where `fluent-uri` rejected it),
  and request dispatch now compares a method *enum* rather than strings,
  so an unknown method lands in `Custom` instead of matching nothing.

  Breaking (library): `proef_core::report::percent_encode` is private.
  It was public solely so `proef-lsp` could encode URI path segments
  against the identical unreserved set; that hand-rolled encoder is gone,
  and redaction needles — its only remaining caller — live in the same
  module.

### Added

- **README answers the comparison a prospect actually runs**: a "When
  something else fits better" section maps raw `hurl` (the exit stays open
  in both directions), Karate (choose it for embedded JS and whole-body
  fuzzy matching — the two mechanisms proef deliberately refuses; choose
  proef for one binary, deterministic reproduction, and files that run
  with no framework at all), and Postman/Bruno-class clients. The
  quick-start also points at `proef init` as the start that demonstrates
  the `ref:` body form — `tests/features/` is deliberately fragment-free
  (the reference corpus is config-independent by design, and
  `[run] fragments` is a config key; the runnable `ref:` demo lives in
  the scaffold, pinned green against the fixture).

- **The docs site can get a visitor to a binary, and CI to a green
  workflow.** New [Installing](INSTALL.md) page — install lived only in
  the repo README, outside the published site's source, so the site's
  first step sent visitors back to GitHub — and a new [CI](CI.md) page
  with the paste-ready workflow the docs never had (zero `runs-on` blocks
  existed anywhere): install, secrets via `PROEF_SECRET_*`, `--junit
  auto`, a `--shard` matrix, `--meta` provenance, the `diff
  --fail-on-regression` baseline gate, `--rerun` continuation, and
  `flaky` over retained records. Nav reordered visitor-first (Installing →
  Getting started → Writing scenarios).
- **AUTHORING gains the three recipes every real suite needs**:
  login-then-use-the-token (the docs' most-asked absent question — zero
  "login" hits existed), waiting for an eventually-consistent result
  (finite `retry:` as the polling primitive, and why it must be finite),
  and test-data seeding/cleanup across its three scopes (`Background:`,
  `[run] setup`/`teardown`, `saveAs: global`).

### Fixed

- **The tutorial's `ref:` invitation no longer self-destructs.** §3.6
  showed a second `[run]` table that, pasted beside §3.5's, was a TOML
  duplicate-table error naming a directory the tutorial's layout doesn't
  have; the fragments key now lives (commented) in §3.5's one config
  block. "A suite is two things" undercounted its own mandatory
  `proef.toml` — it says three files now, and the tree shows all three.
  TROUBLESHOOTING stops listing hurl's `[Options] repeat:` as if it were
  a proef step key.

- **The `proef init` scaffold goes green against the dev fixture.** The
  advertised fastest path (`init` → fixture → `test`) ended 1 pass /
  2 fail: the scaffold calls `/search` and `/version`, and the fixture
  served neither — a red first run that read as a broken tool. Both routes
  exist now, the whole path is pinned by an integration test, and the
  scaffold's `ref:` fragment thereby executes against a live endpoint —
  the body form's first runnable demonstration.

### Added

- **Every release archive ships a `.sha256` sidecar** (basename inside, so
  `sha256sum -c` works from a download directory). Attestation covers the
  provenance story for `gh` users; the sidecar covers everyone who
  installs with `curl` — the half that was missing against the
  ripgrep/uv/starship baseline.

- **A broken `proef.toml` is a located diagnostic, not a bare sentence.**
  The file is edited as often as any pack, and it was the one authored
  input whose errors carried no code, no source excerpt and no caret —
  while `pack::yaml` had all three for the structurally identical failure.
  New codes `proef::config::toml` (with toml's own error span under the
  caret) and `proef::config::unreadable`, in the catalogue (73 → 75) and
  pinned by an integration test; `proef lsp`'s boot warning and `doctor`'s
  config row carry the same message.
- **Five help-less refusals gained their missing action.**
  `feature::parse` (the shape of a feature file, and the most common way
  one stops parsing), `bind::ambiguous_step` (make one pattern more
  specific or retire the duplicate), `bind::table_conflict` (one source
  per param), `pack::use_cycle` (pull shared steps into a third macro),
  and the raw `retry: -1` message now says *why* infinite retries are
  refused (hurl cannot be interrupted mid-call) and what to write instead
  — it used to cite "ADR-0007", an internal document id with no in-band
  route to it.

- **A miss below the did-you-mean threshold names the valid set instead of
  going silent.** All eleven suggestion sites ended
  `closest(…).unwrap_or_default()` — when nothing was near, the tail
  vanished, and `unknown_step_kind` said "not claimed by any registered
  engine" about a registry with exactly **one** member it never named. One
  `matcher::suggest_or_enumerate` now serves every site: the nearest
  spelling when one is near, else the set verbatim (small), else a count
  with the command that lists it (`(9 known — `proef macros` lists
  them)`). `unknown_fake`, `unknown_variable` and `missing_config_var`
  carry the same rendered tail through their typed errors.
- **`unknown_placeholder` fires once per authored defect, not once per
  Examples row** — a 500-row outline with one typo'd `<column>` pushed 500
  byte-identical diagnostics at one span (the console collapsed them;
  SARIF, one-result-per-site by design, did not). It also now names the
  header's columns.
- **Every rendered error links the diagnostics catalogue.** The stable
  codes were greppable and led nowhere — the catalogue was linked from
  every doc and reachable from no error. `Rendered` implements
  `Diagnostic::url()` and the LSP sets `code_description`, so editors show
  a clickable link on the code; on a terminal miette renders an OSC-8
  hyperlink, and into a pipe or snapshot the URL prints as plain text
  beside the code (links ride the same TTY/`NO_COLOR` gate as color — an
  escape sequence a non-terminal sink must never see).
- **Pack diagnostics point at the defect, not the macro's name.** Every
  pattern-family and `defaults:` error anchored on the macro-*name* span —
  thirteen of the nineteen seeded pack snapshots underlined `login:` while
  the broken `{rol}` sat on a line outside the excerpt (one excerpted the
  *previous* macro). The `match:`-line span was computed since the pass was
  written and never reached a diagnostic; it does now, with the name span
  as fallback. `locate::macro_span` also stopped matching pack-root
  `bind:` entries (a macro sharing a name with a bind key anchored every
  diagnostic on the config line).
- **Parser errors speak hurl's and gherkin's prose, not Rust's.** A pack
  author was shown `ResponseSectionName { name: "Wrong" }` and
  `Method { name: "" }` — `{:?}` of internal enums from crates they never
  heard of. All three engine sites now render through hurl's own
  `DisplaySourceError` ("the section is not valid. Valid values are
  Captures or Asserts"), and gherkin's expectation-set tail is
  sort-normalized: it renders from a `HashSet`, so the same broken file
  printed two different messages across processes (observed live) —
  breaking snapshot determinism and the duplicate-collapse alike. Pinned.
- **A `resolve::*` error names the pack it lives in.** The span is the
  feature step (the invocation), but `${nope}` is written in a pack YAML
  the message never named — the reader was sent to a healthy `.feature`
  line while the sick file stayed anonymous. Every resolve error now
  carries `(pack <file>)`.

### Added

- **The HTML report is triageable, linkable, and filterable.** Every
  scenario block carries an `id="s-<slug>"` anchor (the same `stem--name`
  slug as its artifact, so the two cannot disagree) — a failure is now a
  URL a colleague can be handed. A "failed:" jump rail under the summary
  links straight to each failing block (blocks keep completion order — the
  rail is how a reader skips the green between failures), and a
  status-filter bar (all/failed/skipped/warned) toggles block visibility
  through a ~15-line inline script: progressive enhancement over classes
  the blocks already carry, no framework, still one self-contained file.
  Snapshot reviewed deliberately.
- **`--watch` reads like an inner loop.** A visual rule with a rerun
  counter separates iterations (twenty edits used to stack twenty trees
  with nothing marking where the current one begins), and the post-run
  line says the verdict in words ("failures — details above") instead of
  an exit number to decode.

- **Shell completions and a man page**, generated by the binary itself:
  hidden `proef completions <shell>` (bash/zsh/fish/powershell/elvish) and
  `proef man` subcommands, and every release archive now carries
  `completions/` plus `proef.1` — generated during packaging by the exact
  artifact they ship beside, so they can never drift from it.
- **`--env` is global**, like `--config`: `proef --env staging test` and
  `proef test --env staging` both work — five commands read the profile,
  and the position-sensitive spelling was a lesson nobody needed.
- **`doctor` examines the project, not just the engine**: suite resolution
  (feature-file count, or the failure), `hurl` on PATH (a warning when
  absent — the engine is embedded, but ADR-0018's stock-replay promise and
  the emitted `# replay:` hints need the binary), and runs-dir writability
  (probed with cleanup — the first-run `create_dir_all` failure was
  invisible to the one command whose job is diagnosis).
- **A typo'd `--tags`/`--scenario` names the nearest real spelling.** The
  refusal held every scenario name and tag at the moment it printed "check
  --tags/--scenario" and used none of them; it now suggests the closest
  name and tag (glob atoms excepted — a glob selecting nothing is a fact,
  not a typo) and points at `proef flows`, the treatment
  `[run] exclusive-tags` always had.
- **`proef fragments` says *why* a listing is empty** when no
  `[run] fragments` root is configured — previously indistinguishable from
  a configured-but-empty corpus, though the reader's next move differs.

- **The console speaks in color, and every run ends on its identity.** The
  status vocabulary (`✓`/`✗`/`∅`/`⚠`, the dotted glyphs, the summary's
  verdict half) is ANSI-colored on a terminal — `NO_COLOR`, a dumb TERM, or
  a non-terminal stream turns it off, and the `run.log` mirror strips the
  paint either way (content verbatim, paint never). Color is paint on
  identical bytes: the record, the exit code and every text assertion see
  the same output. Each run's final stderr line is now
  `run <id> · <seconds>s` — the run id is the reproduction key `--shard`,
  `--shuffle` and `${fake:…}` all hang off, and it previously printed only
  at the top of the scrollback; a red run's trailer adds the
  `proef explain` pointer. Wall-clock stays console-only, never entering
  the record.

### Fixed

- **A failure no longer prints its detail twice.** An engine fault quotes
  the failing step's own detail, and the located step line just below
  printed the same ~200 characters again; when the fault message contains a
  failing step's detail, the fault line now keeps the scenario identity and
  the step line carries the detail once.

- **JUnit failure and skip detail reaches every platform.** The detail —
  assert diff, fragment provenance, `@skip:reason`, the quarantine notice —
  lived only in the `message` *attribute*; GitLab parses only the element
  *text*, and Azure maps the text to its stack-trace field, so half the
  platforms showed a bare failure (or a reasonless skip). Every non-success
  now carries both, and a failure's text node additionally carries each
  failing step's redacted reproduce hint — the content channel has the room
  the one-line attribute does not. Pinned alongside two library guarantees
  that were verified rather than assumed: quick-junit strips ANSI escapes
  and XML-1.0-illegal control characters on every setter (one binary
  response byte used to be the classic whole-report killer on
  Jenkins/GitLab), and `time` is plain three-decimal seconds; both now have
  tests so a dependency bump cannot shed them silently. A third pin:
  composed reports (suite + rerun-carried + teardown) yield each
  `classname`+`name` identity exactly once — GitLab silently drops
  duplicates.
- **The GitHub job summary can no longer vanish at the 1 MiB cap.** The
  documented failure mode at GitHub's limit is *silent disappearance* (and
  oversized writes have aborted jobs in shipped first-party actions); a
  failing rerun-overlay suite with per-tag tables crosses it more easily
  than it looks. The summary now truncates deterministically at a line
  boundary under a 900 KB budget, saying how many lines were cut and where
  the full detail lives.
- **`::error` annotations budget for GitHub's real limit.** GitHub keeps
  ten error annotations per step and silently drops the rest — an uncapped
  emission made a forty-failure run *look like* exactly ten. The budget is
  now one annotation per failing scenario (its first failing step with
  detail, else its fault) capped at ten, with a closing `::notice` naming
  what the ten are out of; `title=` is clipped under GitHub's 255-character
  cap before encoding.

### Breaking

- **`--output` split by meaning: `--format` chooses a format, `-o/--output`
  names a path.** `test` takes `--format json|tap`; the listing commands
  (`flows`, `macros`, `fragments`, `flaky`) take `--format json` — each
  through its own enum, so clap's help can no longer advertise `tap` on
  four commands whose runtime rejected it (the old shared enum lied about
  a quarter of the surface, and `-o` changed category between siblings:
  format on five commands, directory on `artifacts`, file on `report`).
  `--output json`/`--output tap` no longer parse on those five commands —
  clean break, no alias; `artifacts`/`report` keep `-o/--output` for their
  paths, unchanged. The runtime `json_only` check is deleted: the type
  system does its job now.
- **Library:** `World::set_global` returns `bool` (`#[must_use]`) — `false`
  is a refused promotion — and `World` gains `guard_secrets`;
  `Redactions` gains the `taints` probe. The hurl engine's private
  equality-only gate is deleted in favor of the World's.
- **Library:** `ConsoleReporter::new` takes a fourth `color: bool` — the
  TTY/`NO_COLOR` probe stays at the CLI edge; the sans-IO core takes the
  answer as a plain value.

### Fixed

- **`saveAs: global` refuses a secret it can *find*, not just a secret it
  can *equal*.** The gate lived in the hurl engine and matched whole-value
  equality against raw secret values — a capture merely containing one
  (`Bearer <token>`) or carrying an encoded reflection (base64/hex/percent/
  JSON-escape) promoted to `.proef-state.json` in plaintext. The refusal
  now lives on the store's owner (`World::set_global`), armed once per
  scenario by the runner with the same derived-needle set redaction uses
  (ADR-0005) — one needle list for both invariants, and every engine a
  scenario dispatches to is covered. The invariant is now genuinely
  property-tested (any composite carrying a guarded secret never enters
  the store), as CLAUDE.md had claimed of the single example test.
- **The SLA gate honors `@quarantine`.** `sla::check` measured every
  scenario while the exit code excluded quarantined ones — so a
  quarantined, timing-marginal scenario (exactly what gets quarantined)
  could not fail the run on its assertions but still turned it red on
  latency. The latency population now applies the same non-gating list as
  the exit code.

- **A record that travels can no longer lie, crash, or steer.** Reading a
  record predating `scenario_finished.file` (or any foreign baseline whose
  closes key under the serde default `""`), the step buffer never attached:
  every scenario read as step-less, `flaky` could never see a retry or a
  duration, and `diff --fail-on-regression` certified green over empty step
  maps — the close now adopts its steps' file when exactly one pending
  scenario matches by name (pinned by test). The head fold's "first head
  wins" guard tested emptiness rather than position, so a second
  `run_started` in a concatenated or legacy record overwrote the run's
  `env`/`metadata`/`rerun_of` wholesale (pinned by test). `rerun_of` — a
  string read out of the record — was joined onto the runs root
  unvalidated, so a crafted `"../../elsewhere"` spliced a foreign file's
  events into the rendered report; it must now be a single path component,
  and `--run-id` gets the same rule at the CLI edge (a typed clap error on
  separators or `..`, on all four commands that accept one). Record reads
  gained a generous 256 MiB ceiling — the one input loaded with no bound —
  and every duration sum over record-supplied `u64`s (HTML report, tag
  table, `flaky`) is now saturating instead of a debug-build panic on a
  corrupt file.
- **A `[tag-links]` template can no longer be subverted by a tag's
  spelling.** The GitHub-summary sink substituted the tag into the URL raw,
  so `@JIRA-1)[x](y` closed the markdown link early and injected content
  into the job summary; the tag is now percent-encoded in the URL slot.
  Both sinks (HTML report and summary) also render non-`http(s)` templates
  as plain text rather than minting `javascript:`-class links.
- **An inverted `Span` degrades instead of exploding**: `Span::len` and the
  SARIF `byteLength` are saturating — B1's shipped class, closed in the
  type rather than at one construction site.

- **Eleven sites that swallowed an error and reported success now speak.**
  The class the v0.6.0–v0.8.0 series was named for, still present at the
  edges: a poisoned store lock silently skipped persisting the World (every
  `saveAs: global` promotion of the run lost — now recovered, matching the
  runner's own policy, which also stops failing an innocent scenario for
  another thread's panic); an unreadable subdirectory silently shrank the
  suite to a confident "0 failed" (now warned, per entry too); `doctor`
  reported a clean "no packs" over a tree it could not read (now a Fail
  row) and `fmt` formatted nothing while reporting success (now warned); a
  non-UTF-8 environment *value* read as "not set" — the wrong cause — for
  `${env:…}` (now named up front); a `.map.json` serialization failure was
  the one silent write in the run record (now warned); `proef lsp` booted
  with defaults over a `proef.toml` that exists but does not parse,
  silently diverging from the runner (now says so on stderr); one
  unreadable run aborted all of `proef flaky` (now skipped and counted,
  with the two-run floor re-applied over what was readable); `xtask
  docs-check` printed "aligned" when it could not read the directories it
  checks (now a failure); and a mis-severitied diagnostic pushed into the
  lowering error sink vanished entirely (any error-sink entry now fails the
  scenario).
- **`fmt` normalizes every literal-block spelling.** The scan required the
  key line to *end* with `|`, so `hurl: |-`, `|+`, an indent indicator, or
  a trailing comment — all loadable — were silently skipped and `--check`
  certified them canonical. Folded scalars (`>`) stay out deliberately:
  YAML folding rewrites the line structure there is nothing line-preserved
  to normalize.

- **A Ctrl-C landing in `--watch`'s debounce window no longer launches one
  more full suite run.** The ≥300 ms drain between "change detected" and the
  rerun never checked the interrupt, and the rerun then minted a fresh
  cancellation token — so the handler cancelled the *finished* run's token,
  printed "leaving watch", and a whole suite executed anyway. The interrupt
  is now checked inside the drain and again after the new token is stored,
  so a Ctrl-C from any point forward cancels the token the run actually
  carries.
- **`--watch` can no longer go silently deaf.** A delivered watcher error
  and notify's rescan signal (the kernel-queue-overflow event a `git
  checkout` burst produces) were both discarded by the event filter — the
  watch kept printing "watching … for changes" while missing every change.
  Both now retrigger a run, saying why. Two adjacent silent paths gained
  voices too: a runs dir whose path has no final component now warns that
  its writes cannot be excluded from the watch (the self-feeding-loop
  shape), and a failed Ctrl-C handler registration now says the two-stage
  interrupt is unavailable instead of silently dropping the contract.

- **A comment on a section header no longer blinds the scans that gate on
  it.** hurl's own `section_name` parser leaves the rest of the header line
  to the ordinary comment terminator, so `[Options] # tuning` is a real
  section — but proef's scans required whole-line equality. Behind a
  commented header, validation pass 6 was off entirely: `retry: -1`
  dry-ran clean (the abandoned-thread hole ADR-0007 exists to refuse), the
  delay cap and the double-declaration check with it, in inline blocks and
  fragments alike. The same equality bug made `[Captures] # ids` drop every
  capture under it from `.map.json`, and `[Asserts] # note` open a second
  section under an `expect:` merge. One `is_section_header` recogniser now
  serves every section scan.
- **`delay: 5h` is refused like `delay: 90m` always was.** The duration
  table knew `ms`/`s`/`m` but not hurl's `h`, so an hour-spelled delay five
  times over the 1-hour cap fell through the suffix parse and validated
  clean. The table now mirrors `hurl_core`'s `DurationUnit` in full.
- **A pack-scope `bind:` value resolves in the pack's scope, not in whichever
  macro reached it first.** The table resolved through the first ref-using
  macro's argument scope and was then cached for the scenario — a bare
  `${param}` silently took that macro's value everywhere (or vanished, blaming
  an innocent macro). The pack table now resolves arg-free and default-free:
  namespaced references (`${url:…}`, `${vars:…}`, `${secret:…}`, `${fake:…}`,
  `${env:…}`) are its vocabulary, and a bare `${name}` is a deterministic
  error attributed to the pack's own `bind:` in every macro order.
- **A star-heavy tag atom can no longer hang selection or abort the
  process.** The glob matcher was naive recursion: backtracking was
  exponential in the `*` count (a 19-character atom took seconds per tag per
  scenario) and recursion depth grew with pattern length (a long enough
  atom in `--tags`, `[run] exclusive-tags` or `[tag-links]` overflowed the
  stack — SIGABRT, outside the exit contract). Rewritten as the standard
  two-pointer match: linear-ish, iterative, oracle-property-tested against
  the old semantics.
- **A bound value carrying a lone `\r` is refused at lower time.**
  `lower::multiline_bind` tested `\n` alone, so a carriage return (a value
  read off a CRLF file) sailed into the emitted `[Options] variable:` line
  and died one stage later as `emit::invalid_artifact` — blaming generated
  text the author never wrote. The guard now refuses any control character
  except tab.
- **An `HTTP2-Settings:` request header no longer mis-slots an `expect:`
  merge.** The last-entry scan recognised a response line by the bare
  prefix `HTTP`, which the emitter's own recogniser was already hardened
  against; both now share one `is_response_line` (`HTTP ` / `HTTP/`).

## [0.15.0] - 2026-08-25 (the Robot Framework audit: visible skips, tag verdicts, explicit metadata)

### Breaking

- **A quarantined test-failure reaches JUnit as `<skipped>` with a message,
  not `<failure>`** — Jenkins marked builds UNSTABLE while proef exited 0;
  every dashboard now says what the exit code says (ADR-0019). Library:
  `ScenarioSpec` gains `skip`, `ScenarioOutcome`/`ScenarioRun` gain
  `reason`, `Event::ScenarioFinished` gains additive `reason`,
  `write_junit` takes the non-gating list.
- **`--shard` assignments re-deal: the hash gained a mixing finalizer.** Raw
  FNV-1a's low bit is the XOR-parity of the input bytes, so a scenario named
  after its feature file — the commonest Gherkin convention — collapsed to
  one shard at `N=2` and left odd shards empty at `N=4`, silently (the empty
  shard exits 0). `shard_bucket` now finalizes with Murmur3's `fmix64`; every
  scenario re-buckets, so all jobs of one matrix must run the same proef
  version (already true in practice). Round-18 finding, reproduced and
  mechanism-verified before fixing; the balance test gained the
  name-mirrors-file corpus it was structurally blind to.
- **Tag atoms glob.** `*` and `?` in a `--tags` / `[run] exclusive-tags` atom
  are now anchored wildcards (`@FRD-*` selects the family; `?` is one
  character) — previously they were literal characters that silently matched
  nothing, the trap this closes. Metacharacter-free atoms are bit-identical
  to before, property-pinned. Case stays sensitive.
- **JUnit test identity is `classname` + `name`.** `classname` carries the
  feature file, `name` the scenario alone; the old single `name` embedded
  `file:line`, so an edit above a scenario re-identified every test below it
  in Jenkins history and GitLab's MR diff. Anything keyed on the old
  `file:line name` strings must re-key. The suite `skipped` count is now
  spelled `skipped` (was `disabled`, which no consumer reads).

### Added

- **`[tag-links]` turns tag cells into tracker links** (RF's
  `--tagstatlink`, reduced to one mechanism): tag glob → URL template with
  `{tag}` substituted, honored by the HTML report's by-tag table and the
  GitHub summary; the pattern language is the same anchored glob `--tags`
  uses. Library (Breaking): `render_html` takes the link map;
  `tags::atom_matches_public` exposes the one matcher.
- **`--console dotted|quiet`** (RF wave 3): one glyph per scenario (`.`
  pass, `F` fail, `s` skip, `w` warn — lowercase is non-gating, the
  pytest/RF convention, flushed per glyph, wrapped at 80) or just the frame.
  Purely presentation: the record, every report, the post-pool failure
  details and the exit code are identical in every mode; `run.log` mirrors
  the console verbatim, dots included — `events.jsonl` is the full truth.
  Library (Breaking): `ConsoleReporter::new` takes a `ConsoleMode`.
- **A `--rerun` now produces the one JUnit and the one report that cover
  the whole suite** (E2's rerun half; Robot Framework's `rebot --merge`
  shape, done as composition): the run head records `rerun_of`, the JUnit
  carries the base's not-re-run scenarios as ordinary testcases, and
  `proef report` overlays the base into a merged page (banner named, base
  timestamps stripped so timelines never mix, rotated-away base degrades
  loudly). Exit code and totals stay the rerun's own.
- **`--meta key=value` and `[meta]`/`[env.<name>.meta]` record explicit run
  metadata** (ADR-0020, RF wave 2): commit, build URL, team — recorded in
  the run head, shown by the HTML report, GitHub summary, `explain`,
  `diff` (which now also warns on cross-env comparisons) and the
  `--output json` body (additive keys). The active `--env` profile name and
  the `--shuffle` marker ride the same head. proef never harvests: no git,
  no hostname, no CI env sniffing — the shell harvests, proef records.
  Everything passes the sink-boundary mask, keys and values both. Library
  (Breaking): `RunRecord::open` and `exec::execute` take the head inputs.
- **Per-tag verdicts in the HTML report and the GitHub summary** (RF wave
  2): tags now reach the record — additive `tags` on `scenario_finished`
  (finished-only: the cancel-skip path emits no start), additive
  `exclusive` on `scenario_started` (closes R11-6, the scheduler's own
  bool) — and both reports roll them up per tag (suite-only, Warned counts
  with passed). Requirement-tagged suites (`@FRD-3.1`) get their
  traceability matrix for free. Tags are deduped at the one accumulation
  point (first occurrence wins); the quarantine list is now derived from
  the outcomes' own tags — one owner, same behavior, pinned by the exit
  suite. Library (Breaking): `ScenarioSpec`/`ScenarioOutcome` gain `tags`.
- **`@skip` and `@skip:<reason>` park a scenario visibly** (ADR-0019, RF
  wave 2): counted in every total, reasoned in the console, JUnit, TAP, the
  record, the HTML report, `explain` and `flows --output json`; the harness
  maps it to libtest's ignored flag. All-selected-skipped exits 0; the
  empty-selection refusal stays exit 2. `--tags "not @skip*"` unselects both
  spellings; an authored skip is never re-queued by `--rerun`, and `diff`
  gives skip transitions their own bucket instead of reading them as fixed.
- **`flows` shows the feature description.** The prose block under
  `Feature:` was parsed and then dropped — the one paragraph written for
  exactly the reader `flows` serves never reached them. Human output prints
  it under the feature header; `--output json` rows gain
  `featureDescription: string|null` (additive). Library: `FeatureFile` gains
  `description`.
- **`--shuffle` re-deals the execution order,** seeded by the run id — one
  determinism knob for order and fakes alike, so `--shuffle --run-id <id>`
  reproduces an order-dependent failure exactly (Robot Framework's
  `--randomize`, minus the parallel seed it threads separately). Applied
  after `--shard`, so membership never moves; under `--watch` every unpinned
  rerun re-deals, deliberately. The permutation is version-stable and
  pinned. Recording a `shuffled` marker in the run head is deferred to the
  planned `RunStarted` additions (env/metadata), one wire change instead of
  two.
- **The failing step's `reproduce: curl …` reaches the record.** The engine
  always computed the redacted curl and the live console always printed it —
  and the record dropped it, so `explain` and the HTML report knew less than
  the console did. `StepFinished` gains additive `reproduce_hint` (absent on
  passing steps and every pre-field stream); `explain` and the report print
  it; the sink-boundary mask covers it like `detail`.
- **README documents every flag the binary exposes, enforced.** v0.14.0
  shipped `--shard` and `--max-fail` with no README mention; the docs gate
  gains the flags direction (same vacuity guard as the command half), and the
  measured gap — those two plus `schema --add-to` — is closed.

- **JUnit carries what GitLab and Jenkins actually read** (R3-6, specced from
  GitLab's parser docs and Jenkins' `SuiteResult.java`): `file` on each
  testcase (GitLab source linking), `time` on suite and root. `timestamp` and
  `hostname` stay absent deliberately — ignored or substituted by both
  consumers, and a hostname would undo R12-1's provenance fix.
- **The docs corpus is a website: <https://emrecdr.github.io/proef/>.** mdBook
  renders `docs/` on every push to `main` that touches it; the nav is
  `docs/SUMMARY.md`, which the existing docs gates link-check like any other
  doc, and the pages workflow refuses a corpus doc that is not on the site.
  The crate `homepage` points there from the next release.

### Fixed

- **A failure detail is bounded before it reaches any sink.** hurl's rendered
  assert error quotes the actual response, so a failed assert on a large body
  rode full-size into the record, JUnit, the HTML report and the GitHub
  summary at once. The engine now middle-cuts past 40 lines / 8 KiB with a
  marker naming the elision; the artifact pointer survives outside the cut,
  and the full output is one re-run away (Robot Framework's 40-line rule,
  adopted at the boundary where all sinks are covered at once).
- **The machine-body contract closes its last two paths**: an empty selection
  (`--scenario`/`--tags` matching nothing — loud exit 2 by design) and a
  corrupt global-state file both emitted zero stdout bytes under
  `--output json`.
- **Identical errors collapse like identical warnings** — a broken macro
  usually fails to lower *everywhere*, so the error wall was the more common
  fifty-block wall; distinct errors still render separately, and SARIF keeps
  every site.

- **Injected `[Options]` lines respect every section-ending shape.** The
  section-end move covered one shape of five: an unfenced JSON/XML body after
  an author `[Options]` swallowed the injected lines into invalid hurl (exit
  2 on input that worked before), and an entry with an author section but no
  response line leaked its pending lines into the *next* entry, where hurl
  parsed `retry:` as an HTTP header and the artifact validated green. The
  section now ends at the first line that could not sit inside it.
- **A `#` inside a `bind:` value no longer hides the reads after it.** The
  template probe parsed the value in an unquoted position where ` # ` opens a
  comment; it now probes the quoted `variable:` position bake actually
  injects into, so `"{{a}} # {{b}}"` reports both.
- **A setup that fails to load still emits the machine body** — the last
  terminating path returning zero stdout bytes under `--output json`.
- **SARIF keeps one result per site again.** The warning collapse shipped at
  the front-end aggregation, which also feeds SARIF — a code-scanning
  consumer lost every anchor but the first. The collapse now happens at
  console rendering only; SARIF carries all sites, the console one line with
  the count.

- **Every terminating path emits exactly one machine body** (R17-2.3/2.4).
  An empty shard printed its prose note *as* the `--output json` body — `jq`
  failed on the very path a sharded matrix guarantees one job takes — and a
  setup abort printed nothing at all while JUnit carried the failure. The
  note now goes to stderr under machine output (and lost a stray-space run);
  never-ran paths report ADR-0014's suite-only zeros with the exit code
  carrying the verdict.
- **A failed teardown reaches JUnit as its own suite** (R17-2.5) — #78's
  rule made symmetric: a phase appears in the reports when it fails. A gated
  pipeline used to read a fully-passing report on an exit-3 run.
- **A repeated warning is one warning with a count.** One authored mistake in
  a macro shared by fifty scenarios rendered fifty times; identical warnings
  now collapse to their first occurrence plus "(N sites across the suite)".
- **`explain`'s truncated-record fallback counts the suite only** — a record
  that died mid-setup folded the phase scenario into the totals three lines
  above the label saying phases are excluded (ADR-0014).
- **`bind:` values are read by hurl's parser, not a text scan** (R17-2.2). A
  hurl function (`{{newUuid}}`, `{{newDate}}`) no longer counts as an unbound
  variable — proef refused input stock hurl runs — and a sibling literal bind
  whose name sorts earlier now counts as a supplier, since injected
  `[Options] variable:` lines are written and evaluated in name order. The
  seam answers the question once: `FragmentSupport::template_reads`.
- **A fragment's own `[Options] variable:` lines now evaluate before the
  injected ones.** Injection used to land at the section head, so the
  fragment-supplies-it route the unbound check accepts was assigned too late
  to be read at run time — accepted at dry-run, wrong at execution.
- **A `{{x}}` inside a `bind:` value is validated at `--dry-run`, not at run
  time.** hurl templates the injected `[Options] variable:` line when the entry
  runs, so a name nothing supplies used to pass dry-run and die mid-run;
  `proef::lower::unbound_placeholder` now names the placeholder and the bind key
  at lower time, where the capture set is known. What legitimately supplies it:
  an earlier step's capture, the fragment's own `[Options] variable:`, a secret
  in scope, or a sibling literal bind whose name sorts earlier (injected lines
  are written and evaluated in name order).
- **A literal `bind:` that shadows an earlier capture is named, not silent.**
  hurl's `variable:` assigns into one shared set, so the bound value replaces
  the captured one from that entry on — sometimes intended, so it is a warning:
  `proef::lower::bind_shadows_capture`. A secret bind cannot shadow (it skips
  the `[Options]` path) and draws no warning.
- **A failed `[run] setup` reaches JUnit, the GitHub summary, and PR
  annotations.** The abort used to return before the CI-report block, so a job
  gating on `--junit` saw no file at all — indistinguishable from proef never
  running. The reports now carry the setup scenario itself (suite named by the
  setup feature file); exit codes are untouched (ADR-0014), and nothing is
  fabricated for the pool that never ran.

### Internal

- **The machine body has one exit.** `execute`'s six terminating paths each
  pasted the same empty-body emission; they now return through a single
  funnel with the one `emit_machine_body` call after it, so a new path
  cannot forget the contract — and the empty-selection body takes its exit
  code from the refusal itself instead of restating it. Post-merge cleanup
  pass over the deep-audit cycle; behavior pinned by the existing path tests.
- **The probe and bake share the whole `variable:` line.** `template_reads`
  re-spelled the injected `[Options] variable:` line by hand around the
  shared escaper; `proef_core::lower::variable_option_line` now builds it for
  both, and `quote_option` returns to being private (library-surface swap;
  unreleased either way). The section-end flush in `bake_entry_options` also
  drops its fence-branch duplicate — one check covers all shapes — and the
  console collapse builds its annotated message without cloning the
  diagnostic on the common single-site path.
- **The canary stopped trusting the index's tail twice over**: it skips
  prerelease versions (the sparse index is publish-ordered, so a `9.0.0-beta`
  would have become "latest"), and refuses a backport older than the pin by
  semver ordering (an `8.0.2` published after `9.0.0` would have produced a
  green about a downgrade).
- **`deny.toml`'s advisory ignores were dead and are gone.** The quick-xml
  pair was ignored under "the patched release is unreachable" — the quick-junit
  0.7 bump made it reachable and the workspace has been on the patched line;
  the stale ignores would also have silenced any new advisory against it.
  `quick-xml` itself re-pinned to quick-junit 0.7's in-tree copy (`=0.41.0`,
  one lock generation); `lsp-server` rides to 0.10, `toml` to 1.x.

### Documentation

- **The corpus tells the truth again, audited claim-by-claim**: CONFIG
  documents the `[env.<name>.run]` jobs-only rule a reader used to discover as
  a parse error; GETTING-STARTED can produce its own output (it now states the
  fixture token its §5 requires, and its reproduce command names the
  `--secret` the replay needs); EVENTS carries the provenance, totals, and
  field facts consumers implement against; TESTING-STRATEGY describes the CI
  that exists; RELEASING's gate list predicts CI; TROUBLESHOOTING's exit-1 row
  covers the `--check` family. The `#N` in an outline instance is documented
  as positional, with the column-placeholder naming that keeps identity stable
  across `--shard` and JUnit history.

## [0.14.0] - 2026-08-18 (proef at CI scale)

### Fixed

- **`--rerun` after a cancelled run continues it, instead of a false green.**
  `--max-fail` (and Ctrl-C) stop a run early with the never-reached scenarios
  honestly recorded as skipped — but `--rerun` filtered to failures alone, so
  stop → fix → rerun ran only the old failures and reported `exit 0` with most
  of the suite never executed in either run. Reproduced live before fixing
  (found by round-15 external review): stop at 2 of 6, fix, rerun →
  `2 passed · 0 failed`, green, four scenarios untested. On a **cancelled**
  base record `--rerun` now runs failures **plus** the cancellation-skipped
  tail, and says so (`note: the last run was cancelled before N scenario(s)
  ran…`); scenario-level skips only exist under cancellation, so a completed
  base keeps the old semantics exactly. This also changes `--rerun` after
  Ctrl-C — continuing the unfinished work is what stop → fix → continue always
  meant. Mutation-tested: reverting the union fails the continuation test.

### Added

- **`proef test --shard I/N` — stable hash-mode sharding** (R3-3). A CI matrix
  runs `--shard 1/N` … `N/N` on separate machines; scenarios are assigned by a
  frozen FNV-1a hash of the run-wide `(file, scenario)` identity, so **adding
  a scenario never re-buckets the others** — the measured stability argument
  that rejected index-slicing at triage (inserting one scenario re-bucketed
  the whole shifted tail under slicing, nothing under hashing; the shard tests
  pin both directions, and the assignment itself is frozen by literals — the
  hash is a published contract, and changing it would be breaking). Sharding
  applies **after** every other selector (the pinned filter→shard order), so
  each matrix job partitions one agreed-on set. An empty shard of a non-empty
  selection is a note and exit 0 — a small suite over a big matrix is a fact,
  not a mistake — while an empty *selection* keeps the loud typo'd-filter
  refusal, sharded or not.

- **`proef flaky` — flakiness verdicts over the retained run history** (R3-2).
  The 2026 discipline is detect → quarantine → resolve, and proef already
  owned the middle step: `@quarantine` runs a scenario without gating the
  exit code. This is the missing detect, a fold over the records `runs-dir`
  already retains — the window is `[run] keep-runs`, and no new state is
  written. Three signals from fields the record already carries (ADR-0008):
  **flapping** (verdict changed between consecutive observed runs more than
  once — transition-counting, not fail-rate, which is what separates *flaky*
  from *broken*: a scenario failing every run is consistently broken, a
  different problem), **passes only on retry** (green, but some step needed
  more than one attempt — the latent flake pass/fail-history tools
  structurally miss; the record keeps per-step attempts), and **always
  failing**. A cancellation-skipped row is not evidence and does not count
  toward a scenario's history; phases are excluded (ADR-0014). `--output
  json` emits one object per scenario with the counts behind each verdict.
  Fewer than two runs is refused (exit 2), the same answer `diff` gives.

- **`proef test --max-fail N`** stops the run after N suite-scenario failures
  (`1` = fail fast) — the convention Playwright (`--max-failures`), pytest
  (`--maxfail`) and cargo-nextest (`--max-fail`) share, with the shared honest
  semantics: in-flight scenarios finish, the never-run rest record as
  *skipped* (not absent, never passed), and teardown still runs on its own
  token. The stop rides the graceful-cancel path Ctrl-C already exercises, so
  the record is a complete **cancelled** run — which `diff
  --fail-on-regression` already refuses to certify, exactly right for a
  deliberately-partial one. `[run] setup`/`teardown` failures never count
  toward the threshold (a broken fixture is not a failing test, ADR-0014).

### Documentation

- **The R3 enhancement registry is triaged** (OPEN-FINDINGS): `--max-fail`
  built; a flakiness verdict over the run history and hash-mode sharding
  validated as build-next (the 2026 flaky pipeline is detect → quarantine →
  resolve, and the `@quarantine` tag already owns the middle step); CTRF,
  pack doc and the pre-M6 seam refactors deferred with named triggers; OTel
  and Cucumber-Messages exporters declined under ADR-0008's one-record rule;
  items defined only in the absent v1 research document held for a spec.

## [0.13.0] - 2026-08-17 (a record that travels, and a secret that stays one)

### Security

- **An encoded reflection of a secret is redacted (S1).** Redaction was
  exact-match on the raw secret bytes, and a server that reflects a bearer
  token *encoded* — an OAuth introspection endpoint, a debug echo, a JWT claim
  — defeated it: a failing assert quoted the base64 form in its detail, and a
  string trivially `base64 -d`-able back to the live credential reached the
  **console and `events.jsonl`**, the retained record CI uploads. Demonstrated
  live against 0.12.0 by an external research pass and reproduced here before
  fixing. `Redactions::new` now derives each secret's common encoded forms as
  additional needles — base64 (standard and URL-safe alphabets, with and
  without padding), hex (both cases), RFC 3986 percent-encoding, and the
  JSON-string escape — so every construction site (the CLI sink, the engine's
  internal renderer, TAP) is covered by construction. This is the remedy
  GitHub's own log-masking documents for the same limitation: register each
  transformed value too. The needle set covers the reversible transforms that
  occur at HTTP boundaries and does not claim completeness — a secret
  reflected hashed or re-encrypted matches no needle list. Over-redaction is
  the accepted failure direction. Property-tested over every derived form,
  pinned end-to-end by a fixture route that echoes the bearer base64-encoded,
  and recorded as an ADR-0005 amendment.

- **The fragment corpus read is bounded.** `[run] fragments` names a directory
  proef did not write and does not control, and it was read with no per-file or
  total cap: a 279 MB file cost **601 MB of resident memory on `proef flows`** —
  a command that never looks at a fragment — because the text is read whole and
  then copied into an `Arc<str>`. A file over 8 MiB is now skipped
  (`proef::pack::oversized_fragment_file`) and the reader stops past 64 MiB
  total (`proef::pack::fragment_corpus_too_large`). The size comes from the
  directory entry, so an oversized file is never allocated at all; the same
  bound applies in `proef lsp`, where the corpus is held between requests rather
  than for one command. Skipped, never fatal — a corpus is foreign by design, so
  one bad file must not sink the ones beside it. An unreferenced corpus still
  costs nothing: the scan stays lazy, so nothing is reported unless a pack
  actually names a fragment. Filed as R9-3.

### Added

- **`proef diff` takes a path.** Each side is now a run id, a record
  *directory*, or an events *`.jsonl` file* under any name — the stream is the
  record (ADR-0008), so all three must mean the same thing. The file form is
  the CI baseline flow an adopting suite asked for: download the base branch's
  `events.jsonl` artifact and `proef diff baseline.jsonl <new>
  --fail-on-regression` gates the PR, with no shared record store. Previously
  every argument was joined onto `runs-dir`, so a path produced
  `.proef-runs/<your path>/events.jsonl: No such file` — the argument mangled
  into the complaint. A path that does not exist now names itself; a `--baseline`
  flag was considered and declined as a second spelling of the same positional.

### Internal

- **A hung test is now a five-minute failure, not a five-day zombie.** The
  nextest config had `slow-timeout` with no `terminate-after`, which only
  *labels* a test SLOW and never kills it — an `lsp_stdio` test wedged on an
  unbounded `child.wait()` ran for five days with its `proef lsp` child alive.
  Both layers fixed: the two bare `child.wait()` sites got the file's own
  bounded-watchdog pattern (a server that fails to exit now fails the test in
  10s, naming what did not exit), and the runner gained `terminate-after = 2`
  (120s), sized from a cold-cache census of the whole suite (slowest ordinary
  test: 5.1s). The `harness_` trio — which shells `cargo test` inside the test
  and measured 216s on a fully cold cache — gets a per-test override to 600s,
  the nextest docs' own tight-global-plus-overrides pattern. The
  process-group kill (a spawned server dies with its test) was verified
  empirically with a deliberately hung test holding a live child.

- **Cleanup pass over this cycle's four PRs** (reuse/simplification/efficiency/
  altitude review). The corpus-bound *decision* moved into core as
  `pack::CorpusBudget` — it was abstracted in the CLI and hand-copied in the
  LSP, agreeing by copy rather than by construction; both readers now share it
  and only measurement stays reader-local. `Redactions` stopped allocating on
  the miss path (nearly every call: per string field per event under the
  reporter-stack mutex, with the needle list ~9× larger since the encoded
  forms) — clean fields now hand back their original `Arc`. A relative source
  path is left exactly as it arrived, per its documented contract — it had
  been falling through to a per-file canonicalize that could rewrite a
  `../`-typed spelling. The LSP's percent-encoder folded onto core's
  (byte-identical copies, one character set to drift). The fixture's
  hand-rolled base64 became the crate call — its dependency-surface rationale
  died when this same cycle made `base64` a workspace-wide compile. `diff`'s
  path-or-id resolution moved beside its sibling in `record`. A `deny.toml`
  home for the curl floor was tried and **reverted by mutation test**:
  cargo-deny 0.19.8 mismatches build-metadata versions (`curl-sys@<0.4.90`
  banned the *good* `0.4.90+curl-8.21.0`); the floor stays a unit test, now
  scanning every lockfile entry rather than the first.

- **The bundled libcurl cannot silently regress under the June-2026 CVE
  batch.** `curl-sys 0.4.90+curl-8.21.0` in the lockfile is past the batch —
  but only as a transitive accident of resolution, and the usual gates are
  structurally blind here: RUSTSEC carries no advisories for CVEs in a
  `*-sys`-bundled C library, so `cargo audit`/`deny` stay green however stale
  the bundled curl is. A test now asserts the lockfile floor, and each release
  build prints the libcurl actually linked into that artifact (`proef doctor`
  already reported it; the release log now carries it per target). The hurl-8.1
  watch items — `variables-file:`'s missing sandbox first among them — are
  recorded as a pin-bump checklist in the thin-fork runbook.

- **Fuzzing reaches the fragment rules.** `fuzz_pack_load` ran against an *empty*
  corpus, so `ref:` resolution, `bind:` keys nothing reads, a `bind:` colliding
  with a variable the fragment supplies itself, and unbound placeholders were
  covered on paper and unreachable in fact. The new `fuzz_fragment_binding`
  target is **structure-aware**: it builds a well-formed pack and corpus and
  spends its budget on the name space where those rules live. That shape was
  chosen from measurement, not taste — a byte-oriented version never once
  resolved a `ref:` in 1.45 million runs, because reaching the rules meant
  discovering valid YAML *and* a matching corpus at the same time. The corpus is
  read by a synthetic scanner rather than hurl's, which is what keeps the fuzz
  workspace free of native libraries: cargo dependencies are package-level, so
  one engine-dependent target would compile hurl for all of them.

- **Hurl's own annotation scanner is property-tested**, in `proef-engine-hurl`
  where the native libraries already are. The properties pin what the
  entry-boundary arithmetic is for: every reported line lies inside the file,
  every entry is accounted for exactly once, the starts are ordered and
  distinct, and — the one that matters — **no fragment's text runs into the
  entry after it**. That last assertion exists because a first draft without it
  passed while the boundary was deliberately broken.

- **The fuzz target list comes from `cargo fuzz list`.** It had been spelled out
  in `ci.yml` *and* `nightly.yml`, so a new target ran nowhere until both were
  edited, and nothing failed to say so.

### Fixed

- **A run record no longer names the machine that produced it.** `[run] suite`
  resolves against the config directory (0.12.0), so a path-less `proef test`
  handed the front end an absolute path — and every emitter printed it: the
  `.hurl` `# source:` header, `.map.json`'s `feature.file`, every
  `step_finished` event, the console, and pack diagnostics. Two checkouts of one
  suite stopped producing equal artifacts, which is the property ADR-0010 exists
  to guarantee; an adopting suite hit it as `/Users/…` in 133 artifact lines and
  64% of its event stream by bytes.

  The resolution rule was right and stands. What was missing is its **naming
  dual**: resolve against the project, then name against the project again.
  `front::SourceNaming` is now the one boundary that answers "how is this path
  spelled", for features, packs and fragments alike — replacing the fragment
  corpus's separate cwd-relative strip, which was a second anchor for the same
  question. The four ways to name one suite — derived from `[run] suite`, typed,
  typed absolutely, or reached from a subdirectory — now emit one artifact, byte
  for byte.

  A path that arrives relative is recorded exactly as it arrived; a suite or
  corpus genuinely outside the project keeps its absolute name, there being no
  project-relative spelling of it. Filed as R12-1, and it closes R9-6, which had
  described the same defect as safe from the project root — it no longer was.

  **Breaking**, by the rule in `docs/RELEASING.md`: it changes emitted artifact
  bytes, which is inherently breaking and takes a MINOR bump. Migration: nothing
  to do for a suite invoked with a typed relative path — those bytes are
  unchanged. A tool reading `step.file` or `feature.file` out of a record
  produced by a path-less run now sees a project-relative path where it saw an
  absolute one; join it onto the directory holding `proef.toml`. Records written
  by earlier versions are not rewritten.

### Added

- **`[run] keep-runs`** bounds how many past run records `runs-dir` retains. The
  policy already existed as a hard-coded 200; it just could not be expressed, so
  a suite re-run on every save accumulated records for a day with nothing
  signalling a ceiling. `0` keeps none but the run in flight. Rotation still only
  ever deletes directories named by a *generated* run id — `runs-dir` may be `.`
  — so a `--run-id <name>` record sits outside the budget and is never rotated,
  now stated in CONFIG.md rather than left to be discovered. Filed as R12-2.

## [0.12.0] - 2026-08-14 (one path rule, and a watcher that stops lying)

### Fixed

- **A `runs-dir` edited mid-`--watch` no longer feeds the loop its own output.**
  Reruns re-read the config (the fix below), so records went to the *new*
  directory while the watcher's exclusion still named the one it had frozen at
  startup — and every rerun's `artifacts/*.hurl`, now under an unexcluded
  directory, requeued the next run. One edit produced **39 runs in 12 seconds**,
  firing real traffic. This was the third outing for the watch-feedback class,
  so the fix removes the second answer rather than resynchronising it: each
  rerun registers where it is about to write, and the exclusion is derived from
  the same config the run is. A directory a previous run wrote stays excluded
  too, since its events can still be in flight. Filed as R11-8.

- **`--watch --config <relative path>` retriggers on config edits.** The watcher
  compared the config by exact path while `notify` reports events under the
  spelling the OS resolved them to, so `--config proef.toml` never matched and
  config edits produced *nothing* — silently, because feature edits kept working
  and the loop looked alive. Symlinked and `/tmp`-style aliased paths failed the
  same way and are also fixed: the flag is made absolute when it is stored, and
  identity is settled by comparing canonical paths, which is a stricter question
  than being absolute. The same relative-path flaw silently cost `proef lsp
  --config <relative>` go-to-definition across the whole fragment corpus, since
  `documents::name_to_url` refuses a relative name. Filed as R11-9.

- **`doctor` reports a `proef.toml` that will not parse.** The discovery arm had
  become a silent `unwrap_or_default`, so a malformed config left `doctor`
  reporting on invented defaults and printing "all checks passed", exit 0 — with
  the parse error, which the previous code printed, discarded. It is a `project:`
  row now, so it reaches `worst` and the exit code a CI script actually reads.
  Being *absent* is still not a finding: `doctor` must run outside a project.
  Filed as R11-10.

- **`proef fragments` exits non-zero when a `[run] setup`/`teardown` phase fails
  to load.** It printed `error: setup feature failed to validate:` and exited 0,
  because the phase half flattened its failure to "not measured" while the suite
  half kept its code. Withholding the counts was right; reporting success while
  printing errors was not.

- **`proef.toml` has one path rule.** A path *written in the config* now resolves
  against the directory holding the config; a path *typed on the command line*
  still resolves against the working directory. `[run] fragments` already worked
  this way and everything else did not, so two keys in one table meant two
  different roots: from a subdirectory `fragments = "hurl"` resolved while
  `suite = "features"` reported "neither a feature file nor a directory". With
  `--config` the split was worse than inconsistent — pointing at a config in
  another tree ran `dry-run OK` over whatever suite happened to sit beside the
  shell, and never looked at the configured one.

  The rule now covers `suite`, `setup`, `teardown`, `runs-dir` and the `tests/`
  convention probe, plus two files nothing had inventoried: **`.proef-state.json`**
  (the persistent World) and **`.proef-secrets.json`** (the secret store), which
  were anchored on the working directory — so two shells in one project were two
  Worlds and two secret stores. It is the convention Cargo, `tsconfig.json` and
  pytest's rootdir all follow. Absolute values are taken as written, and with no
  `proef.toml` in scope written paths stay relative to the working directory, so
  the config-independent reference corpus is unaffected. Filed as R11-1.

- **`--watch` rereads the config it retriggers on.** Editing `proef.toml`
  retriggered a run that still used the snapshot loaded at startup: changing
  `[url] base` produced a rerun that dutifully called the old host, and the same
  went stale for `jobs`, `[env.*]` and `exclusive-tags`. Watching a file whose
  contents you then ignore is worse than not watching it, because the rerun
  reports that the edit was taken. Each rerun now re-reads the file and
  re-resolves the suite from it; a config that no longer parses fails that rerun
  and leaves the loop watching, since half-typed TOML is the normal state of a
  file being edited. Which *directories* the loop watches is still fixed at
  startup, so changing `[run] fragments` or `[run] suite` needs a restart to be
  watched. Filed as R11-2.

- **`--config` is honoured or refused by every subcommand.** `doctor` printed the
  error for a missing named file and then reported on defaults, exit 0 — the
  "fall back to defaults" `CONFIG.md` forbids — while `fmt`, `init`, `schema` and
  `secret` accepted a nonexistent path silently, against the "global to every
  subcommand" claim in `CONFIG.md`, `README.md` and this file. A named file that
  is not there is now exit 2 everywhere, including where nothing reads it;
  `doctor` stays lenient about *discovery*, which is a different claim. `secret`
  additionally uses the flag, since the store is the project's. Filed as R11-3.

> **Breaking:** the secret store, the persistent World **and the run records**
> move with the config rather than with the shell. What decides whether this
> reaches you is **where you invoked proef**, not where `proef.toml` sits: runs
> started from the project root are unchanged, but a run started from a
> subdirectory used to write `.proef-state.json`, `.proef-secrets.json` and
> `.proef-runs/` beside the *shell*, and now writes all three beside the config.
>
> Nothing is migrated, and none of it announces itself. A World written from a
> subdirectory reads as empty, so `saveAs: global` values start over on the
> first run after upgrading; stored secrets read as absent; and the old run
> records are simply invisible to `explain`, `report` and `diff`, which say "no
> run records" rather than erroring. To carry them over, move
> `.proef-state.json`, `.proef-secrets.json` and `.proef-runs/` from the
> directory you used to run from into the one holding `proef.toml`. Otherwise
> re-run `proef secret set` and take a fresh baseline.
>
> **Breaking (library):** `proef_cli` is not a published library surface, but for
> the record `front::run` takes the state-file path, `ProjectConfig::runs_dir`
> returns a `PathBuf`, `setup`/`teardown` return `Option<PathBuf>`, `suite` is
> gone (fold into `default_suite_path`), and the `secretstore` entry points take
> the store path. `proef_core` gains one item:
> `pack::FragmentCorpus::unreadable_file`.

- **`[run] exclusive-tags` validates itself.** `--dry-run` did not parse the
  expression at all, so a malformed one exited 2 from `proef test` and passed
  `dry-run OK … 0 warning(s)` from the gate CI runs. And a *well-formed*
  expression matching no scenario was silent: `@soloz` against a `@solo` suite
  put every scenario back in the shared pool, exit 0, nothing said — the exact
  silent degradation the key was designed as a config expression to prevent, and
  one that reads as flakiness rather than as a typo. Both paths now parse it, and
  a zero-match expression warns, naming it and pointing at `proef flows`. Judged
  over every scenario the suite loaded rather than the ones selected, so a
  `--tags` filter that removes the matches from one run is not reported as a
  broken setting. Filed as R11-4 and R11-5.

### Changed

- **`proef fragments` says which half it could not measure.** `--check` reported
  "needs a suite that binds" when the suite had bound perfectly well and a
  `[run] setup`/`teardown` feature was the thing that failed to load, sending the
  reader to inspect the half that was fine. The degraded listing also now carries
  the note `macros` prints, so withheld counts read as "not measured" rather than
  as a corpus nothing uses.

- **`proef fragments --check` refuses to pass with no corpus configured.** With
  `[run] fragments` unset it printed `0 entries` and exited 0, indistinguishable
  from a fully-used corpus — so a CI gate disarmed silently the day the key left
  the config. The listing still works; only the *gate* is now a user error.

- **`proef fragments --output json` carries `annotated` on both row shapes.** The
  annotated and unannotated rows differ in eight fields, and consumers had to
  probe for the absence of one to tell them apart.

### Documentation

- `CONFIG.md`'s "everything else keeps running at `jobs` width" was false:
  queueing is strict FIFO, so nothing new starts while an exclusive scenario
  waits at the head. The cost is bounded, not absent, and is now described.
- The one caveat `[run] exclusive-tags` carries is written down in `CONFIG.md` and
  ADR-0007: exclusivity is enforced against the dispatcher's active set, which a
  watchdog-abandoned scenario leaves while its detached thread is still issuing
  requests (hurl cannot be cancelled mid-entry).
- `TECH-SPEC` §10 gained `proef fragments` and the global `--config`; §11's
  `[run]` inventory listed three of seven keys.
- `DIAGNOSTICS.md` carried a `pack::load` row nothing emits — a reader who
  grepped it found a plausible cause that could never be one — and filed
  `lower::multiline_bind` under `proef::pack::*`. Both fixed, and the two-way
  agreement between the file and the emitted codes is now a test, since this
  drifted twice.
- `OPEN-FINDINGS` R9-2 still said `fuzz_tag_expr` "sits in neither fuzz loop"
  three sections after recording that it is in both.

## [0.11.1] - 2026-08-12 (the gaps 0.11.0 shipped with)

### Fixed

- **An output path creates the directories it names.** `--junit`, `--sarif` and
  `report -o` failed when the parent directory did not exist, while
  `artifacts -o` and the run directory created theirs — no rule, four sites
  deciding separately, with the two used most in CI on the failing side. Every
  adopter paid the same `mkdir -p`. `pytest --junitxml`, `jest-junit`,
  `cargo-nextest`'s JUnit store and the `hurl` proef embeds all create them.
  This does not weaken the "side effects should be explicit" principle: that is
  about writing files the user did not name, and here they named exactly this
  path.

- **`proef fragments` counts `[run] setup`/`teardown` usage.** A fragment only a
  phase feature reached was reported `UNREACHABLE — no macro refs it`, which was
  false, and failed `--check` — a false CI failure in the workflow `--check`
  exists for. The verdict also depended on where the phase file sat: inside the
  suite directory it was discovered as an ordinary feature and counted. The
  listing's universe now matches the runner's, and a phase that fails to load
  withholds every count rather than guessing. Filed as R10-2.

- **One predicate answers "is this a fragment file?"** (`FragmentSupport::claims`).
  Three answered it before — CLI discovery via `Path::extension`, the core scan
  via `rsplit('.')`, and the LSP's corpus invalidation case-insensitively — so
  they disagreed about `api.HURL` (the editor rebuilt its corpus for a file
  nothing would scan) and about a dotfile named `.hurl`. Filed as R10-3.

- **`--config` reaches `proef lsp` and `--watch`.** The flag bypasses the
  upward search so a `proef.toml` beside the suite becomes usable — but the
  editor re-discovered its own config and `--watch` watched whatever a fresh
  search found. So in exactly the layout the flag exists for,
  `proef test --config …` ran green while the editor reported every `ref:` as
  unknown, and editing the config driving the run never retriggered it.
  `ProjectConfig` now keeps the file it was read from (with `root` derived from
  it rather than stored beside it), and both consumers use the config actually
  in force. For `proef lsp` the flag also outranks the client-announced
  workspace root. Filed as R10-1.

## [0.11.0] - 2026-08-12 (the adoption response)

### Added

- **`[run] exclusive-tags`** — a tag expression selecting scenarios that run
  with the pool to themselves. Real suites contain scenarios that cannot run
  beside anything: one asserting absolute positions (`items[0]`) needs a store
  no concurrent scenario writes to, and the only workaround was several CLI
  invocations driven by tag discipline in a Makefile, each producing its own run
  record, JUnit file and exit code to aggregate in shell.

  A matching scenario waits for the pool to drain, runs alone, and the pool
  refills after it, with discovery order unchanged so an exclusive scenario never
  loses its place. Queueing is strict FIFO, so nothing new starts while one waits
  at the head — the throughput dip around each exclusive scenario is the price,
  and it is bounded. A config
  expression rather than a reserved tag name, because with a bare convention a
  scenario added months later lands untagged in the parallel pool and breaks
  isolation intermittently — which reads as flakiness rather than as a missing
  declaration. A malformed expression is a user error, never a silently-ignored
  key.

  This is **exclusion, not ordering**: a scenario that must run *before* the
  rest belongs in `[run] setup`, which already runs once before the pool exists.
  Deliberately one axis of the two `cargo-nextest` settled on — per-group
  concurrency limits (rate-limiting a shared dependency) are a real future need
  that nobody has asked for, and a group table can be added later without
  breaking this key.

- **`proef fragments`** — the corpus listing, symmetric with `macros`. Until now
  no proef output stated how many fragments there were, so neither way a
  fragment can die had a denominator to be noticed against: one no macro
  references was unobservable, and one reached only through a macro no scenario
  binds *looked* covered because the macro was flagged. Both are now named
  apart, unannotated entries are listed by line (they have no name to list by),
  and `--check` exits 1 when something never runs. `--require-annotated` extends
  that to unannotated entries and is deliberately opt-in: an unannotated entry is
  inert by design (ADR-0018), so "not done yet" is a porting team's meaning, not
  every adopter's. Reachability is read off the lowered scenarios, so a fragment
  reached through a chain of `use:` counts as reached.

- **`--config <path>`**, global to every subcommand, naming the `proef.toml` to
  read instead of searching up from the working directory. Discovery only goes
  up, so a config beside the suite is unreachable from the repository root — a
  layout an adopting team planned and abandoned after it failed. A named file
  that does not exist is a user error rather than a fall back to defaults:
  discovery finding nothing means "no project here", but a named path that is
  not there is a typo, and a silently unconfigured run is what that used to buy.

- **`proef doctor` sees the fragment corpus** — a row reporting how many
  fragments loaded from `[run] fragments`, warning when the configured root is
  not a directory. A misconfigured path used to surface much later as
  `pack::unknown_ref`: an error about a *name* when the cause is a *path*.

- **`proef init` scaffolds both body forms** — a one-entry `.hurl` file with a
  `# @proef` annotation, `[run] fragments`, and a pack macro of each kind. The
  newcomer with most to gain from `ref:` is the one who already owns a hurl
  corpus, and a scaffold teaching only `hurl: |` reads as "proef wants your
  files transcribed into YAML".

### Fixed

- **A `bind:` key nothing reads is refused** (`proef::pack::unread_bind_key`),
  with did-you-mean over the names actually in scope. `bind_without_ref` only
  caught a table with no `ref:` at all, so `bind: { token: …, toekn: … }`
  validated clean — the one authoring mistake in the fragment path that produced
  no signal whatsoever. Checked as a **union over the scope**, never against one
  fragment: a pack-scope table is the plumbing every macro in the file needs, so
  a key serving one macro and not its siblings stays correct.

- **`duplicate_fragment` no longer says "in both `x` and `x`"** for two entries
  in one file, and stops offering `file.hurl#name` as the remedy there — that
  qualifies by file and cannot separate two entries inside one. Annotating a
  corpus adds many names to few files, which makes same-file the likely
  collision.

- **`unbound_placeholder` names all three supply routes.** The omitted one was
  the fragment's own `[Options] variable:` — the route that makes a corpus file
  runnable standalone, which is the property ADR-0018 exists to preserve.

- **A fragment's `[Options]` escaped the ADR-0007 value caps.** `retry: -1`,
  `repeat: -1` and an unbounded `delay:` were rejected in an inline `hurl:`
  block and *accepted* in a `ref:` fragment — byte-identical text, exit 2 one
  way and "dry-run OK, 0 warning(s)" the other, then written verbatim into the
  executed input. The scan lived inside the inline-only linter; only the
  twinned-option half of pass 6 had crossed to fragments. It reads the text
  alone, so it now runs against a fragment's too, anchored on the `ref:` line
  and naming the fragment file and line. This is the case the caps exist for:
  hurl has no cancellation, so an infinite retry makes the batch budget
  unestimatable and leaves the watchdog abandoning a thread it cannot stop.

- **A step declaring both `ref:` and a payload was told, falsely, that its pack
  had no `ref:` at all.** The conflicted step is reported and dropped, so the
  loaded bodies stop showing every `ref:` the author wrote — and the pack-scope
  `bind_without_ref` check then drew a conclusion from the gap. It now infers
  nothing from a pack whose steps did not all normalize.

- **A pack-scope `bind:` with no `ref:` anywhere was silently dropped.**
  `AUTHORING.md` said `bind_without_ref` applies "at every scope" while only the
  macro and step scopes were checked — and a setting ignored in silence is the
  bug those two exist to refuse. The check was the better half of the
  disagreement, so the pack scope now has it too.

- **A multi-line `bind:` value blamed the artifact.** A hurl
  `[Options] variable:` value is a single-line scalar, so a newline could never
  reach the entry — but it surfaced one stage later as `emit::invalid_artifact`,
  pointing at generated text the author never wrote. Refused by name at lower
  time as `lower::multiline_bind`, naming the inline `hurl: |` form that *is*
  what splices a multi-line body (ADR-0018's splicing-versus-binding boundary,
  enforced where it can be explained).

### Changed

- **Breaking (library):** `AnalyzeCtx` takes the fragment corpus instead of
  building one. Building it internally meant a fresh scan memo per call, so the
  LSP re-read and re-hurl-parsed the **whole corpus on every request** — each
  completion popup, each go-to-definition, each debounce tick. The server now
  holds one and rebuilds it only when a fragment file changes; editing a pack or
  a feature, which is nearly every keystroke, leaves it alone. It is also what
  core purity already required: the caller does the IO.

- **Breaking (library):** `StepKindSpec` gained `options`, an engine-contributed
  recogniser mapping a raw option key to what ADR-0007's budget rules should make
  of it. The fragment half of that rule already crossed the seam while the inline
  half matched `"retry-interval:"` as a literal inside `proef-core` — one rule at
  two altitudes, and a second engine would have had its fragments linted and its
  inline blocks not. A kind contributing no recogniser is not linted, since the
  core has no way to know what its option keys mean.

- **Breaking (library):** `proef_core::engine::FragmentScanner` returns
  `ScannedFile { fragments, unannotated }` rather than `Vec<ScannedFragment>`.
  An engine's scanner now also reports the 1-based lines of entries carrying no
  annotation — lines only, never built-then-discarded fragments, so a foreign
  corpus still costs a push per unannotated entry. Without it "which entries did
  I forget to annotate?" is unanswerable: a missing annotation produces a green
  run and a silently absent test, and the entry that would prove it was never
  built. `FragmentCorpus` gains `fragments()`, `unannotated()` and
  `diagnostics()`, because the scan is gated on some pack naming a fragment —
  so `PackSet::fragments` is empty for exactly the suite a listing has most to
  say about.

### Documentation

- **Config discovery is a requirement, not a convention.** `proef.toml` is found
  by searching *up* from the working directory, so a config beside the suite
  (`tests/proef/proef.toml`) is never found from the repository root — an
  adopting team planned that layout and discovered it by failure. CONFIG.md now
  says so, and notes that keeping the file at the root collapses the one place
  `[run] fragments` (config-relative) and `suite`/`setup`/`teardown`/`runs-dir`
  (cwd-relative) differ.

- **The release runbook could not work as written.** `main` is a protected
  branch, and step 4's `git push origin main --follow-tags` fails in the
  dangerous direction: `--follow-tags` is not atomic, so the branch is rejected
  while the tag still lands — and the tag is what `release.yml` triggers on,
  starting a release build from a commit that is not on `main`. It happened
  cutting 0.10.0. The runbook now routes the release commit through a PR and tags
  the merged commit, and the `cargo publish` section carries the dry-run,
  tag-check and `--locked` sequence plus why only four crates go
  (`[workspace.package] publish = false` is the default). Also drops step 1's
  reference to changelog "bottom links", which do not exist.


## [0.10.0] - 2026-08-12 (named hurl fragments)

> **Breaking (library):** `proef_core::pack::load` takes a
> `&proef_core::pack::FragmentCorpus` between the packs and the step kinds
> (`&FragmentCorpus::empty()` for the previous behaviour, or
> `FragmentCorpus::new(sources, kinds)` to supply fragment files), and
> `PackSet::fragments` is an `Arc<BTreeMap<…>>` so one scan can be shared by
> every load; `LoweredScenario::secrets` is a
> `BTreeMap<String, String>` of engine-variable → secret name rather than a
> `BTreeSet<String>`; `Prepared` and `ScenarioCtx` each gain a
> `secret_bindings` field carrying that map to the engine; and
> `SourceProvider::discover_fragments` is a **required** method (return
> `Ok(Vec::new())` to serve none) — it was briefly defaulted, and the default
> silently disabled fragments for a provider that forwarded the other two; and
> `ScannedFragment::name` is a `String` rather than `Option<String>`, because a
> scanner now reports only the entries it found an annotation on; and
> `ScannedFragment` and `pack::Fragment` each gain a
> `supplied_variables: Vec<String>` (`Vec::new()` for none), which an engine's
> scanner must fill from the entry's `[Options] variable:` lines — leaving it empty
> reinstates the silent last-wins it exists to refuse; and both
> `LoweredStep`, `StepOutcome` and `Event::StepFinished` gain a
> `fragment: Option<String>` field and `analyze::FragmentDef` gains
> `placeholders: Vec<String>`, so a literal construction of any of them needs
> one more line (`None` / `Vec::new()` reproduces the previous behaviour). The
> *wire* schema is unaffected — the event field is skipped when absent, which is
> what keeps existing records byte-equal.

### Added

- **The docs are checked mechanically, not only read.** `xtask docs-check` gained two
  passes — every relative link resolves, and every fenced `toml`/`yaml` example parses
  with the product's own parsers, so the check means "proef would accept this example"
  rather than "some parser would". A third pass, whether a documented command or long
  flag actually exists, needs a built binary and so lives in
  `crates/proef-cli/tests/docs.rs`.

  All three were written against defects already in the tree: ADR-0018's first example
  could not load (an unquoted `${…}` inside a YAML *flow* mapping, where `{` opens a
  nested mapping), and a row marked *shipped* documented `proef report --html`, a flag
  that never existed. Both had correct prose around wrong code — the failure mode review
  does not catch.

- **Packs can name fragments: `ref:` and `bind:` (ADR-0018).** A macro step's body
  may be `ref: <fragment>` instead of an inline `hurl:` block, and `bind:` supplies the
  fragment's `{{…}}` variables at pack, macro and step scope, most specific winning.
  Fragment names are global, and `file.hurl#name` qualifies one — the same two
  spellings, resolved the same way, that `use:` already accepts.

  Refused at load, each with its own code: a `ref:` naming no loaded fragment
  (`unknown_ref`, suggesting the closest, and saying so plainly when *no* fragment file
  was loaded rather than implying a typo); two files declaring one name
  (`duplicate_fragment`); a file the engine cannot read (`bad_annotation` — its
  siblings still load); a step that is both `ref:` and a payload
  (`body_form_conflict`); and `bind:` on a step with no `ref:`
  (`bind_without_ref` — an inline block takes `${…}`, so that binding would feed
  nothing, and a setting silently ignored is the bug this refuses to ship).

  A fragment declaring its own retry alongside a step's `retry:` is the same
  `option_declared_twice` an inline block gets, so the two body forms behave
  identically rather than differing by where the hurl text happens to live.

  A fragment may also supply a variable to itself with an ordinary
  `[Options] variable:` line — that is how a corpus file stays runnable on its own,
  so it counts as an answer to that fragment's own `{{…}}` and needs no `bind:`.
  Supplying *and* binding the same name is refused (`option_declared_twice`): both
  reach the entry as `variable: k=`, hurl takes the last, and the fragment's own
  line is last — so the bound value would silently never be sent, and would stay
  unsent for every later entry, since hurl's `variable:` assigns into the run-level
  set rather than scoping.

  Discovery arrives below, so a `ref:` resolves end to end.

- **`[run] fragments` — the hurl files a pack may `ref:`.** Names one root, scanned
  recursively for the extensions the registered engines claim, so discovery never learns
  a file type of its own. Unset means no fragments: there is no convention fallback,
  because `unknown_ref` saying *"no fragment files were loaded"* beats guessing at a
  directory.

  **Relative paths resolve against `proef.toml`'s own directory, not the working
  directory.** The config is found by walking *up* from the cwd, so a path in a config
  three levels above must mean "relative to the project" — otherwise `proef flows` from a
  subdirectory reads the right config and then cannot find anything it names. `[run]
  suite` predates this and stays cwd-relative; it is only consulted when no path was
  given, so the difference is not observable there.

  The LSP resolves fragments through the same root, so `ref:` does not read as unknown in
  an editor while the suite runs green. `--watch` retriggers on `.hurl` edits and watches
  the fragment root separately, since a corpus may live outside the suite. `proef fmt`
  still refuses `.hurl` in both discovery branches — it locates hurl blocks *inside* YAML,
  and a corpus proef did not write is not proef's to rewrite — now pinned by a test.

- **Fragments lower, bind, and execute.** A `ref:` step emits the fragment's own text
  with its non-secret bindings baked in as per-entry `[Options] variable:` lines, so the
  artifact stays the executed input and replays identically under the stock CLI
  (ADR-0010). Values are always quoted: `variable_value` tries null/bool/number before
  string, so an unquoted `records`, `2` and `true` would become three different types by
  accident.

  Two refusals guard the parts that could otherwise pass silently:

  - `lower::unbound_placeholder` — a fragment reading a `{{variable}}` that no `bind:`
    in scope supplies and no earlier step captures, anchored on the `.hurl` line the
    variable is on rather than on the pack. hurl's `[Options] variable:`
    *assigns into one shared set* rather than scoping, so an unbound name would inherit
    whatever a previous entry happened to leave and run green against the wrong value.
  - `lower::secret_in_composite_bind` — a `bind:` value mixing `${secret:…}` into a
    larger string. To inject that, the composite would have to be materialized into the
    artifact, which ADR-0005 forbids; bind the secret alone and let the fragment spell
    the surrounding text.

  Secrets keep their own path: recorded as engine-variable → secret name and injected
  via `insert_secret` at run time, never as an `[Options]` line. That indirection is
  what lets `bind: { auth_token: "${secret:apiToken}" }` give a secret the variable name a
  corpus proef did not write already uses.

  Bindings resolve **once per scope instantiation** — pack scope once per scenario,
  macro scope once per invocation, step scope per step — so one binding is one value and
  two bindings are two. A macro with no `ref:` step resolves nothing, so an unused table
  never advances the `${fake:…}` counter.

- **The engine seam can describe fragment files (ADR-0018, groundwork).**
  `StepKindSpec` gains `fragments: Option<FragmentSupport>`, and `proef-core` gains
  `ScannedFragment` / `FragmentScanError` / `FragmentScanner`. The hurl engine implements
  the scanner over hurl's own AST: the `# @proef <name>` annotation is read from the
  entry's `line_terminators`, so the annotation↔entry binding is exactly as reliable as
  hurl's parser and no text is scanned for structure. An entry's required inputs and
  produced captures are read from the same AST, which is what will let an unbound
  placeholder be an error rather than a runtime surprise.

  Additive only — nothing was removed from `proef-core`'s surface, and no hurl type
  appears anywhere in it. Discovery asks the registry for the extension instead
  of naming `.hurl` itself, so this stays ADR-0002's "adding an engine leaves
  `proef-core` diff-empty" rather than an exception to it. **Nothing observable ships
  yet:** no pack can reference a fragment until the schema lands.

  `StepKindSpec::fragments` is one `Option<FragmentSupport>` rather than a separate
  extension and scanner, so a kind that claims a format it cannot read is not
  expressible; a file no kind claims is skipped rather than handed to whichever engine
  happens to be registered first. `ScannedFragment::declared_options` lists option
  *families* rather than flagging retry alone, so the core applies its
  double-declaration rule to `delay:` too — through the same `bake_entry_options` path,
  so leaving it out reproduced the very last-wins bug the rule exists to refuse.
  `supplied_variables` is separate from it because the two clash on different keys:
  an option family family-to-family, a variable name-to-name.

  A note for whoever extends the scanner: hurl's `Visitor` treats templates as *leaves*,
  and `visit_template`, `visit_url` and `visit_filename` are three separate no-op
  defaults that do not forward to one another. Overriding only `visit_template` silently
  under-reports an entry's inputs — and a missing input reads as "needs no binding".

- **A run record says which fragment a step ran, and `explain` prints it.**
  `step_finished` gains a `fragment` field carrying `file.hurl#name` (additive per
  ADR-0008: absent for an inline `hurl:` block, so no pre-existing record changes a
  byte — the reference event-stream snapshot is unmoved), and `proef explain` renders
  it under a failure as `via tests/hurl/admin.hurl#admin.search`. A step that never ran
  reports it too: "not run" is exactly when someone is reconstructing what the suite
  was about to do.

  This closes a promise ADR-0018 made rather than adding a new one — three files per
  test was accepted *on the condition* that `explain` and go-to-definition earn it
  back, and only go-to-definition had. The name is qualified at lowering rather than
  by the reader, because a record has to stand alone: by the time it is read, the pack
  that named the fragment may say something else.

  **`JUnit`, the GitHub job summary and the `::error` annotations name it too**,
  as a trailing `(via file.hurl#name)` on the failure message, and the HTML
  report renders it under the reason. CI is where a reader is least able to go
  looking for themselves, so it is the last place provenance should drop out —
  and all three sinks share one helper rather than a format string each, because
  three copies is how one of them quietly stops agreeing with the run record.

- **`bind:` completes against what the fragment actually reads.** With the cursor
  in a `bind:` table — flow or block style — the editor offers the `{{variables}}`
  of the fragments that pack `ref:`s, nearest `ref:` ranked first, each labelled
  with the fragment that wants it. The names come off the engine's own AST at
  scan time (`analyze::FragmentDef::placeholders`), so this is the file's real
  interface rather than a second description that could disagree with it.

  Until now the only route to a foreign corpus's variable names was to run the
  suite and read `proef::lower::unbound_placeholder` — a *lower-time* error, so
  the names arrived only after a failure. `bind:` exists at three scopes and only
  the step one names a single fragment unambiguously, so the list is a union
  rather than a guess; the owning fragment rides in each item's detail.

- **The fragment corpus is scanned once per command, not once per pack load.**
  A `proef test` loads packs up to four times — the suite, then `[run] setup` and
  `[run] teardown`, each validated and then run — against different feature paths but
  always the same corpus, and each load re-read and re-parsed every `.hurl` file.
  Measured on a 200-file / 15k-line corpus: **140 ms → 40 ms** warm, with pack loading
  falling from ~28% of the run to a single pass. The win scales with the corpus, which
  is the direction adoption goes.

  The corpus is now read once per invocation (`front::fragment_corpus`) into a
  `FragmentCorpus` that scans itself **lazily, at most once**. Laziness is the part
  worth guarding: `load_collecting` still scans only when some pack actually has a
  `ref:`, which is what makes CONFIG.md's "pointing at a corpus you did not write costs
  nothing" true. Hoisting the scan to the caller to share it would have bought the speed
  by breaking that promise, so the memo lives with the corpus instead — and a test
  proves the eager version fails, by pointing an unreferenced corpus at a file that
  cannot parse and asserting no diagnostic appears.

  Built per invocation rather than in a static: `--watch` re-enters the same process
  after each edit, and a corpus outliving one run would serve pre-edit fragments to the
  next.

- **Go-to-definition on a `ref:` worked again, then briefly did not.** Shortening the
  `[run] fragments` root to a cwd-relative spelling — done so a run record would not
  carry an absolute, machine-specific path — also shortened the root `proef lsp` hands
  to its source provider. The LSP keys document identity on absolute names
  (`name_to_url` yields `None` for anything relative), so every `ref:` go-to-definition
  returned null and `.hurl`-positioned diagnostics stopped publishing, while the suite
  still ran green. That is the capability restored two commits earlier.

  Resolution and spelling are now separate concerns: `ProjectConfig::fragments()`
  returns a resolvable path, and the shortening happens at the naming boundary in
  `front::fragment_sources`, which only CLI runs pass through. Both properties hold at
  once — the editor resolves, the record stays portable.

  Covered by an end-to-end `proef lsp` stdio test with a real `proef.toml`, the seam the
  unit tests could not reach: they inject absolute names through a fake provider, so
  they never exercise config → provider → URI. The test canonicalizes its temp root
  deliberately — on macOS a tempdir is `/var/…` whose real path is `/private/var/…`, and
  without that the cwd comparison silently no-ops and the test passes vacuously.

- **Every failure sink names the fragment, not just the CI ones.** `via()` moved from
  `ci_reports` to `render`, and the console failure list and TAP diagnostic now carry it
  too. A helper scoped to one delivery channel was how `proef test` printed no
  provenance on stderr while `report.junit.xml` from that same run printed it — the
  drift the helper's own comment says it exists to prevent.

### Internal

- **The secret-name join has one home.** `proef_core::engine::secret_variables` pairs a
  scenario's `secret_bindings` (variable → secret name) with its `secrets` (name → value)
  and is the only place that join is written. Doing it engine-side invited injecting
  under the *secret* name, which makes a renamed binding (ADR-0018) resolve to nothing —
  the request then leaves with an unresolved `{{…}}` and fails far from the cause. It
  yields borrows on purpose: an owned variable → value map would put a second copy of
  every secret value in memory per scenario, and ADR-0005 keeps values in one place.

- **`engine::OPTION_FAMILIES` names the vocabulary the double-declaration check compares
  against**, and `MacroStep::declared_options` derives the other half of that comparison
  once for both body forms. The two sides were previously hardcoded lists that met by
  string equality with no test spanning the crates — a spelling only the engine knew
  would have matched nothing and quietly disabled `option_declared_twice`, reinstating
  the hurl last-wins it exists to refuse. A `proef-engine-hurl` test now asserts every
  family the real scanner emits is one the pack can declare; `delay` was untested there
  entirely.

- **Lowering's two diagnostic sinks are one `Sinks` value.** They were adjacent
  parameters of the same type threaded through seven functions and a closure:
  transposing them at any of a dozen call sites compiled cleanly and routed every error
  into `warnings`, so a scenario that should have failed lowered "successfully" and the
  run exited 0. No `&mut Vec<Diag>` parameter remains in `lower.rs`, which makes the
  mistake unspellable rather than merely unmade.

### Documentation

- **AUTHORING says which body form to reach for, and why.** A table contrasting
  splicing against binding — what each can substitute, whether it can be reused, whether
  stock `hurl` can run it, and when an unknown variable is caught — plus the rule that
  decides it: inline when you need to splice something hurl cannot template
  (`${docstring}` as a body has no binding equivalent), `ref:` when the request is
  shared, foreign, or must stand alone. `CONFIG.md` gains `[run] fragments` with a
  worked three-file example.

- **The hurl non-goal is about generation, not direction (PRD §3 amendment).** It read
  "importing/round-tripping *hand-written* hurl files into Gherkin (artifacts flow
  outward only)" — a clause and a parenthetical saying two different things, the
  parenthetical forbidding hurl text from being an input at all. What the non-goal
  protects is that proef never authors a test for you, and that reasoning is untouched
  (ADR-0016 stays declined on it). It does not extend to hurl being an input *source*,
  which §1's own framing — "there is no tool that joins the two" — describes as the
  product's purpose. Recorded honestly: OPEN-FINDINGS M3 asked for this re-examination
  to arrive with a measured port cost, and it has not.

- **ADR-0018 — named hurl fragments.** A macro step's body may be `ref: <fragment>`
  naming one entry in a real `.hurl` file, annotated `# @proef <name>`, with proef
  values supplied by an explicit `bind:` map instead of `${…}` splicing. The file stays
  valid hurl, so the same file runs under `proef test` and under stock `hurl`. Inline
  `hurl: |` is unchanged and stays: the two are splicing versus binding, with different
  capability envelopes, and the 844-line corpus port is recorded in the ADR as evidence
  the inline path is sufficient for real work. No behaviour ships with this entry — the
  ADR and the charter amendment land first, deliberately.

### Fixed

- **`--watch` reran itself forever.** ADR-0018 added the engines' fragment
  extensions to the retrigger allowlist — `.hurl` among them — while every run
  writes `.proef-runs/<id>/artifacts/*.hurl`. A watched tree containing its own
  runs dir fed itself: **49 runs in 15 seconds**, firing real traffic in a tight
  loop and churning record rotation. The filter now excludes generated trees by
  directory name, reusing discovery's own `skipped_dir` so there is one rule with
  two consumers, and takes `[run] runs-dir` for the case where it is not a
  dot-directory. `OPEN-FINDINGS` P5 had closed this "by inspection", naming
  `.hurl` as a file that could never match; the note is corrected in place.

- **One unreadable file sank the whole corpus.** A fragment root is *foreign by
  design*, but a single binary or latin-1 file in it exited 3 from every command
  — `flows` included, which never looks at a fragment. Read failures are now
  per-file diagnostics (`pack::unreadable_fragment_file`) that never sink their
  siblings and stay silent until something `ref:`s the corpus, matching what pack
  loading and the annotation scan already did.

- **`schema --add-to` rewrote fragment files.** It prepended a yaml-language-server
  modeline to a `.hurl` corpus file and dropped the pack schema beside it —
  violating ADR-0018's "fragment files are inputs proef never writes". It now
  refuses anything that is not a pack, reusing the `is_pack_file` predicate `fmt`
  already had.

- **A `#` in an annotation name was accepted but unreachable.** `#` separates a
  file from a fragment in `ref: file.hurl#name`, so such a name could be declared
  and never referenced — and the failure suggested the exact spelling that had
  just failed. Refused at scan time.


- **`proef lsp` answered every URI-keyed request with `null` on Windows.** A source
  name is an identity compared as a string, and the two sides spelled it differently:
  `Path::join` appends without rewriting what is already there, so a `proef.toml`
  saying `suite = "tests/features"` — the portable spelling the docs use — produced
  `C:\proj\tests/features\packs\api.yaml` from discovery while the client's document
  URI produced `C:\proj\tests\features\packs\api.yaml`. The two never matched, so
  go-to-definition, find-references and completion all found nothing while the suite
  itself ran green. Discovered names are now rebuilt in native form. Unix has one
  separator and was never affected, which is why every gate stayed green.

- **A fragment's path was absolute everywhere it was named.** `[run] fragments`
  resolves against the config file's directory, so `fragments = "tests/hurl"` became
  `/home/you/project/tests/hurl` — and that spelling then *named* the file in every
  diagnostic and, once steps recorded their provenance, in the run record too. Feature
  and pack names are project-relative because the path the author typed was; a path the
  author never typed had no such luck. Records went machine-specific: the same suite on
  two checkouts stopped comparing equal, and a temp-dir path could reach a durable
  artifact. The root is now shortened back to a cwd-relative spelling when it is under
  the working directory — resolution is untouched, so which file gets read never
  changes.

- **Every `ref:` was an error in the editor while the same suite ran green.**
  `SourceProvider::discover_fragments` shipped with a default `Ok(Vec::new())`, and the
  LSP's overlay provider — which forwards feature and pack discovery to disk — never
  overrode it. So the analyzer saw no fragments at all: go-to-definition on a `ref:` did
  nothing, `ref:` completion returned nothing, and every `ref:` rendered as
  `proef::pack::unknown_ref`. Exactly the diagnostics-you-cannot-trust drift the
  fragment-aware analysis was added to prevent.

  The default is gone; `discover_fragments` is a required method. Every implementation
  lives in this workspace, so the default bought no compatibility — it only let a
  forwarding provider inherit "no fragments" silently instead of failing to compile.
  An integration test now drives the real provider chain and asserts a `ref:` jump lands
  on the annotation in the `.hurl` file.

- **A fragment file saved with a BOM failed at line 1, blaming the request.** Every other
  text entry point (`feature::parse`, the inline-payload probe) strips a leading
  `U+FEFF`; the fragment scanner did not, so the mark reached hurl's parser as the first
  character of the first request. The file is now normalized by the same rule, and the
  mark cannot travel into an artifact that has to be valid hurl.

- **A macro-scope `bind:` with no `ref:` step was silently dropped.** The step-scope
  version of this mistake has been a hard error since `bind:` landed; one scope up it
  vanished at lower time. That is the half authors actually hit, because factoring
  plumbing upward is the habit — and the tempting reading, that a `use:` target will
  pick the table up, is wrong: the child resolves its own scopes. Now
  `proef::pack::bind_without_ref` at both scopes, with a message that says so.

- **A `ref:` step's `name:` reported a `${fake:…}` value it never sent.** A label is a
  *replay* of what the request was built from, not a fresh use of it: the inline path
  rewinds the `${fake:…}` occurrence counter, resolves the label, then restores it to
  the high-water mark. The `ref:` path reproduced that tail without the rewind, so a step
  binding `${fake:email}` and naming `${fake:email}` minted two identities — the console
  and the event stream announced one address while the request sent another, and every
  later step's fake values shifted by one. Both body forms now end in one shared
  `finish_step`, so the rule is stated and enforced in a single place rather than copied.

- **An escaped `$${secret:…}` in a `bind:` value was refused as a composite.** `$${` is
  the escape (ADR-0005), so `$${secret:token}` is the literal text `${secret:token}` and
  names no secret — but the composite check searched for the substring `"${secret:"`,
  matched at offset 1, and rejected the binding with `secret_in_composite_bind`. Both the
  whole-value and composite tests now read the value through the resolver's own
  reference scanner, so there is one thing that knows what a `${…}` is and `$${` stays an
  escape everywhere.

- **A step that set `retry:` twice ran the value it did not name.** A pack could
  declare `retry:` (or `delay:`) as a step key *and* again inside the block's own
  `[Options]`. Lowering extends an author's existing section rather than opening a
  second one, so proef's baked line landed *above* the author's; hurl resolves a
  duplicated option last-wins, and the raw value therefore won every time. The pack
  said `retry: 10`, the run did `retry: 3`, and nothing anywhere said so — the
  finite-retry lint only ever looked for `-1` and over-cap counts, so a plausible
  finite value passed untouched. Declaring an option in both places is now
  `proef::pack::option_declared_twice`, refused at load with the span on the raw
  line that used to take effect.

  The scan is deliberately scoped to `[Options]` sections rather than matching any
  `retry:`-shaped line: `retry` is a legal request-header name, and a header is
  `name: value` like an option is, so a line-shaped match would have turned an
  ordinary header into a hard error. Pinned by a test that a header named `retry`
  on a step carrying a typed `retry:` still loads.


## [0.9.0] - 2026-08-11 (tool-surface integrity & authoring guidance)

> **Breaking:** `proef secret set --value` was removed in favour of `--stdin`,
> and `proef macros --output json`'s `pattern` field changed from a boolean to
> `string|null`.

### Documentation

- **AUTHORING shows how to write a validation-error catalogue.** Two patterns
  that were reachable but not signposted, and that compose into one. A
  validation suite's cases differ *structurally* — one omits a key, one empties
  it, one adds a key the caller may not set — so a single parameterised macro
  cannot express them and an `Examples` cell cannot practically hold JSON; the
  answer is one named macro per malformation, whose sentence says what is wrong
  in business terms. The expectation side then does **not** grow with the
  catalogue: because an `expect:` merges into the *previous* request entry, one
  parameterised `the error code is {code}` covers every case in the set,
  typically the largest de-duplicator in a validation pack. That merging was
  documented as a mechanism in two sentences and never shown as the pattern it
  is. The cost is stated rather than hidden — the pack grows with the catalogue,
  which is what buys feature files a non-engineer can review.

- **An outline's `<column>` placeholders substitute into the docstring, and
  AUTHORING now says so.** They always have — TECH-SPEC §4.4 specifies it and
  the code has done it since — but the author-facing guide named only step
  text and table cells, and `StepDefn`'s own doc comment named the
  substitution on `text` and `table` while describing `docstring` as just
  "raw request bodies". Naming it twice and omitting it once reads as a
  deliberate exception, so a reader concludes the opposite of the truth: this
  is exactly the capability an author reaches for to data-drive a request
  body without leaving the feature file. AUTHORING gains a worked example.
  Pinned by tests for the first time — every other outline test asserts on
  step text, so a regression would have emitted a literal `<label>` into an
  artifact with the suite green.

### Fixed

- **`proef fmt` refuses a file that is not a pack.** It took an explicit path on
  trust, so it rewrote whatever it was pointed at: `proef fmt src/main.rs`
  stripped trailing whitespace from Rust source, printed `formatted:`, and
  exited 0. A mistyped path was a silent edit. Formatters parse before they
  write and refuse what they cannot parse; this one locates blocks textually, so
  the extension is the check available — and it is now the same predicate
  discovery already used, rather than a second opinion about what a pack is.
  Only the explicit-file path was affected: a directory was always filtered.

### Fixed

- **`proef fmt` leaves the YAML skeleton alone, as it always said it did.** Its
  documented scope is hurl blocks — the module doc promises the skeleton,
  comments included, is never touched, and the code claimed the trailing newline
  was the only normalization applied outside a block. Both were wrong: every
  line was trimmed. A pack whose blocks were already canonical failed
  `fmt --check` on nothing but a trailing space in a comment, which is a CI red
  an author cannot explain from the documented scope. This is the same
  over-reach the line-ending fix removed in 0.8.0, in the same function, one
  line above where that fix landed.

### Fixed

- **A truncated record no longer drops a warned scenario from its totals.**
  With no `run_finished` to read, `explain` recounts the scenarios present —
  and counted `Passed`/`Failed`/`Skipped` but not `Warned`, so a scenario whose
  `optional:` step warned vanished from every column. The live path counts
  `Passed | Warned` together (`RunSummary::passed` is "passed, warnings
  allowed"), so the reconstruction silently disagreed with the run it was
  reconstructing — and `optional:` exists precisely so a scenario can warn and
  still pass.

### Fixed

- **A failing run says when the scaffold's *routes* are still placeholders.**
  The scaffold has two halves to fill in, and a reader can have done either.
  Someone who follows `init`'s instruction — point `${url:base}` at your API —
  then hits the other half: `/health` and `/search` 404, and the target-side
  note deliberately cannot fire, because they *did* configure a target. They had
  been told about the routes once, parenthetically, two commands earlier. Now
  they are told at the failure. Decided from the pack's bytes, never from what
  the server answered: a 404 proves a route is missing, not that it is a
  placeholder, and inferring the second from the first is the class of claim
  removed in 0.8.0. The two notes are mutually exclusive — a reader with one
  unfinished half is told about that half, not handed a list.

### Fixed

- **`--dry-run`'s "next" command is the run that was validated.** After
  `--dry-run --env prod --tags smoke` it printed a bare `proef test`, which is a
  *different* run — another `[url] base` from the profile, and every scenario
  rather than the tagged subset. The operator could not tell: the command works
  and simply tests something else. Every selector that chose what ran is echoed
  now (`--env`, `--tags`, `--scenario`, `--scenario-file`, and the path), quoted
  so a tag expression or a scenario name with spaces survives a paste.
  Deliberately selectors only — a general "reprint the invocation" is how
  secret-bearing arguments reach stdout.

- **`--sarif` emits `startLine`.** GitHub keys inline annotations on it, so a
  log carrying only `byteOffset`/`byteLength` uploaded cleanly and annotated
  nothing — the flag looked wired up and delivered none of what it advertises.
  Sources are read once each at the IO edge and only to count newlines; `Diag`
  keeps carrying byte spans, and no column arithmetic is introduced.

- **`--watch` retriggers on `proef.toml`.** It watched the suite path
  recursively, and the config lives above it — so editing a `[url]`/`[vars]`/
  `[env.*]` value that every scenario resolves through changed nothing, which
  reads as the watcher being broken. Matched by exact path rather than by a
  `.toml` extension, so an unrelated manifest in the tree still does not
  requeue.

### Fixed

- **Three places interpolated a value into a format without escaping it.** Same
  shape each time, so they are fixed together:
  - **LSP completion snippets.** `$`, `}` and `\` are LSP snippet syntax, and a
    `match:` pattern is prose — prose carries `$`. `the price is $5` made the
    client read `$5` as tabstop 5 and drop the text, so accepting the completion
    inserted something the author never wrote. Literal characters are escaped
    now; the tabstops the generator writes stay syntax.
  - **GitHub annotations.** `file=` was passed raw while `title=` and the
    message beside it in the same `writeln!` were encoded. A path carrying `,`
    or `:` — every Windows path carries a `:` — broke the
    `key=value,key=value` parse.
  - **The GitHub job-summary table.** The scenario name and file went into
    Markdown cells unescaped; a `|` in either ends the cell and shifts every
    column after it, and the row still renders, which is why it goes unnoticed.

### Fixed

- **A templated `retry:`/`delay:`/`repeat:`/`max-time:` no longer under-counts
  the batch budget.** The estimator matched literal values only, so a
  `{{var}}`-driven option fell through and read as *no retries* — the budget was
  then computed for a single attempt, and the watchdog abandoned a scenario that
  was retrying exactly as authored, reporting it as an environment fault (exit
  3). A placeholder resolves inside hurl at run time and cannot be estimated, so
  the engine now says so: `batch_budget` returns `None`, whose contract already
  routes the batch to the orchestrator's default budget. An infinite count is
  treated the same way, since it is unbounded by definition. `TROUBLESHOOTING`
  described the old behaviour as if the budget could see these values; it now
  says what actually happens.

- **`--output json`'s `exit_code` is the code the process exits with.** A failed
  JUnit write escalates the run to 3, and that escalation was applied by a
  `return` *after* the body had been printed — so a machine consumer read a
  verdict the program then exited past, with nothing to signal the
  disagreement. The escalation is now folded in before anything serializes it.

### Documentation

- **The docs-drift backlog is closed.** `EDITORS.md` said go-to-definition
  cannot land on a `match:` line — it has since 0.5.1, and
  `definition_on_a_step_lands_on_the_match_line` proves it; the bullet now
  names the gap that is real (built-in macros live in a pack compiled into the
  binary, so there is nothing to open). TECH-SPEC §10's command surface gained
  `--run-id`/`--rerun`/`--sarif`. `GETTING-STARTED` no longer shows a scaffold
  comment with a word the scaffold does not write. ADR-0015 described a
  `worker` on `ScenarioFinished` that is always `None`, because that event is
  emitted from the dispatcher thread rather than the worker — an errata records
  what shipped, which `EVENTS.md` had right all along.

  Two entries did not reproduce and are recorded as such rather than dropped:
  `CONFIG.md` carries no claim that `[env.<name>.run]` overrides any section,
  and the 0.5.2 changelog does mention the directory-valued-phase error.

### Fixed

- **`proef fmt` keeps each line's own ending.** Its scope is hurl blocks, not
  line endings, but it split the whole file with `str::lines()` — which throws
  the terminator away — and rejoined with a single one. A file mixing CRLF and
  LF was therefore homogenized, and `fmt --check` came back red on a pack whose
  blocks were already canonical. The earlier fix moved from "always LF" to "the
  dominant ending", which still rewrote the minority lines. Terminators now
  travel with their line, so an untouched line is written back byte-for-byte;
  the only ending `fmt` still supplies is a trailing newline on a file that
  lacked one.

- **`proef --help` describes `macros` as it now behaves.** It still said "with
  its call count" after the command started printing the sentence each macro
  binds — the README table was updated and the clap text that actually produces
  `--help` was not.

### Documentation

- **`WRITING-SCENARIOS`'s two sample outputs match the binary again.** The
  `macros` sample showed two builtins with no ellipsis and omitted the
  `(builtin, unused here)` marker and the trailing count; the
  `missing_config_var` sample dropped the `(or in the active
  [env.<name>.url])` clause. Both read as verbatim transcripts, so a reader
  comparing them against a real run found differences that were the document's,
  not theirs.

### Fixed

- **`proef lsp` adopts the workspace root the client announces.** The root was
  resolved at the process edge, before the handshake, from the working
  directory — so an editor launched anywhere but the project analysed the wrong
  tree, and `nvim ~/proj/x.feature` from `$HOME` rooted the analyser at `$HOME`.
  The `initialize` params were bound and discarded. The server now reads
  `workspaceFolders`, falling back to `rootUri` (deprecated since LSP 3.16, and
  the spec is explicit that folders win when both are present) and then to the
  previous config-then-cwd resolution. `proef-lsp` still knows nothing about
  `proef.toml`: it calls back into the CLI, which owns config (ADR-0012).

### Added

- **The run record says which scenarios were lifecycle phases.** `phase`
  (`"setup"`/`"teardown"`) is now on `scenario_started`/`scenario_finished` —
  additive and optional (ADR-0008), so older records read as "no phases", which
  is what they had. Without it a teardown scenario was indistinguishable from a
  suite one except by feature path, so every consumer re-derived phase
  membership from `proef.toml` and three of them got it wrong in different
  ways. Fixing them off one signal is what the three entries below have in
  common.

### Fixed

- **A mixed suite+phase failure kept the phase label.** `explain` chose the
  label from the whole report (`failed == 0`), so it appeared only while *every*
  failure was a phase failure — and vanished the moment a suite failure joined
  one, leaving `1 failed` above two indistinguishable blocks. The
  disambiguation disappeared exactly where it was needed. Labelled per block
  now, from the record.

- **`--rerun` after a phase-only failure says there is nothing to rerun.** It
  returned the failed teardown, which `build_specs` cannot match because the
  phase is excluded from the pool — producing a run that matched nothing and
  reported "no scenarios matched the filters (check --tags/--scenario)", naming
  flags the operator never passed. Phases are invisible to `--rerun`
  (ADR-0014); it now exits 0 saying so.

- **`diff` no longer counts a failing teardown as a test regression.** A cleanup
  fault makes `test` exit 3, not 1, so blending phases into the regression
  buckets made `diff --fail-on-regression` contradict the run it was diffing.
  Phase scenarios are excluded from the verdict and the exclusion is reported.

- **Records written before 0.6.0 no longer report the wrong verdict with
  confidence.** They carry one `run_finished` per phase and their totals counted
  every phase; read under today's suite-only meaning, a genuine suite failure
  was reported as `1 passed · 0 failed` and labelled setup/teardown. The
  `schema` field cannot distinguish them — that change was semantic and never
  bumped it — but the structure can. `explain` now detects the multiple pairs,
  recomputes the totals from the scenarios present, and says the record predates
  0.6.0. A reader must be able to consume a record *or* detect that it cannot;
  quietly doing neither was the one unacceptable option.

### Fixed

- **`proef init` no longer destroys a `proef-pack.schema.json` you wrote.** The
  never-overwrite loop walks a fixed four-entry array; the schema is not in it,
  and is written afterwards by the shared installer. So the one unguarded path
  was pack-absent + schema-present: `init` scaffolded the pack, then the
  installer replaced an authored file — reported as `created 5 file(s), skipped
  0`, while the README promised the opposite in as many words. `init` now asks
  the installer to preserve what is already there and reports it as skipped;
  `proef schema --add-to` still refreshes, since that is an explicit install and
  how the schema is updated after upgrading proef.

### Changed

- **`proef secret set --value` is gone; use `--stdin`. Breaking.** A secret in
  argv is visible to anyone who can run `ps`, and the failure path *steered
  people to it* — the hidden prompt's error said "pass `--value` in scripts",
  which fires exactly in the non-TTY/CI case where the exposure matters. There
  is now no flag that takes a value: `--stdin` reads it from a pipe (same shape
  as `docker login --password-stdin`), stripping the trailing newline the pipe
  added, and the prompt stays the default. Scripts using `--value` must pipe
  instead: `printf %s "$TOKEN" | proef secret set NAME --stdin`.

### Added

- **`proef doctor` reports a missing pack schema.** `init` installs it
  automatically, but noticing when it is *absent* never shipped — so a suite
  whose editor completion had been silently off had nothing telling it so.
  Reported as a warning, never a failure: it costs autocomplete and load-time
  validation in the editor, not a run, and `doctor`'s exit is the environment
  verdict. Uses the same predicate `init` uses, so the two cannot disagree about
  what "installed" means. Runs outside a project too — no config or no suite is
  reported, not failed.

- **`bind::unbound_step` names `proef macros` again, from the CLI.** The pointer
  was removed from the diagnostic in #25 for a correct reason — that text also
  renders in an editor's diagnostics pane through the LSP, where the affordance
  is completion, not a command — but nothing put it back on the terminal side,
  so a terminal reader saw it zero times. It is now added by the CLI's own
  renderer, which legitimately knows it is the CLI. The core diagnostic still
  names no tool.

### Fixed

- **The first-run note no longer fires on real suites.** It keyed on `[url] base`
  still equalling the value `proef init` writes — which looks init-specific and
  is not: `GETTING-STARTED` teaches that exact line to people building a suite
  by hand, and proef's own `proef.toml` uses it. So a hand-built suite whose
  server was up and whose assertion genuinely failed was told "this suite is
  still the `proef init` scaffold — its target and its routes are placeholders,
  so it cannot pass yet": every clause false, moments after the suite reached a
  real verdict. The deciding evidence is now the run itself — the note appears
  only when **nothing was reachable** (no scenario passed and every outcome is a
  system fault). A suite that got an HTTP response, even a 404, has a target;
  whether its routes are placeholders was a guess, and the note stated it as
  fact. Wording softened accordingly.

- **Suite discovery no longer walks build output, and one unreadable directory
  no longer empties the suite.** The walk had no exclusions, no depth bound, and
  a `canonicalize()` per directory — and it re-runs on every language-server
  request, so entering `target/` cost that price over and over for a subtree
  that cannot contain a suite. It now skips `target/`, `node_modules/`,
  `vendor/` and dot-directories (tested on children only: a suite may
  legitimately be rooted *at* such a name), and refuses beyond 32 levels rather
  than recursing until the stack runs out. A `Permission denied` on one
  descendant used to abort the entire walk, and `proef lsp` swallowed that error
  into an empty analysis — so a single unreadable subdirectory silently emptied
  the suite. Unreadable descendants are now skipped, the way `find` and ripgrep
  do; an unreadable *root* is still a loud error, because that path is the
  caller's own.

- **Ctrl-C no longer skips cleanup in silence.** Teardown shared the run's
  cancellation token, so an interrupt left every teardown scenario `Skipped` —
  and because a skipped phase carries no fault, the worst-wins fold passed it
  without a word. Whatever setup created stayed created and nothing said so,
  against this ADR's own premise that suite cleanup is reliable. Teardown now
  runs on its **own, independent** token (not `child_token()`, which cancels
  with its parent and would have re-implemented the bug): the pool stops at its
  batch boundary, the operator is told cleanup is running, and it completes. A
  second Ctrl-C still hard-exits (130) — the escape hatch ADR-0007 relies on —
  and the announcement says so. Amends ADR-0014.

- **A phase that only *skipped* is now a failure, not a pass.** That silence was
  the shape that hid cancelled cleanup. A setup completing no scenario aborts
  the run rather than letting the suite execute against state setup never
  created — which is also what keeps teardown gated on setup-success, since the
  abort is the gate; a teardown completing no scenario is reported and fails.

- **`--dry-run` validates `[run] setup` and `[run] teardown`** — which ADR-0014
  always claimed ("validated like any other feature but never executed") and
  nothing did: `--dry-run` never read the keys. A broken teardown therefore
  surfaced only after a full suite had run — real requests, a run directory,
  artifacts — while the identical mistake in `setup` failed in milliseconds.
  Both are now validated by one loader shared with `proef test`, which also
  pre-flights teardown **before** the pool. A bad phase path is a user error
  (exit 2) rather than a blanket system fault (exit 3), and creates no run
  record.

### Changed

- **`proef macros` prints the sentence, not just the identifier.** A test author
  writes prose that binds to a vocabulary somebody else maintains — and the one
  command that lists that vocabulary showed `health` where the author needs
  `the service is healthy`. The `match:` pattern was already loaded and already
  linted; both renderers discarded it on the way out. It now appears in the text
  listing, and `--output json`'s `pattern` field carries the string itself
  (`null` when a macro is `use:`-only) instead of a bare boolean.

### Added

- **`proef macros` answers when the suite does not bind.** Listing the
  vocabulary previously required every scenario to bind — so the command
  refused in exactly the situation that sends an author looking for it: a step
  that matched no macro. It now prints the diagnostics, then the vocabulary the
  packs offer, and keeps its exit code unchanged (2), so scripts see no
  difference. Pack loading precedes binding and does not depend on it, so the
  listed vocabulary is complete. **Every count-derived verdict is withheld in
  that mode** — `calls`/`unused` render as `—`/`null` rather than `0`/`false`,
  because a feature that failed to bind contributes no calls and would
  otherwise make its own macros look dead. `proef flows` deliberately still
  refuses: its contract is to list *every* scenario, and a partial list that
  silently omits the unparsed feature is the wrong answer, not a degraded one.

- **A failed run says when the suite is still the untouched scaffold.** A
  freshly scaffolded project cannot pass — its target and its routes are both
  placeholders — and `init` says so once, two commands earlier, in a
  parenthetical the failure never referred back to. The run now names the
  situation and the remedy. It fires only on the conjunction (`[url] base`
  still byte-identical to what `init` wrote **and** no `PROEF_BASE_URL`): an
  operator who set the override *did* name a target, so their failure is about
  their API and is not second-guessed. Exit codes are untouched — whether an
  unreachable target is a user or a system fault is a taxonomy question decided
  in the engine (ADR-0009), and the reader's actual problem is vocabulary.

### Fixed

- **`proef schema --add-to` and `proef init` now announce the schema file they
  write.** Both wrote `proef-pack.schema.json` silently, so `init` listed four
  files and then reported "created 5 file(s)" — the first output a new user
  reads, not reconciling, with the unannounced file being the one that powers
  editor completion.

- **`proef init` no longer sends you to install editor completion that is
  already installed.** A re-run named `proef schema --add-to` unconditionally,
  even with the schema sitting beside the pack. It now says which of the two
  situations you are in.

- **The nextest harness no longer reports green having listed no tests.** A
  `PROEF_HARNESS_SUITE` set to bytes that are not valid UTF-8 read as *unset*,
  which the harness treats as "expose nothing" on purpose — so `cargo test`
  passed having run zero scenarios. A `PROEF_BIN` it could not read fell back
  to `proef` on `PATH`, silently invoking a different binary than the one
  named. Both now surface as a failing `proef::config` trial, the same loud
  shape the harness already used for flows-contract drift, whose comment
  states the invariant this violated: never run zero tests green.

### Documentation

- **One worklist instead of four documents to cross-read.** Four files read like
  backlogs and only one was: `OPEN-FINDINGS` now carries every open item, including the
  residue of both UX reviews (R1–R3) and the decisions taken against them, each entry
  self-contained. The two review documents were **removed** once their open items landed
  there — their transcripts and citations remain in git history, and a retired review
  left on disk is exactly the thing that reads as a backlog. `IMPROVEMENT-PLAN` stays a separate file — five
  ADRs cite it by section number — but its master table gained a **Status** column,
  because its ✅/⚠️ glyphs mean "fits the architecture", never "done", and **13 of its
  16 items had already shipped** while the table gave no way to tell. Item 14's cited
  mechanism (`Refs::default()` resetting per `lower()` call) was corrected: 0.6.0
  replaced it, and only the cross-scenario half of that caveat still holds.

- **A page for the persona the product is named after.** PRD §4's first persona
  writes prose against a vocabulary somebody else maintains — and every
  document labelled "test authors" taught pack authoring, so that reader had no
  route through the tool. `docs/WRITING-SCENARIOS.md` covers only their loop:
  what a sentence is, how to list the ones available, the dry-run cycle, and
  the two diagnostics they will actually hit. The index now labels each
  author-facing page with the persona it serves instead of calling six P2
  documents "test authors".

- **`bind::unbound_step` leads with the action its reader can take.** The help
  opened on "add a macro to a pack" — the pack maintainer's move, which a
  scenario author cannot make — and buried theirs in a parenthetical. It now
  opens with matching a sentence the suite's packs already bind. It names no
  tool: `Diag.help` reaches an editor's diagnostics pane verbatim through the
  LSP as well as the terminal, and each front end already has its own way to
  show the vocabulary (completion in the editor, `proef macros` in a shell) —
  `proef-core` does not know which one is reading. The YAML stub is unchanged:
  it is load-bearing for the maintainer and stays verbatim.

- **ADR-0014 now records the question it was silent on.** It is specific about a
  failing setup and a failing teardown, so a reader reasonably infers the
  *cancellation* case was considered — it was not. What teardown does on Ctrl-C
  is unspecified, and today it silently skips: the phase runs with the
  already-cancelled token, every scenario resolves `Skipped`, and `phase_failed`
  ignores a phase that only skipped, so cleanup never runs and nothing says so.
  The ADR now states the gap and the two defensible answers, since an
  implementer working on teardown reads the ADR, not the findings list.

- **The open-findings list is now in the repo, not on one machine.** A v0.5.3
  review was validated claim-by-claim (40 claims, 38 confirmed) and the record
  lived only in a gitignored scratch directory, so ~26 still-open defects — the
  Ctrl-C teardown gap, LSP rooting, `--sarif` line numbers, several docs drifts
  — existed nowhere durable. `docs/OPEN-FINDINGS.md` carries them, plus what
  shipped against them, so a fixed finding is not re-reported and an open one is
  not lost.

- **`proef init` is now in the command tables it was missing from.** It shipped
  in 0.6.0 and was documented in `GETTING-STARTED.md` and in the README's prose,
  but not in the README's CLI table or `TECH-SPEC.md`'s command surface — so the
  two places a reader scans for "what can this tool do" both omitted the command
  that starts a first run.
- `CLAUDE.md`'s status list now records the v0.6.0–v0.8.0 correctness series
  rather than ending at post-M5, so the three releases that closed the
  reports-success-on-wrong-output bug class are visible to anyone picking the
  project up.

## [0.8.0] - 2026-08-09 (CLI output & exit integrity)

### Changed

- **A set-but-unreadable environment variable is now a loud user error, never
  silence — breaking for a pipeline that relied on the old silent fallback.**
  `std::env::var` collapses "unset" and "set to bytes that are not valid
  UTF-8" into the same `Err`; `.ok()` erased that distinction at five call
  sites, so a value proef could not read was indistinguishable from one the
  user never set. A non-UTF-8 `PROEF_KEY` fell through to the key file and
  decrypted with the wrong key, reporting tampering instead of the real cause
  (and `doctor` reported the key source as the file instead of the override);
  a non-UTF-8 `PROEF_SECRET_<NAME>` fell through to the store and reported a
  missing secret; a non-UTF-8 `PROEF_ENV` ran silently against the wrong
  environment, including in `proef lsp`, where it meant analysing against the
  wrong config profile. Four of the five sites now exit 2 (user error) naming
  the variable; `doctor` instead reports it as a failed check alongside its
  other unready-environment findings and exits 3, the same as an unreadable
  key file. A pipeline that today tolerates a mis-set `PROEF_ENV`, or a
  non-UTF-8 key/secret, will start failing after this upgrade.
- **A failed stdout write now reaches the exit code — breaking for a pipeline
  that tolerated truncated output.** Writing to a full disk or other failed
  stdout exited `0` with truncated output; it now exits `3`. A closed pipe
  (`proef … | head`) still exits cleanly. A pipeline that captures proef's
  stdout somewhere that can fail mid-write (a full disk, a device error)
  previously reported success over truncated output; it now gets a nonzero
  exit it can act on instead of trusting truncated bytes. Per
  `docs/RELEASING.md`, any breaking change is MINOR — together with the
  environment-variable change above, this forces the next release to be
  0.8.0, not 0.7.1.

### Fixed

- **`proef fmt` rewrites line endings wholesale, violating its hurl-blocks-only
  promise.** `fmt` split pack files with `text.lines()` (which strips both `\n`
  and `\r\n`) and rejoined with hardcoded `"\n"`, so CRLF files became LF. On an
  `autocrlf` checkout (a supported way to clone this repo), `fmt --check` was
  permanently failing through no fault of the author. `fmt` now detects the
  file's dominant line ending and preserves it when rewriting.

- `run.log` could gain duplicated fragments when the console accepted a
  short write, because the tee re-wrote the full slice on every retry. It
  now mirrors only the accepted bytes.

- `proef report -o` outside the run dir wrote artifact links relative to
  the run dir, so every link 404'd from the report's own location while
  the command reported success. The href is now absolute when the report
  is written elsewhere.

- `proef diff` reported a brand-new retried step as newly flaky, because a
  step absent from the base run was assumed to have run once. Steps with no
  baseline are now skipped, and the ordinal-shift caveat inherent to
  positional step keying is documented in TROUBLESHOOTING.

## [0.7.0] - 2026-08-07 (record & artifact integrity)

### Changed

- **`${fake:…}` values no longer repeat across a scenario's steps.** The
  occurrence counter restarted on every step, so two steps each asking for a
  fresh `${fake:email}` received the same address. Every independent
  `${fake:…}` reference within a scenario — across steps, and within one
  step's payload/`when:`/label — now gets its own value and never collides
  with another, however many a single step ends up resolving. A step's
  `name:` label (shown in artifact comments and events) is the deliberate
  exception: it is not independent of its own payload, so it replays from
  the start of the step's own occurrence window instead of minting new
  ones, matched by position (the label's Nth `${fake:…}` reference reuses
  the payload/`when:`'s Nth occurrence, regardless of generator kind) — so
  it reproduces the payload's own value when the label's references mirror
  the payload's in kind and order, and shows a different generator's output
  when they don't. Even a label with *more* `${fake:…}` references than its
  payload still reserves each extra one, so a later step can never be
  handed a value the label already displayed. Values remain deterministic
  for a given
  `--run-id`, but suites using `${fake:…}` will see their emitted artifacts
  change. **Known limitation, not fixed here:** the counter resets at the
  start of every scenario, not the run, so two *different* scenarios that
  each resolve `${fake:email}` at the same position in their own step order
  still collide — that is a separate bug with its own snapshot-moving fix.
- **`proef_core::resolve::resolve` changed signature** (public API break for
  downstream `proef-core` consumers): it now takes an additional `&mut
  usize` occurrence counter supplied by the caller, and `Resolution::fakes`
  was removed — `resolve()` no longer owns the counter itself.

### Fixed

- **`run_finished` is once again the last line of a run record.** A scenario
  the watchdog abandons keeps running on a detached thread and only notices
  its cancellation token at the next batch boundary, so it went on appending
  events after the sweep had recorded its outcome — and after the run itself
  was finalized. `docs/EVENTS.md` has always said the last line is
  `run_finished`; it was not, so anything reading a record as a stream (the
  JSONL consumer, `report`, `explain`) could see events arrive after the
  terminal one. Late events from a finalized scenario are now dropped at a
  single gate rather than by asking every emitter to check. Abandonment
  itself is unchanged and stays cooperative (ADR-0007) — only the record's
  tail is affected.

- **`.map.json` no longer loses a request's captures when the pack comments
  one of them.** A comment inside an open `[Captures]` run is the author's
  note about a capture, not the start of the next entry, so it no longer
  closes the scan — previously it dropped every capture after the comment.
  The entry that follows opens with a method or response line, and that
  closes the run on its own.

- **`.map.json` no longer lists captures that were never made.** The sidecar's
  capture scan was fence-unaware — a literal `[Captures]` line inside a
  fenced (```…```) body re-armed it — and it recognised only the stock HTTP
  methods, so an entry opened by a custom method (`PROPFIND`, …) never ended
  the previous scan. Both let capture names that don't exist in the emitted
  entry land in `.map.json`, a normative artifact (ADR-0010). The scan is now
  fence-aware and shares the lowering pass's method recogniser
  (`is_method_line`) instead of carrying a second, weaker copy.
- **`pack::empty_expect` now also catches a whitespace-only `hurl:` fragment.**
  The diagnostic already existed for an `expect:` item with neither `status:`
  nor `hurl:` at all; a `hurl:` key present but carrying no non-blank assert
  line slipped past it, lowered to an empty asserts block. It also gains a
  remediation hint and the seeded corpus case it was missing. **Scope:** this
  check reads the *unresolved* pack text, so a fragment that is non-blank as
  authored but resolves to nothing at lower time (e.g. `${vars:key}` naming a
  `proef.toml` value that is `""` in the active environment, or an unset
  `${global:key}` under `--dry-run`) still lowers to an empty asserts block —
  see the sidecar-emitter entry below for how that residual case is handled.
- **The sidecar emitter can no longer produce an inverted `.map.json` span.**
  A `Then` step whose asserts all resolved to nothing — reachable even after
  the `pack::empty_expect` widening above, since pack validation cannot see
  what a fragment resolves to, only what it says — lowered to a zero-line
  merged-asserts step, and the emitter's line-span arithmetic underflowed:
  the start offset exceeded the end. Such a step now gets no sidecar row at
  all instead of an inverted one — nothing was appended to the artifact, so
  there is nothing to report a span for.

## [0.6.0] - 2026-08-07 (first-run UX & run-record correctness)

### Added

- **`proef init` scaffolds a working suite.** It writes the files
  `GETTING-STARTED.md` teaches — `proef.toml`, one `.feature`, one matching
  pack — installs the pack JSON Schema for editor completion, and prints the
  next command. Nothing is ever overwritten, so a second run is a no-op and no
  `--force` flag exists to destroy authored work. A test asserts the scaffold
  passes `--dry-run` unchanged.
- The README now shows a parameterized macro and states the load-bearing
  non-goals, including the supported path for teams that already have a hurl
  corpus.

### Changed

- A passing `--dry-run` now names the next command. Every failure path already
  named a remedy; the success path stopped talking at the moment a new user
  decides whether to continue.
- **A scenario with no steps is now an error, not a silent pass — breaking.**
  A `Scenario:` with a commented-out or never-written body previously bound to
  nothing, ran nothing, and exited 0; it now exits 2, through `proef test`,
  `proef flows`, the libtest-mimic harness, and `proef-lsp` (which re-analyzes
  on `didChange`, so a half-typed `Scenario:` now shows a live error while
  you're still typing it). Per `docs/RELEASING.md`, any breaking change is
  MINOR — this forces the next release to be **0.6.0, not 0.5.4**.

### Fixed

- `resolve::missing_config_var` now suggests the closest key defined in the
  same namespace, matching `resolve::unknown_variable` and
  `resolve::fake_unknown`. Candidates are namespace-scoped, so a `${url:…}`
  typo can never suggest a `[vars]` key. The code also gains the seeded corpus
  case it was missing.
- `proef init` no longer rewrites a pack it declined to create. Installing the
  editor modeline ran unconditionally, so a hand-authored `suite/packs/api.yaml`
  reported as "already exists" was still modified; the schema install is now
  gated on the file having been created, and an existing pack gets a hint
  naming `proef schema --add-to` instead.
- **Setup and teardown no longer corrupt the run record.** Each phase bracketed
  its own `run_started`/`run_finished`, so one record held up to three pairs and
  `proef explain` reported the last phase's totals — printing "1 passed ·
  0 failed" above a failure it had just listed. The record now carries one
  pair, and its `run_finished` totals are the main suite's own verdict —
  `[run] setup`/`teardown` scenarios still appear as their own events in the
  record, but are never folded into `passed`/`failed`/`skipped`, so those
  numbers agree with the console `summary:` line, JUnit, `--output json`, TAP,
  the SLA gate, and the exit code. The console run header also prints once per
  run instead of once per phase.
- **`report` and `explain` flag a truncated run.** Both rendered an incomplete
  record as if it were whole; `explain` also derived its headline solely from
  the missing tail event, reporting all zeros for a record that held completed
  scenarios. Both now read through the same record reader `diff` uses.
- **`explain`'s step/attempt totals count a still-in-flight scenario.** A step
  only attached to the record once its `ScenarioFinished` landed, so a
  scenario still running when a truncated record's stream ended had its step
  evidence silently dropped from the headline — the one place a post-mortem
  tool most needs it. Totals now fold the raw events directly instead.
- **`explain`'s failure detail is keyed `(file, scenario)`, not scenario name
  alone.** Two same-named scenarios in different files previously bled each
  other's failure output together.
- **`worker` is the slot a scenario occupied, not a per-scenario counter.** The
  timeline drew one lane per scenario regardless of `--jobs`.
- **Run rotation only treats hyphenated UUID directories as run records.** The
  parser also accepted bare 32-hex, `urn:uuid:` and braced spellings, which
  rotation could then delete when the runs directory points somewhere shared.
- The nightly canary can fail again: its step piped through `tee` without
  `pipefail`, so a red canary exited 0 and the open-an-issue step was
  unreachable.
- The raw-print-macro guard now covers `proef-lsp`, where stdout is the
  JSON-RPC channel and a stray print corrupts protocol framing.

### Documentation

- The stdout/stderr macro rule is now written down where contributors look:
  `docs/CONTRIBUTING.md` ("Rules that are easy to trip over") and `CLAUDE.md`.
  0.5.3 began enforcing it with a source-scanning test, so a raw `println!` or
  `eprintln!` in `proef-cli` failed the suite with nothing explaining the rule
  or naming `render::outln!`/`errln!` as the sanctioned spellings.

## [0.5.3] - 2026-08-06 (closed-pipe safety)

### Fixed

- **The CLI no longer panics when stderr is a closed pipe.** Every remaining
  raw `eprintln!` in `proef-cli` now routes through the EPIPE-safe `errln!`
  guard added in 0.5.2, so `proef test … |& head` ends the pipeline with the
  contracted exit code instead of aborting with 101 — a code outside the typed
  0/1/2/3 taxonomy (ADR-0009). The execution failure summary, which writes
  several lines per failing scenario, was the largest remaining exposure. A
  source-scanning test now keeps raw `eprintln!` out of the crate.
- **The language server no longer dies while recovering from a panic.**
  `proef-lsp` reports a caught analysis panic on stderr; that report used a raw
  `eprintln!`, which panics when its write fails — so a closed stderr (EPIPE)
  took down the very server the surrounding `catch_unwind` exists to keep
  alive. The write is now explicitly unchecked. Ships without a test: reaching
  the line needs a real analysis panic *and* a closed stderr, and the panic is
  not injectable without a test-only hook in shipping code; the mechanism
  itself is already covered by the CLI's closed-pipe tests.

### Changed

- `proef report` derives its output directory through the shared
  `fsutil::parent_dir` helper instead of an open-coded empty-parent fallback,
  so there is one spelling of that derivation. Internal consistency only — the
  emitted artifact links are unchanged.

## [0.5.2] - 2026-08-05 (CLI correctness)

### Fixed

- **A directory-valued `[run] setup`/`teardown` is now a loud user error.**
  ADR-0014 defines setup/teardown as a single feature file; a directory ran
  every feature under it as the phase and again in the pool (a silent
  double-run) — that path is closed.
- Diagnostics no longer panic when stderr is a closed pipe: `print_all` and
  `report_front_error`'s trailing `"{errors} error(s)"` summary line are now
  routed through an EPIPE-safe `errln!` guard (mirroring `outln!`'s stdout
  guard), so `proef test --dry-run <broken suite> |& head` exits cleanly
  instead of panicking (exit 101).
- `diff` step records are now keyed by `(text, occurrence ordinal)` instead of
  text alone — macro-expanded steps that share text no longer collide in the
  last-write-wins map and silently drop out of the diff.
- `diff --fail-on-regression` now fails when the new run is incomplete or
  cancelled (was a silent pass), and banners any incomplete/cancelled record
  in the diff output either way. Its slower-step duration math is hardened
  against overflow (saturating arithmetic).
- **A bare-filename `[run] setup`/`teardown` (or suite path) now resolves its
  packs and assets from the current directory.** A path with no directory
  component (e.g. `setup = "setup.feature"` at the project root) has an empty
  `Path::parent()`, which produced a `cannot read directory` failure; it now
  normalizes to `.` (the current directory) via a shared `fsutil::parent_dir`
  helper at the pack/asset base-derivation sites.

### Documentation

- The second-interrupt hard-exit code **130** (128+SIGINT) is now documented
  for `test` and `watch` (TECH-SPEC §10, ADR-0009) — a deliberate escape
  hatch outside the typed 0/1/2/3 `ExitCode` taxonomy.

## [0.5.1] - 2026-08-05 (LSP go-to-definition + correctness)

### Added

- **LSP go-to-definition: `use:` references and `match:` landing (ADR-0017).**
  Go-to-definition now jumps from a `use:` reference in a pack to the macro it
  targets, and lands on the macro's `match:` line rather than its name key
  (falling back to the name key for use-only macros with no `match:`).

### Fixed

- **LSP: the stdio server now exits cleanly.** `proef lsp` dropped the connection
  after joining the transport threads, so the writer thread (holding the sole
  channel Sender) never ended and the process leaked. It now drops the connection
  before joining. Covered by a real stdio subprocess lifecycle test.
- **LSP: a malformed request no longer crashes the server.** A bad document URI or
  out-of-range position propagated a deserialization error out of the event loop
  and exited the process; the request now gets an `InvalidParams` (-32602) reply
  and the server keeps serving.
- **LSP: one broken pack no longer blanks the whole suite.** `analyze_suite` now
  keeps the packs that loaded (and reports the broken one's diagnostic) instead of
  zeroing all bindings, completion, and go-to-definition on any pack error.
- **LSP: analysis is scoped to the configured suite.** The server roots at
  `[run] suite` (else the `tests/` convention) under its launch directory rather
  than walking the entire working tree, sharing the CLI's suite resolution.
- **LSP: unsaved edits are honored for paths with special characters.** The
  open-buffer overlay is keyed by source name instead of the raw file URI, so a
  path segment containing sub-delimiters (`(`, `+`, `'`, …) no longer misses.

### Documentation

- Documented `proef-lsp` and the `lsp`/`macros`/`diff`/`report` subcommands across
  the README, TECH-SPEC CLI/dependency references, and the RELEASING publish order.

## [0.5.0] - 2026-08-04 (LSP language server)

### Added

- **`proef lsp` language server (ADR-0017).** A server-only, generic-LSP stdio
  binary — a second front-end over the sans-IO core — giving feature/pack authors
  live editor support: **diagnostics** (the whole `--dry-run` validation set,
  republished across the suite as you type), **go-to-definition** (Gherkin step →
  the macro that binds it), **completion** (macro-pattern step completions,
  prefix-ranked by relevance to the typed prose), and **find-references** (every
  step a macro binds). Wired into Neovim/Helix/Emacs via generic LSP config — see
  `docs/EDITORS.md`. No VS Code extension in v1. `proef.toml` config is a startup
  snapshot (restart the server after editing it). Works on Linux, macOS, and
  Windows. Pinned `lsp-server 0.7.9` / `lsp-types 0.97.0`.
- **New `proef-core` public surface** enabling the language server: the injectable
  `SourceProvider` seam (`proef_core::provider`), the collect-all `analyze_suite`
  analysis (`proef_core::analyze`) — the same headless analysis the CLI runs,
  driven over an overlay-then-disk provider so the LSP re-validates the whole suite
  on every edit — and `matcher::prefix_rank` for prose-prefix completion ranking.
  All keep the core sans-IO (the IO is injected).

## [0.4.0] - 2026-08-03 (external config & environments; competitive-review breadth)

### Added

- **Suite setup & teardown (`proef.toml [run] setup`/`teardown`, ADR-0014).**
  Each names a feature run once around the whole suite (the Playwright/Jest
  `globalSetup` model). `setup` runs before the parallel pool and merges its
  `saveAs: global` promotions into the shared store **before any scenario
  lowers**, so it seeds fixtures/shared state every scenario reads via
  `${global:…}`; `teardown` runs once after for cleanup. A setup failure aborts
  the run as a user/system fault (never a test failure, exit 1); teardown runs
  only if setup succeeded and its failure is a distinct exit 3 (never a silently
  green suite). Both are excluded from the pool, so a setup/teardown feature
  inside the suite never also runs as an ordinary scenario.
- **`proef test --output tap`** — a TAP version 13 stream to stdout, one test
  point per scenario, derived from the run's own outcomes (not from hurl), for
  `prove`/`tappy` and TAP-native CI. The human report moves to stderr (as with
  `--output json`). `@quarantine` scenarios map to the `# TODO` directive
  (their failure does not gate); skipped scenarios to `# SKIP`; failure detail
  rides in a redacted YAML block. `--output tap` is rejected on `flows`/`macros`
  (a user error, not a silent human fall-back).
- `proef macros` now flags **near-duplicate** pattern macros — two that differ
  only in their `{capture}` names (identical literal skeleton), which are
  confusable to authors. Advisory only (never gates the exit code); `--output
  json` gains a `nearDuplicateOf` field beside `unused` for a CI hygiene check.
  The heuristic is deliberately tight (skeleton equality), so a legitimately
  similar family with distinct literals is left alone.
- Localized Gherkin (`# language:`) is now verified and test-covered — a
  localized feature parses, its dialect keywords are stripped, and a localized
  scenario outline with `Examples` expands like any other. Outline detection now
  keys primarily on `Examples` presence (dialect-independent) with the English
  keyword as a fallback, so this no longer relies on an English-only heuristic.
  (A localized outline that omits its `Examples` still degrades to an
  unbound-step error, since gherkin 0.16 does not expose its dialect keywords.)
- **Built-in `expect:` shape-macro library.** The embedded `Core` pack gains a
  curated, product-neutral set of response-shape assertions — `the value at
  {path} is a string` / `… a number` / `… a boolean` / `… a uuid` / `… an ISO
  date` / `… present` / `… a non-empty list` — each merging one hurl type
  predicate (`isString`/`isUuid`/`isList` + `count`, …) into the previous
  request. It is a convenience layer over the existing `expect:` mechanism (no
  new engine capability, no marker DSL); the raw-hurl assert vocabulary still
  covers anything the macros don't.
- **Run-level SLA gate (`proef.toml [sla]`).** An opt-in latency budget: after a
  run, per-step wall-clock durations fold into `p95-ms` (95th-percentile ceiling)
  and `max-ms` (slowest-step ceiling); a breach prints the offending metrics + the
  slowest steps and maps to **exit 1** (a test failure). It is off by default (no
  `[sla]` table = no gate, run byte-identical to before), env-overridable via
  `[env.<name>.sla]`, introduces no new exit code, and never downgrades a
  `User`/`System` fault. Distinct from hurl's per-request `duration <` assert —
  the gate is an aggregate budget over the whole run. Skipped steps are excluded
  from the population.

- **External config & environments (`proef.toml`, ADR-0012).** New `[url]` and
  `[vars]` tables hold non-secret suite variables, referenced in packs as
  `${url:<key>}` / `${vars:<key>}`; `[env.<name>.<section>]` profiles deep-merge
  per-environment overrides over the base tables (`url`/`vars`/`http`/`run`).
  `proef test --env <name>` (or `PROEF_ENV`) selects the active environment.
  `proef.toml` is discovered by searching up from the working directory (like
  cargo/git), so it is found from any subdirectory. Adds the
  `proef::resolve::missing_config_var` diagnostic.
- **Default suite path.** `[run] suite` sets the path `proef test`/`flows`/
  `artifacts` use when given none (falling back to the `tests/` convention), so
  `proef test` runs with no argument. An explicit path still wins.
- Documentation set completing the corpus: `docs/DIAGNOSTICS.md` (all 57
  diagnostic codes, corpus coverage marked), `docs/CONFIG.md` (`proef.toml`
  reference), `docs/EVENTS.md` (the `events.jsonl` wire schema for CI),
  `docs/TROUBLESHOOTING.md` (exit codes, glyph legend, frequent failures),
  `docs/CONTRIBUTING.md` and `docs/SECURITY.md` (threat model, private
  vulnerability reporting), and an IDE-integration section in AUTHORING.
- `proef test --scenario-file <file>`: scope a `--scenario` name filter to
  one feature file (duplicate scenario names across files stay disjoint;
  the libtest-mimic harness uses it to keep the Trial↔scenario bijection).
- `scenario_finished` events now carry a `file` field — the run-wide scenario
  identity alongside `scenario` (additive, ADR-0008; absent in older records).
- Diagnostics `pack::pattern_duplicate_capture` (a `{capture}` written twice)
  and `lower::kind_unrouted` (internal registry-drift safety net).
- `proef macros` lists every loaded macro with its call count and flags
  user-pack pattern macros that no scenario binds (dead prose bindings);
  `use:`-only helpers and unused builtins are listed but never flagged.
  `--output json` for CI dead-code gates.
- `proef test --run-id <id>` pins the injected run id (like `artifacts --run-id`),
  so a run's `${fake:…}` data — which keys on the run id — is reproducible; the
  JSON summary echoes the id.
- `proef test --dry-run --sarif <path>` serializes validation diagnostics
  (unbound steps, pack lint, non-finite retries) to a SARIF 2.1.0 log — a
  shift-left gate that renders findings as inline PR annotations. The export is
  additive: the dry-run's exit code is unchanged.
- `proef test --rerun` re-runs only the scenarios that failed in the last run
  (read from its JSONL record, keyed on the run-wide `(file, name)` identity);
  it composes with `--tags`/`--scenario`, and reports "nothing to rerun"
  (exit 0) when the prior run was clean.
- `@quarantine` tag: a scenario so tagged runs and reports normally, but its
  *test-failure* no longer gates the exit code (a `System`/`User` fault still
  does — quarantine is for flaky tests, not broken input or infra). A note
  prints when a quarantined scenario fails, so it is never silently swallowed.
- `proef diff [base] [new]` compares two run records (defaulting to the previous
  and latest runs) and reports scenario status transitions — regressed, fixed,
  still-failing, new, removed — keyed on the run-wide `(file, scenario)`
  identity, plus per-step flakiness (rising retry counts) and perf deltas
  (steps diffed on `text`, never the volatile authored line). It is a derived
  view over `events.jsonl`, never a second record (ADR-0008); `--fail-on-regression`
  exits 1 when a scenario regressed, for CI gating.
- Flaky-failure detail: a step that passes only after a retry now records the
  messages from its earlier, failed attempts as `attempt_details` on the
  `step_finished` event (additive, ADR-0008); JUnit surfaces them as
  `<flakyFailure>` under the passing test case, so a green-on-retry run is honest
  instead of indistinguishable from a clean pass. The engine already collected
  the earlier-attempt errors — they were being discarded on success.
- `proef report [run-id]` writes a self-contained HTML report for a run —
  scenario tree with pass/fail pills, per-step attempts and timing, a
  per-scenario timing waterfall (each step's bar offset by the steps before it
  and as wide as its own duration — the sequential cascade within a scenario,
  derived purely from step durations), a **cross-worker timeline** (a lane per
  worker, each scenario a bar on a shared run-relative axis, so concurrency is
  visible at a glance), failure detail, and deep-links to the executed `.hurl`
  artifacts (bodies are not inlined).
- **Injected run timing (ADR-0015).** `scenario_started`/`scenario_finished`
  events gain optional `timestamp_ms` (run-relative) and `worker` (0-based
  index) fields, stamped at the CLI sink on the worker thread so the sans-IO
  core stays clock-free. Additive (absent on records without timing); they power
  the HTML timeline. Records without them degrade to the waterfalls alone. A pure `proef_core::html::render_html` derives it from the event
  stream (ADR-0008, snapshot-locked); the events are already redacted at the
  sink, so the page is too. Defaults to `report.html` inside the run dir; `-o`
  redirects it.

### Changed

- **`--tags` is now a boolean expression, not a comma-separated list.** It takes
  a single expression over `and`/`or`/`not` and parentheses (the `@` stays
  optional), e.g. `--tags "@api and not @slow"`; a bare tag still works. The
  grammar and evaluator live in the sans-IO core (`proef_core::tags`,
  deterministic and fuzzed); a malformed expression is a user error (exit 2), as
  is a selection that matches nothing. This replaces the old CSV OR-list — there
  is one selection mechanism, not two.
- `--output` is a typed value: an unknown format (e.g. a `jsonl` typo) is a
  user error (exit 2) instead of silently degrading to the human report.
- `--watch` reruns only on `.feature`/`.yaml`/`.yml` changes — the watched
  tree can now contain proef's own run output without a self-trigger loop.
- The example corpus (`tests/features/`) and the dev fixture use a neutral
  workspace / activity-board domain (record · note · event · attachment ·
  session · channel) — no product-specific vocabulary.
- `CHANGELOG.md`, `CONTRIBUTING.md`, and `SECURITY.md` moved under `docs/`
  (root keeps only `README.md` and `CLAUDE.md`).
- **Pack root key renamed `templates:` → `macros:`** (ADR-0004 amendment): one
  canonical spelling for the prose→engine binding layer (the entry is a *macro*,
  the file a *pack*). No `templates:` alias — packs using the old key fail to load.
- The dev-loop fixture (`cargo run -p xtask -- fixture`) binds the advertised
  default port **8787** — falling back to an ephemeral port (and printing a
  `PROEF_BASE_URL` line) only if 8787 is busy; `... -- fixture <port>` overrides.
  So `proef.toml`'s default `base` reaches it with no `PROEF_BASE_URL` export
  (ADR-0011 amendment). Its `GET /health` now returns a versioned identity —
  `name`, a numeric `version` (`1.0`), and the RFC 3339 `time` it answered.
- The unbound-step diagnostic (`bind::unbound_step`) now prints a paste-ready
  pack-macro stub — quoted tokens in the sentence become `{argN}` captures —
  alongside the existing did-you-mean suggestion, so an author can add the
  missing macro without hand-writing the `match:`/`hurl:` scaffold.
- CI reporting surfaces failures and flakiness more honestly. Under GitHub
  Actions the run emits a `::error file=,line=,title=` annotation per failure
  (rendered in the PR "Files changed" gutter; gated off when `--output json`
  owns stdout). The job summary gains a **flaky passes** section and per-failure
  attempt counts, and the JUnit report records "passed on attempt N" for a
  scenario that only went green after retries — a silent green-on-attempt-2 is
  no longer invisible.
- `docs/AUTHORING.md` gains an "Asserting responses" cookbook surfacing the hurl
  8.0 predicate/filter/RFC-9535-JSONPath vocabulary that raw `hurl:` blocks
  already accept — documenting existing capability, not new engine work.
- A failed step now prints a `curl:` reproduce line — the redacted `curl` for the
  failing request, surfaced from the embedded engine via a new engine-agnostic
  `StepOutcome.reproduce_hint` — so a failure can be replayed request-by-request
  without leaving the terminal. Secrets are masked.

### Removed

- **The `# key: value` feature-file directive mechanism** (e.g. `# baseURL:`,
  ADR-0012 amendment). Variables now have exactly one home — `proef.toml`
  (`[url]`/`[vars]`) — so a `.feature` file can no longer define a variable
  (one-way-to-do-one-thing). `#` comment lines stay valid gherkin comments; they
  are simply no longer parsed. The env-override the directive provided is
  preserved by embedding `${env:NAME:-default}` in a config value (resolved
  recursively). `${…}` plain-name resolution is now `args > defaults` only.

### Fixed

- Optional-batch error path no longer double-reports later batches into the
  JSONL run record (ADR-0008); `saveAs: global` promotions are no longer
  dropped when the store lock is poisoned; the event sink recovers from a
  poisoned lock instead of truncating the record.
- `expect:` merge scopes to the last entry (fence-aware); `[Options]`
  injection can no longer duplicate a section; the `use:` graph walk is
  node-linear instead of exponential on multi-edge chains.
- The embedded-hurl version lockstep is now asserted by a test; the encrypted
  secret store maps user vs. environment faults to exit 2 vs. 3 (ADR-0009);
  run.log / artifact-write / malformed-`proef.toml` failures surface instead
  of being swallowed.

## [0.3.1] - 2026-07-29 (secret-management hardening)

### Added

- `proef secret rm NAME` removes a stored secret (locked atomic rewrite;
  removing an absent name exits 2).
- `PROEF_KEY` env override supplies the project key directly (base64) — a
  committed ciphertext store now decrypts in CI without shipping the key
  file; a set-but-invalid key errors instead of silently falling through.
- `proef doctor` reports secret store/key health (readable, parseable,
  private permissions); a corrupt `.proef-secrets.json` no longer bricks
  `secret set` — it is moved aside to `.corrupt` and a fresh store begins.

### Fixed

- **Secret-valued captures never reach `.proef-state.json`**: a
  `saveAs: global` capture whose value equals a known secret is refused —
  the owning step warns with the reason — closing the one sink the
  redaction invariant (ADR-0005) did not cover.
- Secret resolution reads the store and key once per run instead of once
  per secret (no torn view against a concurrent `secret set`).
- Warned steps now print their reason on the console (`↳ …`) — a bare ⚠
  glyph explained nothing, for `optional:` failures too.

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
