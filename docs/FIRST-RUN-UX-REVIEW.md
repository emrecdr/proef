# proef — first-run UX review (external, 2026-08-06)

> **Evidence, not a worklist.** Findings below are preserved as written — the
> transcripts and citations are the record. Whatever remains open from this
> review is tracked in **[OPEN-FINDINGS.md](OPEN-FINDINGS.md)**, which is the
> single list of work still to do.


**Reviewer:** an experienced engineer meeting proef for the first time, evaluating it for
adoption by a real service (a FastAPI/Postgres task service with an existing 14-file,
844-line hurl E2E suite). **Version reviewed:** 0.5.3 (Homebrew).

## 0. What this is

A **first-use experience** review, not a capability review. `IMPROVEMENT-PLAN.md` is
competitor- and capability-driven (Karate ledger, §12 re-survey); it tracks *what proef can
do*. This tracks *what happens in the first thirty minutes*, which is a different failure
surface and — verified by grep — is **not currently tracked**. Searching `IMPROVEMENT-PLAN.md`
for onboarding / first-run / first-use / new-user / adoption returns exactly one hit,
line 258 (*"needs `--force`/first-run fallback"*), which concerns skipping a scenario that
would fail — unrelated to onboarding. No item in the plan addresses the first-use path.

Every finding below was reproduced against the shipped binary. Every proposal was checked
against **PRD §3 non-goals** and the existing ADR corpus before being written down; the ones
that failed that check are in §7, withdrawn, so they are not re-raised later.

## 1. Headline

**Diagnostics are the product's standout strength — and the reason the gaps are worth
fixing.** `bind::unbound_step` is best-in-class: source span, did-you-mean, *and* a
copy-pasteable macro stub. It sets a standard the rest of the first-run path does not yet
meet. The three highest-value fixes are all "make the rest behave like that one."

The single biggest structural gap: **there is no way to create a working suite except by
hand.** 13 commands, none scaffolds.

## 2. Method — reproducible

```console
$ brew install emrecdr/proef/proef && proef doctor      # all green, hurl 8.0.1
$ mkdir -p spike/suite/packs                            # hand-authored from docs
$ proef test --dry-run                                  # passed first try
$ proef artifacts -o out && hurl --test out/*.hurl      # stock hurl executes it
```

Then four deliberate failure experiments (§3–§6 cite them as E1–E4). The suite content was a
verbatim lift of the adopting project's existing `.hurl` bodies into pack `steps[].hurl` —
which **worked unmodified**, confirming P3's "pastes between corpus and packs" workflow
(PRD §4) is real and cheap.

## 3. F1 — No `proef init` (effort: S · highest adoption impact)

**Evidence.** `proef --help` lists 13 commands: test, flows, macros, artifacts, schema,
doctor, secret, explain, diff, report, fmt, lsp, help. None scaffolds a project. The README's
quick start is literally `mkdir -p suite/packs` followed by a prose instruction to *"write
suite/case.feature … or copy tests/features/ wholesale"*.

**Cost observed.** Reaching a first green `--dry-run` required hand-authoring three files
(`proef.toml`, `*.feature`, `packs/*.yaml`) and consulting two documents (README →
GETTING-STARTED) to learn the pack shape. That is the blank-page problem, and it is the step
where evaluation is abandoned.

**Proposal.** `proef init [dir]` writes a minimal working suite — `proef.toml` with a
commented `[url] base`, one `.feature`, one matching pack — then runs `--dry-run` on it and
prints the next command. Time-to-first-green drops from ~10 minutes to ~30 seconds.

**Non-goal check: passes.** PRD §3 forecloses further engines, mocking, contract testing,
load testing, dashboard/server, OTel, dynamic plugins, hurl import, and static binaries.
A blank-project scaffold is none of these. It is **not** ADR-0016's deferred
`generate --openapi` either: no spec is read, nothing is derived, output is a fixed template.
ADR-0016's decisive objection — *"OpenAPI describes an API in endpoint terms; proef's value is
business prose"* — does not apply to a hello-world template that ships prose by construction.

**One-canonical-way check: passes.** It adds no second way to do anything; it writes the same
files an author would write by hand.

## 4. F2 — `resolve::missing_config_var` is below the standard its siblings set (effort: S)

**Evidence (E4).** With `[url] base` defined and a pack referencing `${url:bse}`:

```
× in macro `m`: url variable `bse` is not set — define `[url]` `bse` in proef.toml …
 ╭─[suite/t.feature:3:5]          ← the span points at the FEATURE SENTENCE
 3 │     When a thing happens
```

Three distinct gaps, all verifiable against `DIAGNOSTICS.md`:

1. **No did-you-mean**, though `base` is one edit away and the `[url]` table is already
   loaded. Two sibling codes in the *same* family are documented as suggesting:
   `resolve::unknown_variable` — *"(suggests the closest)"* — and `resolve::fake_unknown`.
   PRD §4 names *"load-time validation with did-you-mean hints"* as a **P2 persona need**.
2. **The span points at the feature sentence, not the pack line** where `${url:bse}` actually
   appears — so the reader must go hunting. This is a capability regression *within the same
   run*: E3 (`pack::invalid_hurl`) mapped a payload-internal error back to `p.yaml:8:11`
   precisely, so pack-relative spans demonstrably work.
3. **No seeded corpus case.** `DIAGNOSTICS.md` marks the Corpus column empty for this code,
   and its own coverage note records 23 of 59 codes seeded.

**Proposal.** Add closest-match suggestion from the merged `[url]`/`[vars]` key set; retarget
the span to the pack site; seed `tests/errors/resolve__missing_config_var/`. Same treatment
for `resolve::missing_env` and `resolve::unknown_namespace`, which share the shape.

## 5. F3 — Success paths do not coach; only failure paths do (effort: S)

**Evidence.** A passing dry-run terminates with:

```
dry-run OK: 1 feature(s), 1 scenario(s), 2 step(s), 1 batch(es), 1 artifact(s) …, 0 warning(s)
```

No next command. Note the asymmetry: every *error* in this review ended with a `help:` line or
a named remedy — even the empty-directory error names all three fixes (pass a path, set
`[run] suite`, create `tests/`). The success path is the one that stops talking, at exactly
the moment a new user is deciding whether to continue.

**Proposal.** On dry-run success print the next command (`proef test <path>`). If no `[url]`
key is configured, say so as a warning — it is the *guaranteed* next failure and is known at
that point.

## 6. F4 — Two discoverability gaps in the README (effort: S, docs only)

**a. No parameterized macro appears in the README.** Verified: `grep -c "params:" README.md`
→ **0**. The only `match:` shown is a static sentence. The `searchRecords` sample carries
`params:`/`defaults:` but not the `match:` line binding them, so the placeholder syntax
(`match: … {term}`) exists only in GETTING-STARTED and AUTHORING.

*Behavioral evidence this matters:* writing the spike, I **deliberately avoided** parameterized
macros and wrote a dumber test rather than guess the syntax. An experienced reader silently
routing around the feature that makes packs worth writing is worse than an error — errors get
reported; avoidance does not.

**b. `proef schema --add-to` is undiscoverable.** Schema-backed completion is the single
largest authoring aid for YAML, and PRD §4 names *"schema autocomplete"* as a P2 need — but it
is one row in a CLI table and requires the user to pass pack paths. It should be something
`init` does automatically (F1) and `doctor` reports as missing.

## 7. Withdrawn after checking the charter — do not re-raise

**W1 — "Add `proef import` to convert existing `.hurl` files into packs."** This was my
second-highest recommendation before validation. It is a **permanent non-goal**: PRD §3 —
*"importing/round-tripping **hand-written** hurl files into Gherkin (artifacts flow outward
only)"* — restated in IMPROVEMENT-PLAN §3. Withdrawn.

**W2 — Anything OpenAPI-generator-shaped.** Settled by ADR-0016: the oracle/drift mode is
permanently rejected; the narrow scaffolder is deferred. Not re-raised.

### The signal in W1 (this is the actionable part)

An informed reader who had read the README and GETTING-STARTED proposed a documented permanent
non-goal as a top-two recommendation. The charter is not wrong — the *boundary is invisible
where newcomers look*. PRD and IMPROVEMENT-PLAN are maintainer documents; neither is on the
path a new user walks.

Related observation supporting the same point: PRD §4's **P3 persona** ("Backend engineer,
owns the hurl corpus", who *"pastes between corpus and packs"*) is the exact answer to
"I already have hurl" — and it is invisible outside the PRD. The adopting project I evaluated
has 844 lines of hurl and 28 near-identical `POST /v1/tasks` blocks; my spike proved bodies
paste in **verbatim**. That is a strong adoption story that currently nobody outside the PRD
can find.

**Proposal (effort: S, docs only).** A short README section — *"What proef deliberately isn't"*
— naming the load-bearing non-goals (no hurl import: artifacts flow outward only; no mocking
or contract testing; no second engine), plus one line telling existing-hurl users the
supported path is pasting bodies into pack steps. This converts a repeated outside proposal
into a stated boundary, and turns the most common objection into an onboarding asset.

## 8. Suggested sequence

| # | Item | Effort | Why this order |
|---|---|---|---|
| 1 | F1 `proef init` | S | Removes the blank-page problem; unlocks F4b (schema install) and F3 (next-step nudge) as things `init` can do |
| 2 | F4a + §7 README section | S | Docs-only; fixes the two things that made an informed reader both avoid a core feature and propose a non-goal |
| 3 | F2 `missing_config_var` | S | Brings the resolve family up to the standard `bind::unbound_step` already sets |
| 4 | F3 success-path coaching | S | Smallest, but only fully lands once `init` exists to point at |

Everything here is S. None requires an ADR, none touches `proef-core`'s sans-IO contract
(F1/F3 are CLI-edge; F2 is diagnostic text plus a span already computed elsewhere), and none
adds a parallel mechanism.

## 9. What not to change

The `bind::unbound_step` diagnostic, `doctor`, convention-based discovery with nothing to
register, `--dry-run` validating with no network, and the artifact escape hatch — verified:
stock `hurl 8.0.1` parses and executes a `proef artifacts` output, reaching an assertion
failure rather than a parse error. For an adopting team weighing a 0.5.x dependency, that
escape hatch is the single most reassuring property proef has, and it is under-sold.

---

## Status (2026-08-10)

F1, F3 and F4a shipped in 0.6.0; F2's did-you-mean half shipped with them. **Two halves
remain open**, both tracked in [OPEN-FINDINGS.md](OPEN-FINDINGS.md): F2's span retarget
(**R1** — deferred with the written reason in the validation notes below) and F4b's
`doctor` check for a missing pack schema (**R2**). Two further proposals were declined
with reasons, also below.

## Validation notes (2026-08-06, maintainer)

Every finding above was re-checked against the tree at `03b442f`.

**Reproduced exactly:** the 13-command inventory with no scaffold; the single
unrelated `IMPROVEMENT-PLAN.md` hit at line 258; `params:` absent from the
README; `DIAGNOSTICS.md`'s "23 of 59" coverage note and the empty corpus column
for `missing_config_var`; the PRD §3/§4 and ADR-0016 quotations; E4's missing
suggestion and feature-sentence span; E3's pack-relative span; and the silent
dry-run success line.

**Two corrections.**

1. **§4's sibling proposal is narrowed.** Extending did-you-mean to
   `resolve::missing_env` is declined: its candidate set is the injected
   environment snapshot, so suggesting from it risks surfacing unrelated
   environment variable names in diagnostics, against the secret-masking
   posture. `resolve::unknown_namespace` already enumerates all seven valid
   namespaces in its message. Sibling codes share a *shape*, not a *candidate
   set*. `missing_config_var` is implemented.
2. **§5's `[url]` warning is dropped.** The claim that a missing `[url]` key is
   "the guaranteed next failure" does not hold: a suite with absolute URLs and
   no `proef.toml` at all dry-runs green with 0 warnings and executes fine, so
   the warning would fire on a valid suite. When a suite *does* reference an
   unconfigured `${url:key}`, dry-run already fails with
   `missing_config_var` — making the warning redundant in the case it targets.
   The next-command half of §5 is implemented.

**F2 is split.** The did-you-mean half is small, as §8 says. The span retarget
is not: §8 justifies it as "a span already computed elsewhere", but
`ResolveError` carries no position and `resolve()` is documented "Pure and
total". E3's pack span comes from hurl's own parser reporting a line/column
that feeds `locate::payload_line_span(…, rel_line)`; nothing computes a
`rel_line` for a resolve failure. Supplying one means threading an offset out
of a deliberately position-free pure function and carrying pack identity to the
diagnostic site. It is deferred to its own spec.
