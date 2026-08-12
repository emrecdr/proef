# proef — open findings

**This is the worklist.** Every open defect and gap lives here, whichever review found
it. Each entry is self-contained: the evidence, the reasoning, and — where something was
declined — why.

**Companion:** [IMPROVEMENT-PLAN](IMPROVEMENT-PLAN.md) is the *feature* roadmap (own
numbering, 13 of 16 shipped) and stays separate because five ADRs cite it by section
number. [CHANGELOG](CHANGELOG.md) records what shipped, per release.

**Provenance.** Three reviews fed this list, each validated claim-by-claim against the
tree and then retired into it:

| Review | Scope | Contributed |
|---|---|---|
| v0.5.3 external (2026-08-06) | 40 claims → 38 confirmed, 1 partial, 1 already fixed | the A/B/P/Q items below |
| first-run UX (0.5.3, engineer's first 30 min) | F1–F4 | **R1–R2** |
| non-technical UX (0.8.0, PRD §4 P1 calibration) | N1–N5 | **R3** |
| round-7 pre-merge review of PR #13 | 4 defects + residue | **§2.1–§2.4 below**; three shipped in #31 |
| corpus-port report (0.8.0, a real 844-line hurl suite ported) | 12 items → 3 shipped (#41, #43), 2 premises corrected | **M/E/D items below** |

The review documents themselves were removed once their open items landed here; their
full text, transcripts and citations are in git history (`git log --diff-filter=D
-- docs/FIRST-RUN-UX-REVIEW.md docs/NON-TECHNICAL-UX-REVIEW.md`). The shipped/open split
was re-checked against `main` on **2026-08-10**.

**Read the citations as "start reading here", not as addresses.** They were accurate on
2026-08-06 and files have moved since; locate symbols with `rg`, not line numbers.

---

## Shipped since validation

Kept here so the list reads as live rather than stale, and so a finding is not
re-reported after it is fixed.

| ID | Finding | Shipped in |
|---|---|---|
| P10 | Abandoned-scenario events appended after `RunFinished` — worse than reported: past the *run's* terminal event, not just the scenario's | #15 |
| B11 | `${fake:*}` collided across a scenario's steps | #15 |
| P9 | `.map.json` gained phantom capture rows (fence-unaware scan, unrecognised custom methods) | #15 |
| B1 | Whitespace-only `expect:` produced an inverted sidecar span `[9,8]` | #15 |
| P2 | Non-UTF-8 `PROEF_KEY`/`PROEF_ENV`/`PROEF_SECRET_<NAME>` read as *absent* | #18 |
| P6 | Full disk: `--output json` exited 0 with truncated JSON | #18 |
| P7 | No stdout-side pipe-close test (both existing ones closed stderr) | #18 |
| P1 | Tee re-wrote the full slice on every `write_all` retry, duplicating `run.log` tail bytes | #18 |
| P8 | `proef fmt` rewrote CRLF → LF wholesale | #18 |
| Q3 | `report -o` outside the run dir shipped dead relative artifact hrefs | #18 |
| B8 | `diff` flagged a brand-new retried step as flaky | #18 |
| A3 | `CLAUDE.md` status stopped at post-M5 | #21 |
| N1 | First run reported `system error` with no explanation (NON-TECHNICAL) | #24 |
| N2 | `proef macros` printed identifiers, never the `match:` sentence | #24 |
| N3 | `macros` refused to list when any step failed to bind | #24 |
| N4 | `unbound_step`'s help led with the pack maintainer's action | #24 |
| N5 | No document described the scenario author's workflow | #24 |
| §8 | `init` announced four files and reported five | #24 |
| Q5 | Ctrl-C skipped teardown silently — cleanup never ran, nothing said so | #26 |
| Q4 | `--dry-run` validated neither `[run] setup` nor `[run] teardown` | #26 |
| R2 | `doctor` did not report a missing pack schema (FIRST-RUN F4b's second half) | #29 |
| B7 | `secret set --value` put the secret in argv, and the error text steered to it | #29 |
| §2.2 | `init` destroyed an authored `proef-pack.schema.json` (round 7) | #30 |
| §2.3 | a mixed suite+phase failure lost the phase label exactly when it disambiguated | #31 |
| §2.4 | `--rerun` after a phase-only failure blamed filters never passed | #31 |
| §2.1 | pre-0.6.0 records reported the wrong verdict with confidence | #31 |
| — | `diff` counted a failing teardown as a test regression | #31 |
| P4 | `fmt` homogenized mixed-endings files beyond its hurl-blocks-only promise | #33 |
| P4 | `proef --help` described `macros` with pre-prose wording | #33 |
| P4 | `WRITING-SCENARIOS`' two sample outputs drifted from the binary | #33 |
| — | `init` destroyed an authored `proef-pack.schema.json` (round-7 §2.2) | #30 |
| Q7 | `fuzz_tag_expr` compiled but was in neither fuzz loop | #30 |
| B3 | `windows.yml` built and tested without `--locked` | #30 |
| B13 | `justfile` gate list omitted `public-api` (and the fuzz gate) | #30 |
| B5 | `explain`/`diff`/`report` each inlined `ProjectConfig::load()` | #30 |
| A6 | TROUBLESHOOTING's exit table omitted `130` | #30 |
| A4 | README's ADR range and flag rows, and TECH-SPEC §10's command surface, were stale | #30, #34 |
| A5 | TECH-SPEC's `publish` claim and its run-dir inventory were stale | #30 |
| A1 | `EDITORS.md` claimed go-to-definition cannot land on a `match:` line | #34 |
| A7 | `GETTING-STARTED`'s copy of the scaffold comment had a word the scaffold does not | #34 |
| P11 | ADR-0015 described a `worker` on `ScenarioFinished` that is always `None` | #34 |
| B2 | a templated `retry:`/`delay:` under-counted the batch budget, abandoning healthy scenarios | #35 |
| B4 | `--output json`'s `exit_code` disagreed with the real exit after a JUnit failure | #35 |
| B6 | LSP completion snippets did not escape `$`/`}`/`\` | #36 |
| B9 | GitHub annotation `file=` and job-summary table cells were unescaped | #36 |
| — | the `--dry-run` nudge echoed a command that was not the run validated (round-7) | #37 |
| P3 | `--sarif` emitted no `startLine`, so it annotated nothing | #37 |
| P5 | `--watch` did not retrigger on `proef.toml` | #37 |
| — | a run against untouched scaffold *routes* got no coaching (round-8 §5) | #38 |
| — | truncated-record fallback totals dropped `Warned` scenarios (round-7) | #39 |
| — | `fmt` rewrote any file handed to it, not just a pack | #40 |
| — | `fmt` trimmed the YAML skeleton, turning `--check` red outside its scope | #40 |
| C1 | negative-case authoring had no signposted catalogue form | #43 |
| C3 | `expect:` composition documented as a mechanism, never shown as the pattern | #43 |

**Q7 is now closed** (#30): `fuzz_tag_expr` is in both fuzz loops as well as the
compile gate.

---

## Open — round-9 residue (ingested 2026-08-12)

The review's P1/P2 and three P3s shipped in #48 and #50. What follows is what was
verified and deliberately not built, so none of it depends on remembering.

### R9-1 — `proef fragments` has no listing command

`flows` lists scenarios and `macros` lists the vocabulary; nothing lists the
corpus. There is no way to ask which fragments exist, which are referenced, or
which `.hurl` entries carry no annotation — and an unannotated entry is *dropped
at scan time by design*, so the tool structurally cannot report what it never
built.

Raised by a consumer migration whose coverage gate ("every `@proef` name is
referenced, every entry is annotated") had to become a script that repo owns.
**Not built for 0.10.0 on purpose:** new public surface, and the migration was
unblocked by correcting its own gate instead.

### R9-2 — fuzz coverage does not reach the fragment surfaces

`fuzz_pack_load` runs with an empty corpus, so `ref:`/`bind:` clash logic never
executes under fuzzing; the annotation scanner's entry-boundary arithmetic —
proef's own code, not hurl's — and `bake_entry_options`' textual injection are
unfuzzed entirely. Split the fuzz input into pack and corpus halves, and consider
a `fuzz_fragment_scan` target (nightly, accepting the native-libs cost).
`fuzz_tag_expr` also still compiles in gates while sitting in neither fuzz loop.

### R9-3 — no resource bounds on the corpus read

No per-file or file-count cap: a multi-GB `.hurl` is read whole on every command
that loads packs. Pairs with the read-resilience work in #48, which made the read
*survivable* but not *bounded*.

### R9-4 — a bind that shadows a capture is silent

hurl's `variable:` assigns into one shared set, so a pack- or macro-scope `bind:`
re-assigning a name an earlier entry captured overrides it for every later entry,
with no diagnostic. A warning shaped like `option_declared_twice` fits — the
difference is that this one is only decidable where the capture set is known, at
lower time.

### R9-5 — `{{x}}` inside a bind value is unvalidated at lower time

It fails at run time instead of at `--dry-run`: loud, but late, and the late half
is what `--dry-run` exists to prevent.

### R9-6 — provenance is cwd-dependent

Run from a subdirectory and `step_finished.fragment`, explain's `via`, JUnit and
the diagnostics carry an absolute machine path; the record-portability claim holds
only from the project root. Relativize against the config root rather than cwd —
the same boundary `[run] fragments` already resolves against.

### R9-7 — smaller edges, verified and recorded

Artifacts written *inside* a fragments root poison the corpus with proef's own
output (loud, but the remedies misdirect — skip files carrying the artifact
header, or document it); a step-scope `bind:` key the fragment never reads is
silently baked as a run-level `variable:` and can shadow a later capture, and an
unused `${secret:}` bind silently widens the required-secret set (warnable at step
scope, where it is decidable); a `# @proef` annotation placed mid-entry is
silently ignored and the resulting `unknown_ref` does not hint at misplacement;
`proef macros` prints a corpus error twice on the degraded path; same-file
duplicate annotations read as "declared in both f.hurl and f.hurl".

## Open — round-7 residue (ingested 2026-08-10)

The round-7 pre-merge review of PR #13 never entered any worklist; a round-8
revalidation re-reproduced its findings against v0.8.0. §2.2, §2.3, §2.4 and the
`diff` item shipped in #30/#31. What remains, carried on that report's evidence
rather than re-reproduced here:

- **The early-error record** — *reproduced 2026-08-11, needs a decision.*
  `proef test --tags <nothing-matches>` prints the error and then a
  `summary: 0 passed · 0 failed · 0 skipped` line, and the record it leaves is
  `run_started` + `run_finished 0/0/0` — byte-indistinguishable from a clean run
  of an empty suite. A post-mortem reader cannot tell "errored before dispatch"
  from "ran nothing successfully".

  The fix is a design call, not a patch. Suppressing the tail on this path would
  leave the record *incomplete*, which the tooling already banners correctly —
  but `RunRecord` emits its tail structurally, on `Drop`, precisely so no return
  path has to remember it, and adding an exception reintroduces the fragility
  that design removed. Opening the record later is blocked by setup, whose
  scenario events need it. The third option is an additive event carrying the
  early error (ADR-0008 permits it) — the most honest and the most work.

## Open — residue of the two UX reviews

Verified against `main` on 2026-08-10. Everything else those reviews raised has shipped
(first-run: F1, F3, F4a and F2's did-you-mean in 0.6.0 · non-technical: N1–N5 and the
`init` count in #24).

### R1 — `missing_config_var`'s span points at the sentence, not the pack line

The diagnostic reports at the *feature* step that used the variable, e.g.
`suite/case.feature:3:5`, rather than the pack line where `${url:bse}` actually appears —
so the reader goes hunting. The did-you-mean half shipped in 0.6.0; this half did not,
**deliberately**.

*Why it was deferred, in full — this is the whole reasoning, do not re-derive it:*
`ResolveError` carries no position, and `resolve()` is documented "pure and total".
The comparable diagnostic that *does* land on a pack line (`pack::invalid_hurl`) gets
its position from hurl's own parser reporting a line/column, which feeds
`locate::payload_line_span(…, rel_line)`; nothing computes a `rel_line` for a resolve
failure. Supplying one means threading an offset out of a deliberately position-free
pure function and carrying pack identity to the diagnostic site. That is a design
change, not a fix — it wants its own spec.

Two sibling extensions were **declined** at the same time: `resolve::missing_env` must
not suggest from the injected environment snapshot (it would surface unrelated
environment variable names in diagnostics, against the secret-masking posture), and
`resolve::unknown_namespace` already enumerates all seven valid namespaces. Sibling
codes share a *shape*, not a *candidate set*.

### R3 — the scaffold default is the dev fixture's port *(declined 2026-08-11)*

`init.rs` writes `base = "${env:PROEF_BASE_URL:-http://127.0.0.1:8787}"`, which is
proef's own dev fixture port — so to someone who installed a binary and has no
fixture, the value *looks* configured and is not. The proposal was an obvious
placeholder (`https://api.example.com`) to cover **prevention**, since a failing run
already covers recovery.

**Declined, with the reasoning recorded rather than a silent skip.** Recovery is now
covered on *both* halves: an unreachable target and untouched routes each get their
own note (#28, #38). The remaining benefit is that the config file would read as
obviously unfilled. Against that, `init.rs`'s module doc states the scaffold
deliberately mirrors what `GETTING-STARTED` teaches — so changing the literal changes
the tutorial too, and the tutorial's "run it against `xtask fixture` with no
`PROEF_BASE_URL`" flow stops working. That flow is a real onboarding asset for
contributors. Trading a working tutorial for a more obviously-fake string is not worth
it once the failure itself explains both halves.

Revisit if first-run drop-off is ever measured rather than reasoned about.

### Decided against — do not re-raise

Recorded as decisions, so they are not rediscovered as fresh ideas.

- **Re-classify the unconfigured-scaffold failure from exit 3 to exit 2.** Not a
  CLI-edge change: the verdict is set in `proef-engine-hurl` (`classify_error`'s
  `_ => Infra` arm), `Fault::System(String)` carries no kind to match on, and the exit
  derives in `proef-core` (`RunSummary::exit_code_excluding`). Both routes — string-
  matching the engine's opaque message, or adding a structured kind to core's public
  surface — cost more than the value, which is *vocabulary*. The note delivers that, and
  fires on the exit-1 placeholder-route path a re-classification would have missed.
- **Degrade `proef flows` the way `macros` degrades.** `flows` promises *every*
  scenario; a list silently omitting the feature that failed to parse is a wrong answer,
  not a degraded one. `macros` degrades safely only because pack loading precedes
  binding and does not depend on it.
- **Ship `proef-fixture` in the binary so the scaffold's first run passes.** Needs a new
  ADR (it is dev-only today), enlarges the binary and the security posture of a test
  runner with a listening server — and R3 plus #24's note remove the need.
- **A GUI, web UI, or "no-terminal" mode.** PRD §3 forecloses dashboard/server mode. The
  P1 gap was always about *vocabulary and error text*, never a second interface.
- **Importing or round-tripping hand-written hurl**, and **anything OpenAPI-shaped as a
  recurring oracle.** PRD §3 and ADR-0016 permanent non-goals.

---

## Open — correctness

Q2 is the remaining Tier 1 branch. Q5 and Q4 shipped in #26.

### Q2 — the walk still happens twice per request *(remainder)*

**Shipped in #27:** the walk skips `target/`, `node_modules/`, `vendor/` and
dot-directories, is depth-bounded, and no longer aborts the whole discovery on
one unreadable subdirectory (which `analyze.rs` swallowed into a silently empty
analysis). **Shipped in #32:** the server adopts the workspace root the client
announces — `workspaceFolders`, else `rootUri`, else the previous
config-then-cwd resolution — so an editor launched outside the project no longer
analyses the wrong tree.

**Still open.** `analyze.rs` discovers packs and features with two independent
walks, on every completion/definition/references request, and completion
requests are not debounced. The excludes cut what each walk costs; they do not
stop it happening twice. Halving it means either caching per analysis pass or a
combined discovery call — both need a way to invalidate, which the
`SourceProvider` trait has no hook for today. Purely a cost item now that the
root is correct: no wrong answers depend on it.

### P5 — watch: the atomic-save half *(remainder)*

**Shipped in #37:** `--watch` now also watches `proef.toml`, matched by exact path.

**~~Closed by inspection~~ — the inspection was invalidated by a later change, and
the bug shipped.** The original argument was: the retrigger filter is an allowlist
of `.feature`/`.yaml`/`.yml`, and no run-record file (`.jsonl`, `.log`, `.hurl`,
`.vars`, `.json`, `.xml`, `.html`) matches it. ADR-0018 then added the engines'
fragment extensions to that allowlist — `.hurl`, named in this very paragraph as
the thing that could not match — while every run writes
`.proef-runs/<id>/artifacts/*.hurl`. A watched tree containing its own runs dir
fed itself: **49 runs in 15 seconds**, firing real traffic in a tight loop.

**Now closed by construction, not inspection.** The retrigger filter excludes
generated trees **by directory name**, reusing discovery's own `skipped_dir`, so
there is one rule with two consumers rather than a second list to drift; the
configured `[run] runs-dir` is passed in for the case where it is not a
dot-directory. `watch::tests` pins both halves — that an emitted artifact never
requeues, and that a fragment edit still does.

The lesson is the general one: a "closed by inspection" note records a conclusion
whose premise nothing watches. This one even enumerated the fact that later became
false. Prefer a test that would fail when the premise changes.

**Still open:** "a single watched file dies after an atomic save". It did **not**
reproduce on macOS/FSEvents; `notify`'s own docs say it is real but
platform-dependent and worst on inotify. Do not chase it on a Mac — that is how it
gets "fixed" by coincidence. It needs a Linux reproduction first.

---

## Open — adoption and execution model (ingested 2026-08-11)

Source: a report written while porting a real 844-line hurl corpus onto proef —
field evidence rather than inspection, which is why it found a different class
from the review rounds. Every claim below was re-checked against `main` before
filing; where the report was wrong, the correction is recorded with the item.

**Already closed from it:** the docstring-placeholder documentation gap (#41).
Two of its claims did not survive checking, and both are noted in place (M1, D2).

**The through-line.** These are *adoption*, not correctness. The first-run path
is finished and the correctness series closed its bug class; the next constraint
is whether a team with an existing hurl suite can move onto proef and
**demonstrate** they lost nothing. M1 and M2 are that story. E1 is the first wall
a real suite hits afterwards.

### F1 — `proef.toml` now has two path-resolution rules

`[run] fragments` resolves relative to the **config file's directory** (ADR-0018);
`suite`, `setup`, `teardown` and `runs-dir` stay relative to the **working
directory**. The reasoning that produced the new rule — the config is found by
walking *up*, so a path in a config three levels above must mean "relative to the
project" — applies verbatim to all five keys, and `setup`/`teardown`/`runs-dir`
are consulted on every run rather than only when a path was omitted.

Cost: one file with two semantics and no marker distinguishing them. A user with
`setup` and `fragments` in the same `proef.toml` gets one working from a
subdirectory and one not, and every future path key re-litigates the choice
against four precedents for the older rule.

**Not fixed here on purpose.** Changing the four existing keys is a behaviour
change for every project that already relies on cwd-relative resolution, which
is out of scope for the change that introduced the fifth. The fix is a single
`ProjectConfig::resolve_path` used by every path accessor, shipped deliberately
with a changelog note — recorded so it is a decision rather than an oversight.

**ADR-0018 (named hurl fragments) lands into this section — read it against these
items before assuming what it closes.** It lets a pack `ref:` a named entry in a real
`.hurl` file, so a corpus file is annotated once instead of transcribed, and stays
runnable under stock `hurl`. Item by item:

- **M1** is *not* closed and must not be built concurrently — both touch `fmt`
  discovery. ADR-0018 requires the opposite of M1 at one entry point (directory
  discovery must never sweep `.hurl` into the pack formatter) while leaving M1's actual
  ask untouched (an explicitly named `.hurl` may be canonicalized). Sequence them,
  either order, never at once.
- **M2** is *not* closed. ADR-0018's integration test runs one fragment both ways,
  which proves a file is dual-runnable; it does not compare two suites' result sets.
- **M3** is *unanswered and now overtaken*: the charter re-examination M3 asked for has
  happened (PRD §3 amendment) without the measurement it asked it to rest on. The
  amendment argues from the non-goal's own rationale instead, and says so. Measuring
  the port cost is still worth doing — it now informs priority rather than permission.

### M1 — `fmt` cannot canonicalize a standalone `.hurl` *(report A2, reframed)*

**The report had this backwards** and it is worth recording why. It claimed `fmt`
*refuses* a file outside a pack, and proposed teaching it to accept `.hurl` as a
small plumbing change. `fmt` in fact accepted any file and rewrote it — two
defects fixed in #40, which now makes it refuse `.hurl` **correctly**, since
applying YAML block-location logic to hurl syntax would be nonsense.

So the item survives but changes shape: making it real means teaching `fmt` to
recognize a hurl file and run the block canonicaliser over the whole thing, with
no `hurl:` key to locate. That is a feature, not a flag.

**Why it still ranks first.** It is what converts M2 from clerical to mechanical,
and it is the cheapest unlock for the most valuable capability.

### M2 — no mechanical equivalence check between a hurl corpus and its proef port *(report A1)*

**Verified.** `Diff` takes `base`/`new` **run ids** only (`main.rs:187-195`); no
path reads a `hurl --report-json`, which the pinned hurl 8.0.1 does emit.

**Why it matters.** The safe way to adopt proef is to run both suites until the
new one is trusted. During that window nothing *proves* the two assert the same
things, so the equivalence gate degrades to a hand-maintained mapping table
reviewed once by a human — and that table is what a team's decision to delete
their old suite rests on.

**Scope.** Not the hurl-import non-goal in disguise (`PRD.md:42`). Import means
reading `.hurl` and *generating* Gherkin. This compares two **result** sets,
which is `diff`'s existing job with one more input format. The non-goal
forecloses a direction of data flow, not the ability to check your own work.

### M3 — the port cost has never been measured *(report A3)*

`PRD.md:42` makes hurl import a **permanent** non-goal, and that rests on
persona P3's "pastes between corpus and packs" (`PRD.md:57`) being cheap —
which nobody has measured. A 14-file, 844-line port is the first real datum
available. Recording the hours settles a recurring argument in one direction or
the other: cheap vindicates the non-goal with evidence instead of assertion,
expensive earns the charter a re-examination with numbers rather than opinion.

### E1 — no intra-run serialization primitive *(report B1)*

**Verified.** `TECH-SPEC.md:313` — scenario ordering is "preserved for artifact
naming, not execution order." No `serial` tag or config key exists anywhere in
core, cli, `CONFIG.md` or `AUTHORING.md`.

**Why it matters.** Real suites contain scenarios that mutate global state — the
reporting corpus has two, one needing an empty database for absolute `items[N]`
assertions and one installing a workflow definition governing everything created
afterwards. Neither can run in a parallel pool, and proef offers no way to say
so; the workaround is several CLI invocations driven by tag discipline in a
Makefile.

**Charter fit.** Scheduling, not a new engine or execution mode — the
orchestrator already decides what runs when, and `[run] setup`/`teardown` prove
the surrounding concept is in charter. Those cover *before* and *after* the pool
and nothing *inside* it.

**Options.** A reserved `@serial` tag, or `[run] serial-tags = [...]`. The config
form is more explicit and keeps runner semantics out of the feature files — and
E4 is an argument for it.

### E2 — N invocations produce N run records, with no merge *(report B2; consequence of E1)*

**Verified.** Each run writes its own `.proef-runs/<run-id>/` (`TECH-SPEC.md:299`).

E1's workaround therefore yields N records, N JUnit files, N HTML reports, and
pass/fail aggregation pushed onto the caller's shell, while `explain`/`diff`
operate per-run so a post-mortem reader must know which to open. **Recorded as a
consequence, not an independent item** — solve E1 and this largely evaporates;
solving it alone (a `proef merge`) treats the symptom.

### E3 — no per-scenario state reset hook *(report B3)*

**Verified.** `[run] setup`/`teardown` are whole-suite only, run once around the
pool (`CONFIG.md:63-64`, `120-141`).

Any suite against a real database wants before-each; today isolation is
convention (title prefixes so scenarios do not see each other's rows) and
convention has no guardrail. proef knows nothing about databases, so "reset the
DB" cannot be a proef feature — but framed as *a feature file run before each
scenario* it is the same primitive as `setup` at a different scope, which is
engine-agnostic by construction. The cost is real: it multiplies run time by
scenario count and interacts with parallelism. **This needs an ADR against
ADR-0014, not a patch**, and it may well be declined — deliberately rather than
never asked.

### E4 — nothing enforces tag-group discipline *(report B4; record, do not build)*

If E1 ships as a tag convention, a scenario added six months later lands untagged
in the parallel pool and breaks isolation intermittently — the worst failure
mode, because it reads as flakiness. A lint would have to guess which endpoints
are global, which proef cannot know. Its value is as a marker: this is the
follow-on cost of the tag form of E1, and therefore an argument for the config
form.

### D1 — no first-class requirement traceability

**Verified.** `flows --output json` prints one object per scenario
(`main.rs:128-137`), which with tags like `@FRD-3.1-create` gets most of the way.
Almost certainly a **documented recipe rather than a feature** — proef should not
learn what a requirement is — but the recipe does not exist, so every team
reinvents it and the capability is not advertised for this use.

### D2 — report generation across N runs *(premise partly corrected)*

**The report overstated this.** It claimed a Makefile must capture the run id
because proef needs `proef report <run-id>`; in fact `run_id` is optional and
defaults to the latest run (`main.rs:199-201`), so the ordinary single-run case
needs nothing captured.

What survives is the compounding with E2: with N invocations, "the latest" is one
of N. Minor on its own, and listed because report-generation friction is felt by
every CI integration rather than by one team.

### Positive evidence — recorded so it is not undone

- **The raw-hurl paste path covered 100% of a real corpus.** All 844 lines used
  only `[Asserts]` (75) and `[Captures]` (15) — no `[Options]`, `[Query]`,
  `[FormParams]` or `[Cookies]` — with seven ordinary predicates (`==`, `exists`,
  `not exists`, `matches`, `count ==`, `>=`, `isString`), every one passing
  through untouched. **The strongest evidence yet for ADR-0004**, and the kind of
  claim that gets doubted later.
- **`proef macros` printing sentences (#29) is load-bearing.** The porting plan
  gated its prerequisite phase on it, purely to author 14 files of new prose.
- **`--rerun`** (`main.rs:120-122`, re-run only the last run's failures) fits
  conversion iteration exactly.

### Suggested order

M1 → M2 (adoption becomes provable) → E1 (dissolves E2). The two documentation
items, C1 and C3, shipped in #43. E3, E4, D1, D2 and M3 are record-only — none
blocks anyone today.

---

## Closed — docs drift (2026-08-11)

Every item in this section shipped; the table above records which PR each landed in.
Two did not reproduce when re-checked, and are recorded here rather than dropped, so
the next reader does not spend the same time on them:

- **A2** — `CONFIG.md` was said to claim `[env.<name>.run]` overrides any section. It
  carries no such claim today: its precedence text names `jobs` specifically, which is
  what `RunOverride` actually allows.
- **B12** — the CHANGELOG's 0.5.2 entry was said to lack a line about the
  directory-valued-phase hard error. It has one, first bullet under **Fixed**.

One half of **A5** was deliberately not acted on: TECH-SPEC §11's run-dir inventory
lists the files a run *generates*, and the `[run] setup`/`teardown` features are inputs
named by config, not run-dir output. The reviewer called this half "defensible-but-
interpretive" and it is; `report.html`, which the inventory genuinely omitted, was added.

---

## Open — maintainability and CI

| ID | Finding |
|---|---|
| B10 | The canary would chase a hurl **prerelease** (no semver filter) |
| P12 | The matcher re-tokenizes per `(step, pattern)` pair on every bind *(performance)* |
| P13 | No World snapshot/restore proptest; no CI workflow runs `llvm-cov` |
| Q1 | `EngineLowering` seam: structured payloads are **unreachable** today (pack validation rejects them first) — design debt, zero live misbehaviour |
| Q6 | `html.rs` re-derives the emitter slug (the event schema carries no slug field); `exec.rs`'s own comment forbids exactly this. Four `file_stem()` sites — correct today, future-drift risk |

---

## Open — deferred during the v0.6.0–v0.8.0 correctness series

Found while fixing the above; each was validated and consciously left out of scope.

- **`proef-harness` `PROEF_BIN`/`PROEF_HARNESS_SUITE`** — fixed in #19, but the same
  reader is now duplicated in `proef-cli` and `proef-harness`. Justified today (a binary
  crate cannot be depended on; these are the only two `env::var` callers in the tree).
  **Tripwire: at a third caller, promote it to a shared crate.**
- **Capture-name charset** is narrower than hurl's grammar, so an out-of-charset name is
  silently omitted from `.map.json`.
- **A `#` comment inside a `[Captures]` run** — fixed in #16; the one/two-letter-method
  gap it exposed remains (`is_method_line` requires three characters, hurl's grammar does
  not).
- **`key_line_spans`' flow-style undercount** is guarded by convention, not types. Two
  callers guard it independently; a third would have to remember. Cheap hardening: have
  the primitive return a reliability flag.
- **Cross-scenario `${fake:*}` coincidence** — two scenarios can still draw the same
  value. Documented as a known limitation in AUTHORING/CHANGELOG/TECH-SPEC.
- **No corpus tier for engineered robustness fixtures.** `tests/` has zero custom-method
  entries and zero fenced blocks, so that bug class is pinned only by unit tests on
  private functions.
- **`fmt`'s tie-break** (equal CRLF/LF → LF) now applies only to the trailing newline
  of a file that lacked one — per-line endings are preserved (#33). Lone-`\r` files are
  still unhandled: the splitter keys on `\n`, so a classic-Mac file is one long line.
- **`normalize_pack` keeps the skeleton verbatim by construction at each `push`, not
  by the algorithm's shape.** The "hurl blocks only" promise has broken three times
  (#18 line endings, #33 mixed endings, #40 trailing whitespace), each caught by an
  example pinning that one instance. #44 added properties — skeleton-only text round-trips
  byte-for-byte, and formatting is a fixed point — so a fourth over-reach now fails CI
  instead of shipping. The structural version would locate each block's byte span and
  splice the canonicalized body back into the original text, making "bytes outside a
  span are never visited" a property of the shape. **Not worth the rewrite for a small
  textual formatter; revisit if a fourth normalization rule is ever added to that loop.**
- **The stdout latch's single-reader test isolation** is safe under the mandated nextest
  (one process per test) but is a convention, not an enforced invariant.
- **A disk filling mid-run** still truncates the human console report without reaching the
  exit code: `ConsoleReporter` drops write errors. A disk already full at start *is*
  caught. Closing it needs a `note_stdout_failure()` in `Tee::write` when the console is
  stdout — no core change.
- **Absent-secret fallthrough** ("an unset `PROEF_SECRET_<NAME>` still reads the store")
  is load-bearing and pinned only by an integration test, not a unit test.
