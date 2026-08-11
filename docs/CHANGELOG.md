# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning follows [SemVer](https://semver.org) (policy in `docs/RELEASING.md`).

## [Unreleased]

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

> **Contains breaking changes — the next release is a MINOR bump, not a patch**
> (`docs/RELEASING.md`): `proef secret set --value` was removed in favour of
> `--stdin`, and `proef macros --output json`'s `pattern` field changed from a
> boolean to `string|null`.

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
