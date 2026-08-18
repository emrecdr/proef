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
| R9-1 | no `proef fragments` listing — neither way a fragment dies had a denominator | (branch) |
| §2.1 | a `bind:` key nothing reads passed silently — the one authoring mistake with no signal | (branch) |
| §2.2 | `duplicate_fragment` said "in both `x` and `x`" and offered a remedy that cannot work | (branch) |
| §2.3 | `unbound_placeholder` named two of ADR-0018's three supply routes | (branch) |
| §3.1 | `doctor` did not know fragments exist — a path error surfaced as a name error | (branch) |
| §3.2 | config discovery searches only up, undocumented; no way to name the file | (branch) |
| §3.3 | `init` scaffolded only `hurl: |`, so `ref:` was invisible to the persona built for it | (branch) |
| — | ADR-0007 value caps never crossed to fragments: `retry: -1` validated clean | (branch) |

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

**Shipped.** A second migration report (`ADOPTION-REQUEST.md`, 97 entries)
supplied the field evidence this entry was waiting for and ranked it first of
seven. `proef fragments` now names both death modes apart, lists unannotated
entries by line, and gates CI with `--check`; `--require-annotated` is opt-in
because an unannotated entry is inert *by design* (ADR-0018), so "not done yet"
is a porting team's reading of that signal and not every adopter's.

### R9-2 — fuzz coverage does not reach the fragment surfaces *(shipped)*

`fuzz_pack_load` runs with an empty corpus, so `ref:`/`bind:` clash logic never
executes under fuzzing; the annotation scanner's entry-boundary arithmetic —
proef's own code, not hurl's — and `bake_entry_options`' textual injection are
unfuzzed entirely. Split the fuzz input into pack and corpus halves, and consider
a `fuzz_fragment_scan` target (nightly, accepting the native-libs cost).

**Shipped, and the prescription was half wrong — measurably.** Splitting the
input into pack and corpus halves was tried first and *did not work*: a
byte-oriented target never resolved a single `ref:` in **1.45 million runs**,
because reaching the rules means discovering valid YAML and a matching corpus
name simultaneously. Verified by probe (panic on a resolving `ref:`, run the
fuzzer, see whether it fires) rather than assumed from coverage numbers — which
is the same mistake this finding is about, one level up.

What shipped instead is `fuzz_fragment_binding`, **structure-aware**: it builds a
well-formed pack and corpus from the input and spends the budget on the name
space, so every run reaches the rules. The probe fires in seconds.
`fuzz_pack_load` stays byte-oriented and unchanged — parser totality is a real
job and the split would only have diluted it.

The `fuzz_fragment_scan` half was declined for a concrete reason, not on cost
alone: cargo dependencies are **package-level**, so adding `proef-engine-hurl` to
the fuzz crate compiles hurl for all five targets and drags native libraries into
a job that has none. Hurl's scanner is instead **property-tested in
`proef-engine-hurl`**, where those libraries already are — pinning that every
reported line lies inside the file, that entries are accounted for exactly once
in order, and that no fragment's text runs into the entry after it. The last
assertion was added after mutation testing: the first draft passed with the
boundary deliberately broken.

Still open from this entry: **`bake_entry_options`' textual injection is
unfuzzed.** It is lower-time, not load-time, so it sits behind lowering rather
than `pack::load` and needs its own target.

### R9-3 — no resource bounds on the corpus read *(shipped)*

No per-file or file-count cap: a multi-GB `.hurl` is read whole on every command
that loads packs. Pairs with the read-resilience work in #48, which made the read
*survivable* but not *bounded*.

**Shipped, and worse than filed by one word:** not "a multi-GB file" — a 279 MB
file cost **601 MB of resident memory** on `proef flows`, a command that never
looks at a fragment, over a file carrying no `# @proef` annotation at all. The
doubling is `read_to_string` into a `String` and then `Arc::from(&str)`, which
copies.

Bounded now at 8 MiB per file and 64 MiB per corpus, measured from the directory
entry so an oversized file is never allocated (601 MB → **15 MB** on the same
input). Reported through the per-file diagnostic channel `unreadable_file`
already established — skipped, never fatal — and applied in `proef lsp` too,
where the corpus is *held between requests* rather than for the length of one
command. The laziness promise is intact: a corpus nothing `ref:`s still reports
nothing and exits 0, pinned by a test.

The `Arc<str>` copy itself was left alone. Removing it means changing
`PackSource`'s type across every reader, which is a wider change than a bound
and buys a constant factor on an input that is now capped anyway.

### R9-4 — a bind that shadows a capture is silent

hurl's `variable:` assigns into one shared set, so a pack- or macro-scope `bind:`
re-assigning a name an earlier entry captured overrides it for every later entry,
with no diagnostic. A warning shaped like `option_declared_twice` fits — the
difference is that this one is only decidable where the capture set is known, at
lower time.

### R9-5 — `{{x}}` inside a bind value is unvalidated at lower time

It fails at run time instead of at `--dry-run`: loud, but late, and the late half
is what `--dry-run` exists to prevent.

### R9-6 — provenance is cwd-dependent *(shipped)*

Run from a subdirectory and `step_finished.fragment`, explain's `via`, JUnit and
the diagnostics carry an absolute machine path; the record-portability claim holds
only from the project root. Relativize against the config root rather than cwd —
the same boundary `[run] fragments` already resolves against.

**Shipped** as part of R12-1, which found the same defect reaching further than
this entry describes — the safe case it names, running from the project root, had
stopped being safe. The prescription here was the right one and is what landed:
one anchor, the config directory, for every input kind.

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

**The double print is broader than filed** (verified 2026-08-14 while adding the
corpus bound, which inherits it). It is not specific to `macros`: `proef
fragments` does it too, and to *any* corpus diagnostic — `unreadable_fragment_file`
and the new `oversized_fragment_file` alike. The mechanism is that
`commands::fragments` renders `corpus.diagnostics()` itself and then loads the
suite, whose failure path renders the same diagnostics again. Both land on
stderr, so the count line reads `1 error(s)` under two rendered copies. Left
here rather than folded into the bound: it is a rendering decision about which
of the two sites owns corpus diagnostics, not a property of any one diagnostic.

## Open — round-10 residue (ingested 2026-08-12)

Found by a cleanup review over the fragments branch, after its own gates were
green. All three are consequences of what that branch added; none is a defect in
what shipped before it. Recorded rather than fixed in place because each is a
behaviour change, and the branch was already carrying two correctness fixes.

### R10-1 — `--config` is honoured by the runner and ignored by the editor *(shipped)*

`--config <path>` bypasses the upward search so a `proef.toml` beside the suite
becomes usable. `proef lsp` never sees it (it re-discovers via
`ProjectConfig::load_from`), and `--watch` watches the config found by its own
fresh upward search, not the one the run was given.

So in **exactly the layout the flag exists for**, `proef test --config …` runs
green while the editor gets no `[run] fragments` and reports every `ref:` as
unknown — diagnostics disagreeing with the runner, which is the drift that makes
an editor untrustworthy.

**Shipped.** `ProjectConfig` now keeps the *file* it was read from and derives
`root` from it, rather than storing the directory and leaving every consumer that
needed the file to search again. `--watch` watches the config the run resolved
through; `proef lsp` takes the flag and lets it outrank even the client-announced
workspace root, since a named file is not a guess to be improved on. The free
`config::config_path()` — the fresh upward search both bugs went through — is
gone, which is what stops the class recurring. `proef lsp` still starts when a
named config is missing (an editor offering less beats one that will not boot),
where the runner exits 2; the asymmetry is deliberate and documented.

### R10-2 — `proef fragments` judges reachability over a smaller universe than the runner *(shipped)*

`[run] setup` / `[run] teardown` are not loaded, so a fragment used **only** by a
phase feature counts as never run and fails `--check` — a false CI failure in the
workflow `--check` was asked for, unless the phase feature happens to sit inside
the suite directory. `exec::execute` already threads one corpus through both
phase validations and both phase runs; the listing needs the same universe.

### R10-3 — three predicates answer "is this a fragment file?", and they disagree *(shipped)*

`front::fragment_extensions` (exact match, and its doc claims to be "the one
place that answers this"), `pack::scan_fragments` (exact), and the LSP's own
`is_fragment` (case-insensitive). `api.HURL` therefore invalidates the editor's
corpus but is never scanned by core or discovered by the CLI.

The shared home is `proef_core::engine`, beside `StepKindSpec` — it is pure logic
over the registry, so it is sans-IO-legal, and `proef-lsp` cannot reach
`proef-cli`'s copy. Worth pairing with the deeper question the LSP predicate
raises: membership in `discover_fragments()` is the real test, and an extension
match also claims emitted artifacts that happen to end in `.hurl`.

### R11-1 — `proef.toml` resolved its paths against two different roots *(shipped)*

`[run] fragments` resolved against the config file's directory and `suite`,
`setup`, `teardown` and `runs-dir` resolved against the working directory, so the
same relative spelling meant two directories depending on which key it sat under.
`.proef-state.json` and `.proef-secrets.json` were cwd-anchored too and appeared
in no inventory, making two shells in one project two Worlds and two secret
stores. One rule now: written paths resolve against the config, typed paths
against the working directory.

### R11-2 — `--watch` retriggered on a config it then ignored *(shipped)*

The loop watched `proef.toml` and reran on an edit while the rerun used the
startup snapshot, so changing `[url] base` produced a rerun that called the old
host. Fixed by re-reading per rerun — and by moving the startup config out of
scope, which makes the stale value unreachable from the rerun closure and the
invariant a compile error rather than a habit. Which *directories* are watched is
still fixed at startup, so `[run] fragments` and `[run] suite` need a restart to
be watched. `runs-dir` was in that list until R11-8 showed it did not belong
there: it is not a watched root but an *excluded* one, and freezing it was the
bug rather than the limitation.

### R11-3 — `--config` was honoured, swallowed, or ignored depending on the command *(shipped)*

`doctor` printed the error for a missing named file and then reported on
defaults, exit 0; `fmt`, `init`, `schema` and `secret` accepted a nonexistent
path silently. Three documents called the flag global to every subcommand. A
named-but-missing file is exit 2 everywhere now; `doctor` stays lenient about
discovery, which is a different claim.

### R11-4 / R11-5 — `[run] exclusive-tags` did not validate itself *(shipped)*

`--dry-run` never parsed the expression, and a well-formed expression matching
nothing was silent — both defeat the reason the setting is a config expression
rather than a reserved tag name.

### R11-6 — exclusivity is invisible in the run record

`Event::ScenarioStarted` carries no field saying a scenario ran exclusively, so a
post-mortem cannot tell a deliberate drain from a stall: the record shows
parallelism dropping to one and nothing explaining why. An additive field is
permitted by ADR-0008, and the reporters would need to decide whether to surface
it. Filed rather than built — it is a design question about what the record
should say, not a defect, and the run behaves correctly either way.

### R11-7 — the corpus-read rule is shared, its *discovery* is not

`FragmentCorpus::unreadable_file` now gives both readers one diagnostic, but the
CLI walks the fragment root with `std::fs` while the LSP reads through its
overlay provider. That difference is real — the editor must see unsaved buffers —
so the readers stay separate. What is worth watching is that "which files are in
the corpus" is still answered twice, and only the *meaning* of a failed read was
unified here.

### R11-8 — a `runs-dir` edited mid-`--watch` fed the loop its own output *(shipped)*

R11-2 made each rerun re-read the config, so records went to the *new* runs dir
while the watcher's exclusion still named the one frozen at startup. Every
rerun's `artifacts/*.hurl`, now under an unexcluded directory, requeued the next
run: 39 runs in 12 seconds, firing real traffic, from one edit. The third outing
for this class, so the fix removes the second answer rather than resynchronising
it — each rerun registers where it is about to write, *before* it writes, and the
exclusion is derived from the same config the run is. Deliberately not a
uuid-shaped exclusion: `--run-id` names a run directory that is not uuid-shaped.

### R11-9 — a relative `--config` was never the file `--watch` matched *(shipped)*

The watcher compared the config by exact path while `notify` reports events under
the spelling the OS resolved them to, so `--config proef.toml` matched nothing and
config edits produced no rerun — silently, because feature edits kept firing and
the loop looked alive. Two questions had been conflated: where a path *points*
(answered once, lexically, when the flag is stored) and whether two paths *are the
same file* (answered by comparing canonical forms, since absolute is not enough —
macOS's `/var` → `/private/var` aliasing and symlinks both survive it). The same
relative path had been costing `proef lsp --config` go-to-definition across the
whole corpus, because `documents::name_to_url` refuses a relative name.

### R11-10 — `doctor` reported on defaults over a `proef.toml` that would not parse *(shipped)*

R11-3's discovery arm became a silent `unwrap_or_default`, dropping the parse
error the previous code printed: a malformed config left `doctor` reporting on
invented defaults and printing "all checks passed", exit 0. A `project:` row now,
so it reaches `worst` and the exit code CI reads. Leniency still means *absent* —
`doctor` must run outside a project — not *broken*.

## Ingested — competitive research v2 (2026-08-16), validated claim-by-claim

An external research pass (prototyped against the built 0.12.0 binary) plus its
round-14 companion review. Each actionable claim was re-reproduced here before
anything was written down. Disposition:

### S1 — an encoded reflection of a secret defeated redaction *(shipped)*

**The one defect in the set, confirmed by live reproduction**: a server
reflecting the bearer token base64-encoded put `dG9r…` (trivially decodable)
into an assert-failure detail; the raw needle never fired; the encoded
credential reached the console and `events.jsonl`. The raw-form invariant was
intact — this violated its *intent*. Shipped as derived needles inside
`Redactions::new` (see the changelog and the ADR-0005 amendment); property- and
mutation-tested, pinned end-to-end against a fixture introspection route.

Not covered, on purpose: hashed/split/re-encrypted reflections (not needle-
matchable), double encodings (an unbounded tower; echo endpoints produce one
level). The research doc's companion ideas — a redaction-verifying scan over a
finished run record, GitHub `::add-mask::` for captured secret-typed values,
RF-style secret-typed macro *arguments* — are enhancements, not part of the
defect, and await triage.

### Corrections to the research set, so they are not re-litigated

- **S4 (Trusted Publishing plan) rests on a false premise**: it plans a
  *first* publish with a classic token, but all four crates have been live on
  crates.io since 0.5.1 (0.12.0 current). Trusted Publishing can be configured
  directly against the existing crates; the token sequence is unnecessary.
- **S2's exposure check is right and already satisfied**: `Cargo.lock` carries
  `curl-sys 0.4.90+curl-8.21.0`, past the June-2026 CVE batch. The *detection
  blind spot* (RUSTSEC carries no advisories for `*-sys`-bundled C libraries)
  is real; the proposed libcurl-version print in release artifacts awaits
  triage with the rest.
- **R3-16/R3-17 (browser and Android engines) are foreclosed**, not deferred:
  proef is API-testing-with-hurl only — a standing decision, not a gap the
  research reopens. The M6 line in CLAUDE.md is architectural readiness, with
  nothing scheduled. The seam-hygiene half of R3-15 stands on its own merits
  and awaits triage like the rest of the registry.
- The round-14 review audited `214a39d` (a pre-amend commit never pushed;
  what merged is `c3ac752`, differing by one deliberately-removed proptest
  seed), counted 464 tests where 462 exist, and credited #63 with the LSP
  corpus-holding change that shipped earlier — recorded here because review
  counts have now drifted by +2 for three consecutive rounds.

### The R3 registry — triaged 2026-08-17

Triaged as a set against the PRD, the ADRs, and current industry practice,
with each seam re-validated against the tree first. The v1 research document
was confirmed absent (only v2 exists on disk), so items defined only there are
one-line summaries with no spec — that fact drives several verdicts below.

**Built:**

- **R3-1 `--max-fail N`** *(shipped with this triage)*. The convention is
  universal — Playwright `--max-failures`, pytest `--maxfail`, nextest
  `--max-fail` — with one shared semantics: stop after N failures, un-run
  tests report as not-run rather than passed. proef's seams made it a CLI-only
  change: a sink wrapper counts suite-scenario failures (the `phase` field
  keeps setup/teardown out of the count) and cancels the run token, which is
  the tested Ctrl-C drain path — in-flight batches finish, the rest record as
  skipped, teardown still runs on its own token, and the record is a complete
  *cancelled* run. That last part is free correctness: `diff
  --fail-on-regression` already refuses to certify a cancelled run, which is
  exactly right for a deliberately-partial one.
- **R3-4 `diff` takes a record path** — shipped earlier (#65), with the
  research's `--baseline` flag spelling declined as a second name for the
  same positional.

**Build next (validated, in order):**

- **R3-2 a flakiness verdict** — *(shipped as `proef flaky`)*. The 2026
  pipeline is detect → quarantine → resolve, and proef already owned the
  middle step (`@quarantine` runs-but-does-not-gate); `flaky` is the missing
  detect, a fold over the records `runs-dir` already retains, so the history
  window is `[run] keep-runs` and no new state exists. Transition-counting
  separates flaky from broken (a mutation test proved the test suite could
  not initially tell that apart from a naive fail-rate — the F,F,P,P case
  now pins it), per-step attempt counts surface the pass-only-on-retry
  latent class, and a cancellation-skipped row is not evidence. **No
  `--check` gate, deliberately** — its sibling `fragments` has one, but a
  flakiness verdict is advisory by nature and `@quarantine` owns the gating
  decision; the asymmetry is a choice, not an omission, and the thresholds
  become contract (and move to `proef.toml`) only if a gating mode ever
  exists.
- **R3-3 sharding, hash-mode only.** Discovery order is deterministic
  (verified byte-identical), and the research measured the nextest lesson
  concretely: naive index-slicing re-buckets 2 of 3 scenarios when one is
  added; hash-by-`(file, scenario)` re-buckets none. Ship only the hash mode;
  pin the filter→shard order when specifying.
- **R3-6 JUnit attributes** — *after* a fresh spec written from what GitLab
  and Jenkins actually consume. The original definition lives in the absent
  v1 document; building from a two-word summary is guessing.

**Deferred, with the trigger named:**

- **R3-5 CTRF output** — on the first concrete consumer request. The format
  has momentum but proef already emits JUnit, TAP, JSONL, a GH summary, SARIF
  and HTML; a seventh format needs a consumer, not a trend.
- **R3-18 generated pack documentation** — when pack-vocabulary discovery becomes a
  reported adoption pain; the LSP currently serves that need interactively.
- **R3-15 pre-M6 seam refactors** — when a second engine is actually
  scheduled (M6 has nothing scheduled; the snapshot corpus already provides
  the golden artifact-diff prerequisite).
- **R3-7 `--affected-by`, R3-10 fake variants** — defined only in the absent
  v1 document; need the source or a fresh spec before any verdict.
- **R3-9 seeded shuffle** — already tracked as IMPROVEMENT-PLAN #14; no
  second listing here.

**Declined — do not re-raise** (moved to the standing section's rules):

- **OTel trace export (R3-11) and Cucumber Messages (R3-12).** ADR-0008: the
  JSONL event stream *is* the record, no second record format. Both are
  re-encodings of the record for ecosystems that can convert from JSONL
  outside proef; building them in creates permanent format-tracking
  obligations against moving upstream schemas.
- **Browser/Android engines (R3-16/R3-17)** — foreclosed by the standing
  hurl-only decision, not deferred.
- **S4's first-publish token sequence** — false premise; the crates have been
  live since 0.5.1. The worthwhile residue (crates.io Trusted Publishing for
  the *existing* crates, then the token-delete) is an owner-side dashboard
  action, recommended to the user rather than something the repo can do.

## Open — adoption report on 0.12.0 (ingested 2026-08-14)

From a suite that ported to `ref:` at scale — 15 hurl files, 112 fragments, 21
scenarios — and ran 0.12.0 as an installed release. Three items, each reproduced
here against the tree before being written down. Two shipped in the same change;
the third is recorded because the report's *diagnosis* was wrong even though its
*observation* was right, and that distinction is the finding.

### R12-1 — provenance named the machine that produced the record *(shipped)*

`[run] suite` resolves against the config directory (R11-1), so a path-less
`proef test` handed the front end an **absolute** path and every emitter printed
it: the `.hurl` `# source:` header, `.map.json`'s `feature.file`, every
`step_finished` event, the console, and pack diagnostics. Two checkouts of one
suite stopped producing equal artifacts, which is exactly the property ADR-0010
exists to guarantee.

**Worse than R9-6 filed it.** R9-6 says the portability claim "holds only from
the project root"; this reproduces *from* the project root with the config in it.
R11-1 was the right fix — one resolution rule — but resolution produces absolute
paths, and nothing was named at the other end.

**Shipped, and R9-6 with it.** `front::SourceNaming` is the one naming boundary:
resolve against the project, then name against the project again. A relative path
is left exactly as it arrived (machine-independent already, and the caller's own
spelling, which their terminal can open); an absolute one is spelled relative to
the config directory when it lies inside it. This also replaced the fragment
corpus's cwd-relative strip, which was a second anchor for the same question —
the drift R9-6 predicted. The four ways to name one suite (derived, typed, typed
absolute, from a subdirectory) now emit one artifact byte-for-byte, pinned by
`crates/proef-cli/tests/provenance.rs`.

Two limits, deliberate: a corpus genuinely outside the project keeps its absolute
name, because no project-relative one exists; and `DiskSourceProvider`
(`proef lsp`) still yields absolute names, because it keys document identity on
them.

### R12-2 — the run-record ceiling was a constant no project could reach *(shipped)*

Retention was `const RUN_RETENTION = 200` with only `runs-dir` configurable, and
artifacts are byte-identical across runs of an unchanged suite — so a suite
re-run on every save accumulated identical bytes for a day before anything
signalled a ceiling existed. `[run] keep-runs` makes the policy expressible; `0`
keeps none but the run in flight.

**The report's inference that artifacts should therefore not be stored is wrong,
and it said so itself:** an old record's artifacts are what *that run executed*,
and once the corpus changes `proef artifacts` no longer reproduces them. Bound
the cost, do not drop the evidence.

**Not closed by this, and not reported:** rotation only ever deletes directories
named by a *generated* run id, so `--run-id <name>` records sit outside the
budget entirely. A CI minting a fresh id per build accumulates without bound.
Guessing at user-named directories is the worse failure — `runs-dir` may be `.`
— so this stays, documented in CONFIG.md rather than fixed.

### R12-3 — a `[run] setup` test failure is invisible to JUnit

Reproduced: a setup feature whose *assertion* fails exits **2** with
`summary: 0 passed · 0 failed · 0 skipped`, and `--output junit` writes an empty
report, because the abort precedes the reporter. A CI reading JUnit sees nothing
at all.

**The exit code is not the defect.** ADR-0014 decided it explicitly — a setup
failure maps to a user (2) or system (3) fault, never a test failure, "the same
distinction Playwright draws between a clear setup error and a cryptic test
failure". Changing it needs a superseding ADR, not a bug fix.

**Three of the report's supporting claims do not survive checking**, recorded so
they are not re-litigated:

- *"teardown already has a distinct code; setup collapses both into one"* —
  false. A teardown **assertion** failure exits 3, not 1: both phases map a test
  failure onto a non-test code (`phase_failed(…, UserError)` / `…, SystemError`).
  Neither distinguishes, by design.
- *"appears in nothing `explain`/`diff` consume"* — false for `explain`, which
  prints `failed (setup — excluded from the totals above)` with the assertion
  detail and the artifact reference; the events are in the record with
  `phase: setup`.
- *"previously raised, still open"* — no entry in this file matches it.

So the open item is narrow: **the phase reporters run only for the pool.** Worth
fixing at the reporter, not the exit code.

---

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

**Shipped** as `[run] exclusive-tags`, a tag *expression* rather than a list —
the same language `--tags` takes, so group membership is answered exactly as
selection is. Two corrections to this entry, both from checking before building:

1. The filing describes one axis; the mature shape has two. `cargo-nextest`
   separates a group concurrency limit (`max-threads`, which bounds members
   against each other and leaves the rest of the pool running) from per-test
   weight (`threads-required`, which is what buys global exclusivity — they
   redefined it in 2024 precisely so limits "are never exceeded", enabling
   mutual exclusion against *all* tests). Only the second is what was missing
   here, so only that shipped; a group table can be added later without
   breaking this key.
2. Of the two motivating scenarios, only the first is a serialization problem.
   "Installs a workflow definition governing everything created afterwards" is
   **ordering**, which `[run] setup` already provides — a feature run once
   before the pool exists. Recorded so an ordering primitive is not built on
   the assumption that it was needed.

### E2 — N invocations produce N run records, with no merge *(report B2; consequence of E1)*

**Verified.** Each run writes its own `.proef-runs/<run-id>/` (`TECH-SPEC.md:299`).

E1's workaround therefore yields N records, N JUnit files, N HTML reports, and
pass/fail aggregation pushed onto the caller's shell, while `explain`/`diff`
operate per-run so a post-mortem reader must know which to open. **Recorded as a
consequence, not an independent item** — solve E1 and this largely evaporates;
solving it alone (a `proef merge`) treats the symptom.

**Largely closed by E1 shipping:** a suite whose isolation needs are expressed
as `exclusive-tags` runs in one invocation, so it produces one record, one JUnit
file, one report and one exit code. Kept open rather than closed outright
because a suite may still split invocations for reasons E1 does not address
(different environments, different `--tags` in separate CI jobs), and nothing
merges those.

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
