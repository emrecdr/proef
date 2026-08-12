# Adoption request — porting an 844-line hurl corpus onto ADR-0018 fragments

**From:** the task-service E2E suite (14 files, 844 lines, **97 entries**) · **2026-08-12**
**Against:** proef `main` (v0.10.0 + `19ab437`), binary built from source

**Intake for `docs/OPEN-FINDINGS.md`**, in the same way #42 ingested the corpus-port report.
Not a second worklist — fold in what earns a place.

## 0. Why this document exists

Implementation of the port was **stopped deliberately** at the point where it would have
required hand-annotating 97 entries with no feedback loop, and writing a local script to
verify the result. Both are workarounds for missing tooling. Rather than build the
workarounds, the friction is reported here.

Everything below was **reproduced live** on a working fragment suite, not inferred.

## 1. The headline gap: fragments have no authoring-time visibility

proef knows fragments at **scan time** — `ScannedFragment` carries `placeholders` and
`supplied_variables` — and reports them at **run time**: `step_finished.fragment` records
`file.hurl#name` (EVENTS.md:49-51), with a documented `jq` recipe for "which fragment files
a failing run exercised" (EVENTS.md:110).

**The gap is exactly the middle: authoring.** Between "proef parsed your corpus" and "proef
ran it", there is no way to ask what it found.

### 1.1 Three questions a porting team cannot ask

| Question | Today | Verified |
|---|---|---|
| Which entries have **no** `# @proef` annotation? | No answer. An unannotated entry is dropped at scan time and never mentioned | ✅ 2-entry file, 1 annotated → `dry-run OK … 0 warning(s)` |
| Which annotated fragments does **no scenario ever run**? | No answer, and it has **two levels** — see §1.2 | ✅ both reproduced |
| How many entries are in the corpus at all? | No answer | ✅ had to count method lines with `awk` to budget the pass |

### 1.2 "Annotated but unused" is two distinct cases, and only one is half-covered

A fragment reaches execution through a chain: **fragment → macro → scenario.** It can die at
either hop, and proef treats the two very differently. Reproduced with one suite carrying
three fragments:

| Fragment | Situation | What proef says |
|---|---|---|
| `api.used` | referenced by a macro a scenario binds | runs — correct |
| `api.deadNoMacro` | annotated, **no macro references it** | **nothing, anywhere.** Not in `macros` (it is not a macro), not in `--dry-run` (`0 warning(s)`). Its existence is unobservable |
| `api.deadViaMacro` | referenced by `orphanMacro`, which **no scenario binds** | `macros` flags the *macro*: `orphanMacro 0× UNUSED — no scenario binds it`. **Nothing connects that to the fragment** |

The second case is the more dangerous of the two, because it *looks* covered: a reader sees
a dead-macro warning and has no reason to suspect a second casualty behind it. Knowing
`api.deadViaMacro` never runs requires manually tracing every `UNUSED` macro's `ref:`
targets by hand.

And the summary line reports `10 macro(s) · 1 unused` — **no fragment total appears
anywhere in proef's output**, so there is no denominator against which either case could be
noticed.

### 1.3 Why silence is the wrong default here specifically

Missing an annotation on 1 of 97 entries produces a **green dry-run and a silently missing
test**. There is no signal at any point in the pipeline: not at scan, not at bind, not at
run. The failure mode is invisible by construction, because a fragment that was never
scanned cannot be reported.

That is a different risk profile from a typo'd `ref:`, which fails loudly and well
(`pack::unknown_ref`, with did-you-mean).

### 1.4 The symmetry argument

proef already does all of this — for macros:

```
  health        1×  the service is healthy
  orphan        0×  UNUSED — no scenario binds it

10 macro(s) · 0 unused
```

Dead-macro detection, call counts, a summary line, and `--output json` carrying
`unused`/`nearDuplicateOf`. **Fragments are the second input language and have none of it.**
The asymmetry is the clearest argument that this is a gap rather than a preference.

### 1.5 The ask — `proef fragments`

A listing command, symmetric with `macros`, that distinguishes **both** death modes from
§1.2 — the count column is "scenarios that actually run this fragment", so it is 0 for a
fragment reached only through an unbound macro:

```
tests/hurl/task_create.hurl        2 entries
  create.task              2×
  create.readback          1×
tests/hurl/task_full_flow.hurl    29 entries
  flow.create.alpha        1×
  flow.orphan              0×   UNUSED — no macro refs it
  flow.stale               0×   UNUSED — only `staleMacro`, which no scenario binds
  (line 143)                     UNANNOTATED — not referenceable
tests/hurl/tags.hurl               6 entries
  …

97 entries · 95 annotated · 2 unannotated · 3 never run
```

Three properties that matter, in order:

1. **A fragment total exists at all.** Today no proef output states how many fragments there are, so neither death mode has a denominator to be noticed against.
2. **Reachability is computed through the chain**, not just one hop. `flow.stale` is the §1.2 case that currently looks covered because the *macro* warning fires; here it is named directly, with the reason.
3. **Unannotated entries are listed by line**, since they have no name to list by. This is the only class proef cannot currently report at all.

Plus:

- `--output json` for scripting, as `macros` has.
- **A `--check` mode** exiting non-zero on unannotated or never-run fragments, so CI owns the invariant instead of a hand-rolled script in every adopting repo.

**Naming note.** "Unused" is doing two jobs above; if the distinction is worth surfacing in
the summary, *unreferenced* (no macro at all) and *unreachable* (macro exists, no scenario
binds it) separate cleanly, and only the second needs the explanatory clause.

This is already raised as **R9-1**; this document is the field evidence for it, and the
argument that `--check` matters as much as the listing.

**Charter note.** This asks proef to *report* what it scanned. It does not generate names,
prose, or macros — PRD §3's amended non-goal is untouched. We are explicitly **not**
requesting auto-annotation.

## 2. Authoring defects that will bite at 97-entry scale

### 2.1 An unread `bind:` key passes silently ✅

Fragment reads `{{token}}`; pack binds `{ token: "x", toekn: "y" }` → `dry-run OK … 0
warning(s)`. `toekn` is never mentioned.

In the commoner shape — the typo *replaces* the correct key — the only error is
`fragment reads 'token', which nothing supplies`, pointing at the **fragment** while the
mistake is a misspelling two lines above in the **pack**.

**AUTHORING overpromises:** *"A `bind:` that nothing can read is refused
(`proef::pack::bind_without_ref`) … at every scope."* Verified by isolating both branches:
`bind_without_ref` fires **only** for the structural case (a `bind:` with no `ref:` step at
all — and #50 improved that case). The **per-key** case is neither refused nor warned.

**Ask.** did-you-mean on unread `bind:` keys against `ScannedFragment::placeholders` —
already in scope, and already driving the LSP's `bind:` completion. **The data exists; only
the check is missing.** Either way, correct the AUTHORING sentence.

**Why it matters at scale:** a port binds one or more variables per pack across 14 packs.
This is the one authoring mistake that produces no signal.

### 2.2 `duplicate_fragment` is tautological for a same-file collision ✅

```
× fragment `api.health` is declared in both `hurl/api.hurl` and `hurl/api.hurl`
  help: … rename one, or qualify the `ref:` as `file.hurl#name`
```

"both X and X" reads as a proef bug, and the offered remedy **cannot work** — qualifying by
filename does not disambiguate two entries in one file. Cross-file is perfect:
*"both `hurl/api.hurl` and `hurl/other.hurl`"*.

**Why this branch, specifically:** annotating a corpus adds **many names to few files** —
ours is 97 names across 14 files, with 29 in a single file. **Same-file is the likely
collision**, so the common case gets the broken message.

**Ask.** Same-file phrasing (*"declared twice in `hurl/api.hurl`"*), point at both spans,
drop the `file.hurl#name` suggestion on that branch.

### 2.3 `unbound_placeholder` names two of three supply routes ✅

The message: *"no `bind:` in scope gives a value, and no earlier step captures it"*.
ADR-0018 defines **three**; the third is the fragment's own `[Options] variable:`.

The omitted route is the one that makes a corpus file **standalone-runnable with fewer
variables passed in** — the property ADR-0018 exists to preserve, and the one the ADR
records as missing from its own list until an audit found it. A porting author learns the
two routes that keep a fragment dependent and never learns the one that makes it
independent.

**Ask.** Add the third clause to the message or its `help:`.

## 3. Environment and discovery

### 3.1 `doctor` does not know fragments exist ✅

`proef doctor` checks embedded hurl, the parser, libcurl, the pack schema, the secret key
and store. Nothing about fragments — with `[run] fragments` set, unset, or naming a missing
directory.

A misconfigured path surfaces later as `pack::unknown_ref`: **an error about a name, when
the cause is a path.**

**Precedent is exact.** R2 added this check for the pack schema, on the argument that a
suite whose editor completion had been silently switched off had nothing telling it so.

**Ask.** An `authoring:` row — `[ok] fragments — 97 loaded from tests/hurl/`, `[warn]` when
the directory is missing, silent when unset. Reuse the loader so `doctor` and the runner
cannot disagree, as #29 shared `init`'s predicate.

### 3.2 Config placement is over-constrained, and it broke our planned layout ✅

Three rules interact:

1. `proef.toml` is discovered by searching **up** from the working directory (CONFIG.md:206).
2. `[run] fragments` resolves relative to the **config file's** directory; `suite`, `setup`, `teardown`, `runs-dir` relative to the **working** directory (F1, already filed).
3. There is **no `--config` flag** (verified against `proef test --help`).

Our plan placed `proef.toml` under `tests/proef/` beside the suite. That is **unworkable**:
a `make` target running from the repo root never finds it. We moved the file to the repo
root, which also collapses F1 — cwd and config directory become the same.

**That is a fine outcome, but it was discovered by failure rather than stated.** CONFIG.md:3
says *"proef.toml lives in the project root (the directory you run proef from)"*, which is
true and is also the whole constraint — it reads as a convention rather than a requirement.

**Ask, cheapest first:** say plainly that the config **must** be at or above the working
directory, since discovery only searches up. A `--config <path>` flag would remove the
constraint entirely and make F1 moot for anyone who hits it.

### 3.3 `ref:` is invisible to the adopter it was built for ✅

A fresh `proef init` writes a scaffold teaching **only** the inline `hurl:` form. `ref`,
`fragment` and `@proef` appear **zero** times in it; `init --help` has no flags.

Docs are fine (README 7 mentions, GETTING-STARTED 6, TROUBLESHOOTING 12, CONFIG 12), and
`WRITING-SCENARIOS` correctly has 0 — it is the P1 page.

But the newcomer with most to gain from `ref:` is the one who **already owns a hurl
corpus** — PRD §4's P3, the reader PRD §1 names as the product's reason to exist. They run
`init`, see a scaffold transcribing hurl into YAML, and conclude proef wants a copy.

**Ask.** Have `init` scaffold both body forms: a small `.hurl` with one `# @proef`
annotation, `[run] fragments` set, and a pack with one macro of each form plus a one-line
comment on when to pick which. **Charter-safe** — a fixed template *demonstrating* the form
generates nothing from anyone's corpus. (`init --from-hurl` deriving macros from real files
would cross PRD §3; this does not.)

## 4. Execution model — costs this port is absorbing

Already filed as **E1/E2**; recorded here with the concrete price.

Our suite has two scenarios that cannot run in the parallel pool: one needs an empty
database (absolute `items[N]` assertions), one installs a workflow definition governing
every task created afterwards. proef offers no way to declare either, so the plan runs
**three sequenced CLI invocations** driven by tag discipline.

Direct consequences the adopting repo now owns:

- **Three run records**, three JUnit files, three HTML reports.
- Exit-code aggregation in shell (`set -e` is insufficient if a middle group must continue).
- Run ids carrying the group name so a post-mortem knows which record to open.
- `explain`/`diff` operate per run, so triage starts by choosing one of three.

**E1 is the fix; E2 is the cost of E1's absence.** Solving E2 alone (a merge command) would
treat the symptom.

## 5. Recorded, not requested

- **N6** — `proef macros` shows no fragment backing for a `ref:` macro. **Not clearly a gap:** `macros` is the P1 command ("what may I write?"), and adding file paths to every row taxes the persona it was fixed for in #24/#29. If ever taken, it belongs in `--output json`, which already carries `pack`. A `proef fragments` command (§1.4) serves this need better.
- **N7** — `pack::unknown_ref` spans the macro name rather than the `ref:` line. The did-you-mean makes it moot.
- **M2** — mechanical hurl-vs-proef equivalence checking. Open upstream, but **no longer on our critical path**: with `ref:` both suites execute the same bytes, so equivalence is structural rather than compared.

## 6. What worked — so it is not traded away

Six of six fragment failure modes we deliberately triggered were caught loudly, each with a
named code, a precise span, and an actionable remedy:

| Mistake | Diagnostic |
|---|---|
| Typo'd fragment name | `pack::unknown_ref` — did-you-mean **plus** a help line defining what a fragment is |
| `[run] fragments` unset | same code, **help adapts to cause**: "no fragment files were loaded — set `[run] fragments`" |
| Unbound `{{placeholder}}` | `lower::unbound_placeholder` — names fragment *and* variable, **spans into the `.hurl` file** |
| `bind:` with no `ref:` | `pack::bind_without_ref` — explains the `use:` subtlety |
| Cross-file duplicate name | `pack::duplicate_fragment` — names both files, remedy applies |
| Annotation with no request | `pack::bad_annotation` — precise span |

The adaptive help line is the standout. And **the diagnostics already pay back most of
ADR-0018's stated "a test spans three files" cost** — someone who has never read the ADR can
follow these messages to a fix.

**Confirmed working end to end** on a real corpus file: `tests/hurl/task_create.hurl`
annotated with two comment lines binds, flows `{{task_id}}` between fragments, preserves
every assert in the artifact, and **still runs under stock hurl** (reaches a connection
error, not a parse error).

## 7. Priority, from the adopting side

| # | Ask | Effort | Why this order |
|---|---|---|---|
| 1 | **`proef fragments` + `--check`** (§1.5) | M | Unblocks the port. Without it a 97-entry annotation pass has no feedback loop, dead fragments are unobservable in both senses (§1.2), and every adopter writes the same script |
| 2 | **Unread `bind:` key** (§2.1) | S | The only *silent* authoring failure; data already in scope |
| 3 | **`doctor` sees fragments** (§3.1) | S | Turns a name-error into a path-error at the right moment; precedent is exact |
| 4 | **Same-file duplicate + third supply route** (§2.2, §2.3) | XS | Two message edits; §2.2's broken branch is the *likely* collision when annotating |
| 5 | **`init` shows `ref:`** (§3.3) | S | Makes the feature findable by the persona it was built for |
| 6 | **Config discovery doc / `--config`** (§3.2) | XS / M | Doc line is nearly free; the flag removes the constraint |
| 7 | **E1** (§4) | M | Not blocking us — we absorb E2's cost — but it is the first wall any shared-state suite hits |

**Items 1 and 2 are what stopped implementation.** Everything else we can work around
without writing tooling proef should own.
