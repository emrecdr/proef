# `proef flaky` — the 2026 predicate upgrade (design)

**Status:** proposed, awaiting review. **Date:** 2026-09-07. Companion to the
0.18 survey (§6). Design-first per the survey's own note that this is a
*feature*, not a defect fix.

## What already exists (do not rebuild)

`crates/proef-cli/src/flaky.rs` (400 lines) is further along than the survey
implied. Validated against the tree:

- **Broken ≠ flaky is already a distinct verdict.** `Verdict::Broken` =
  failed every observed run; `Verdict::Flaky` = the pass/fail line flapped
  (`transitions() >= 2`). This is the single highest-value idea in the 2026
  field (Trunk makes it a status priority, Datadog a guard), and proef has it.
- **Transition-counting, not fail-rate** — `F,F,P,P` reads as a fix that
  stuck (one transition), not a flake. Fail-rate cannot tell those apart; a
  mutation test pins that it does.
- **`Latent`** (green only after retries), **`Disabled`** (quarantined +
  failing every run — switched off, not flaky), **`Recovered`** (quarantined
  but green — the tag can come off), and **`New`** (< 2 runs) already exist.
- History is a per-scenario fold over the retained records, keyed by
  `(context, file, scenario)` where `context` is the `--by` value (env).

So three of the survey's five predicates are present in spirit. **Four real
gaps remain**, and one collides with an ADR.

## The four gaps, and how to close them

### 1. A configurable minimum-sample floor

**Today:** the floor is a hardcoded `observed() >= 2`. Below it, `New`.

**The gap:** the 2026 norm (BuildPulse default 10; Trunk's worked example
refuses a 37.5%-at-n=8 verdict) is that a verdict on thin data is worse than
no verdict. Two runs is far too few to call something flaky.

**Design:** a `--min-samples N` flag (and `[flaky] min-samples`), default
**10**. Below it a scenario is `insufficient-data` (rename `New`'s key from
`"new"` to `"insufficient-data"` to match the vocabulary — *breaking* for a
consumer keyed on `"new"`, so note it). The transition and broken predicates
only classify at or above the floor.

### 2. Activation/resolution hysteresis + a recovery window

**Today:** every verdict is recomputed fresh from the whole window each run —
no memory, so a scenario can flip `flaky`↔`healthy` between adjacent runs.
Transition-counting is somewhat flap-resistant but not hysteresis.

**The gap:** Trunk uses a **band** — activate at (say) 30% failure, resolve
only below 15%, so a test hovering at 20% stays flagged rather than
oscillating — plus a **recovery window** (default 7 days) of clean behaviour
before healing. Datadog auto-`Fixed` only after 30 clean days.

**Design:** because proef holds no persistent per-scenario verdict state (it
recomputes from records each run — and that statelessness is a virtue), model
hysteresis over the *window* rather than across invocations:

- **Activation/resolution on the transition signal itself.** A scenario is
  `flaky` once it shows `>= activate-transitions` flaps (default 2, today's
  value); it *stays* `flaky` until it shows a clean tail of
  `recovery-runs` consecutive passes (default derived from the window). This
  is hysteresis expressed as "flapped recently, and has not yet earned its
  way out", computable from the ordered `runs` slice with no new state.
- `--recovery-runs N` (default 5) / `[flaky] recovery-runs`: the length of
  the trailing all-pass run that resolves a `flaky`/`latent` scenario to
  `healthy`. Fewer than that trailing passes → the flag holds.

This keeps the pure-fold design (no verdict store) while adding the
anti-flap the survey asks for.

### 3. An environment-outage guard

**Today:** every observed run counts, even one where the fixture or staging
environment was down and *every* scenario failed.

**The gap:** Trunk's Infrastructure Failure Protection discards any upload
where > ~80% of tests failed — an outage, not evidence about any one
scenario. This is the failure mode most likely to bite an HTTP-only runner
(a staging outage), and only Trunk publishes it.

**Design:** `flaky` already reads every record in full, so it can compute
each run's **overall scenario failure rate**. A run whose suite failure rate
exceeds `--outage-rate` (default **0.8**) / `[flaky] outage-rate` is an
*outage* and its observations are **excluded from every scenario's history**
(logged: "N run(s) excluded as environment outages"). A scenario that only
ever "failed" during outages is then correctly `healthy`/`insufficient-data`,
not `broken`.

Note the interaction with min-samples: excluded runs do not count toward the
floor either, so an outage cannot push a scenario over the sample floor with
non-evidence.

### 4. The equivalence class — and the ADR-0020 collision

**Today:** keyed by `(context, file, scenario)`. `context` is the `--env`
value. A `proef.toml` edit, a pack change, or a fragment edit silently
changes what a scenario *is* while its `(file, scenario)` key stays the same
— so the window mixes runs of materially different inputs.

**The survey's suggestion:** BuildPulse keys on the git tree SHA; Develocity
on a declared-inputs fingerprint. Both make "same code" precise.

**The collision:** **ADR-0020 §1 forbids proef from harvesting environment
facts** — "no git, no hostname, no CI env sniffing". A git-tree-SHA
equivalence class would require proef to *read git state*, which the ADR
prohibits by name. This is exactly why the design goes first.

**The resolution — two independent halves, both ADR-0020-clean:**

- **The input fingerprint is proef's own computation, not harvested.** proef
  already holds, at front-end time, everything that defines a scenario's
  inputs: the normalized feature source, the resolved pack set, the fragment
  corpus digest, and the injected `config_vars` snapshot. A stable hash of
  those is a *proef-computed fact about proef's own inputs* — the same
  category as the artifact slug or the shard hash, not an environment fact.
  Stamp it into `run_started` as an additive `input_fingerprint` field
  (event schema stays 1, additive per ADR-0008). `flaky` then keys on
  `(fingerprint, file, scenario)` by default, so a pack or config edit
  correctly ends the comparison window. This is the Develocity model, done
  without harvesting.

- **Git grouping stays handed-over, never harvested.** A user who wants
  commit-based grouping hands the SHA over as metadata —
  `--meta commit=$(git rev-parse HEAD)`, ADR-0020 §1's own worked example —
  and `proef flaky --by commit` groups on it (the `--by` machinery already
  reads metadata). proef never runs git itself. The survey's "git tree SHA"
  becomes "a fingerprint proef can compute, plus a commit the user can hand
  over" — strictly more ADR-compliant, and it also serves non-git VCS.

**Open question for review:** should the fingerprint be the *default* key
(changing today's behaviour — a scenario's history now resets when its inputs
change, which is more correct but means a config edit visibly shortens
windows), or opt-in behind `--by fingerprint`? Recommendation: **default**,
because the current behaviour silently mixes incomparable runs, which is the
bug; but it is a behaviour change worth calling out, so flagged here.

## Verdict vocabulary

Converge on the survey's `healthy | flaky | broken | insufficient-data`,
keeping proef's richer set as sub-states:

| key (machine) | keep? | note |
|---|---|---|
| `healthy` | keep | |
| `flaky` | keep | now hysteresis-gated (gap 2) |
| `broken` | keep | already distinct (the field's best idea) |
| `insufficient-data` | **rename** from `new` | matches the vocabulary — *breaking* for a `--format json` consumer keyed on `"new"` |
| `latent` | keep | proef-specific: pass-only-on-retry |
| `disabled` / `recovered` | keep | proef-specific: quarantine lifecycle |

## Config surface

New `[flaky]` table (all optional, env-overridable like every table):

```toml
[flaky]
min-samples = 10     # floor below which a scenario is insufficient-data
recovery-runs = 5    # trailing all-pass run that resolves a flag to healthy
outage-rate = 0.8    # a run failing more than this share is an outage, excluded
```

Plus the CLI flags `--min-samples` / `--recovery-runs` / `--outage-rate`
overriding the table (the established flag-over-config precedence).

**No `--check` gate, still** — this stays advisory by design (`@quarantine`
owns the gating decision); the survey's R3-2 verdict is untouched. Thresholds
become contract only if a gating mode ever exists.

## Additive vs breaking

- **Additive (wire):** `run_started.input_fingerprint` (event schema stays 1).
- **Additive (config):** the `[flaky]` table.
- **Breaking (library/JSON):** the `new` → `insufficient-data` verdict key
  rename; the default key change to include the fingerprint (a window that a
  config edit now resets). Both are MINOR pre-1.0, recorded as Breaking.

## Testing

- Unit, over hand-built `History` slices: the floor (n=9 → insufficient,
  n=10 → classified); hysteresis (a flap that then earns `recovery-runs`
  clean passes resolves; one short of it holds); the outage guard (a run at
  the outage rate is excluded and does not count toward the floor).
- The fingerprint is deterministic over identical inputs and changes when the
  pack/config/corpus changes — pinned like the shard hash.
- A property: excluding outage runs never *worsens* a verdict (an outage
  cannot make a healthy scenario broken).
- Integration: two runs with an edited `proef.toml` between them land in
  different fingerprint windows.

## What this is not

Not an ML model, not a second record format, not a gating mode. Five
predicates that are pure folds over the JSONL history proef already keeps —
the survey's framing, honoured. No OTel, no external service (the lcov the
coverage recipe emits is the only external-tool touchpoint, and that is Wave
E, not this).

## Decision requested

1. **Fingerprint as default key** (recommended) or opt-in `--by fingerprint`?
2. **`new` → `insufficient-data` rename** accepted as a MINOR breaking change?
3. Does stamping `input_fingerprint` into `run_started` want its **own ADR**
   (it is an additive event field with an ADR-0020 rationale), or is the
   ADR-0020 amendment note in this doc + the CHANGELOG sufficient?

On approval I implement it as one PR, stacked appropriately, gated green.
