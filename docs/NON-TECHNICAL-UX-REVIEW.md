# proef — non-technical adoption review (P1 calibration, 2026-08-10)

**Question asked:** is proef easy enough for a first-time, *non-technical* user to adopt?
**Version reviewed:** 0.8.0 (Homebrew, `/opt/homebrew/bin/proef`) · **Platform:** macOS arm64

## 0. What this is — and what it is not

A **persona-calibration** review. It measures the shipped binary against the job PRD §4
assigns to **P1** — *"Test author (QA, PM, support engineer; not necessarily a programmer)"*
— whose stated job is narrow: **write prose sentences that bind to a vocabulary somebody
else maintains.**

It is deliberately *not* a capability review ([IMPROVEMENT-PLAN](IMPROVEMENT-PLAN.md)), a
defect list ([OPEN-FINDINGS](OPEN-FINDINGS.md)), or a repeat of the 0.5.3 first-run review
([FIRST-RUN-UX-REVIEW](FIRST-RUN-UX-REVIEW.md)) — whose backlog is **closed**, verified in §2.

**Stated limitation, so the confidence here is not over-read:** no non-technical person was
observed using the tool. This is a charter-versus-behavior audit — every claim is a
reproduced command transcript or a source citation, but the *impact* estimates are
reasoned from PRD §4's persona definition, not measured from user sessions. Findings are
facts; severity ordering is judgement.

## 1. Headline

**proef's promise is P1; its surface is P2.**

PRD §1 opens with the product's reason to exist — *"let non-programmers author real tests"* —
and PRD §4 makes P1 the first persona. But every path a first-time user can walk in the
shipped binary assumes the reader can author a YAML macro pack containing raw hurl. The
non-programmer is the product's founding promise and, today, the only persona with no
supported route through it.

Three consequences, all reproduced below:

1. A freshly scaffolded project **cannot pass its own first run**, and the path the
   tool's own `next:` chain walks you down reports `system error` and exits **3** (§3).
2. The **list of sentences a P1 may write is not printable** from the CLI at all (§4) —
   and the command that comes closest **refuses to run** in the exact situation that
   sends you looking for it (§5).
3. **No document** describes P1's workflow; the six docs labelled *"test authors"* all
   teach pack authoring (§7).

None of this is a defect in the sense OPEN-FINDINGS uses — the tool does what it says.
It is a **calibration** gap: excellent ergonomics, aimed one persona to the right.

## 2. Method — reproducible, and what it confirms already shipped

```console
$ proef --version                 # proef 0.8.0
$ mkdir rep && cd rep
$ proef init                      # exit 0
$ proef test --dry-run            # exit 0
$ PROEF_BASE_URL=http://127.0.0.1:9099 proef test    # unused port = clean machine
```

`PROEF_BASE_URL` is pointed at a closed port deliberately — see the blind spot in §3.

**Prior review verified closed.** Every item in FIRST-RUN-UX-REVIEW.md now ships:

| Item | Status | Evidence |
|---|---|---|
| F1 `proef init` | shipped | scaffolds 5 files, exit 0 |
| F2 `missing_config_var` did-you-mean | shipped | emits ``did you mean `base`?`` |
| F3 success-path coaching | shipped | dry-run ends `next: proef test` |
| F4a `params:` in README | shipped | `grep -c "params:" README.md` → 2 |
| §7 "What proef deliberately isn't" | shipped | present in README |

That review was written by *"an experienced engineer meeting proef for the first time."*
Its persona is P2/P3. **P1 has never been reviewed** — which is why its closed backlog
does not answer the question asked here.

## 3. N1 — The scaffold's first run reports `system error`, exit 3 (effort: M · highest impact)

**Evidence.** A clean machine, immediately after `proef init`:

```console
$ proef test
summary: 0 passed · 1 failed · 0 skipped
system error: suite/case.feature:2 A known record is found — HTTP connection
  (case--a-known-record-is-found.hurl:6: (7) Failed to connect to 127.0.0.1 port 9099
   after 0 ms: Couldn't connect to server) (artifact case--a-known-record-is-found.hurl:6)
    curl: curl --connect-timeout 10 --max-time 30 'http://127.0.0.1:9099/health'
  reproduce: hurl --test .proef-runs/019feb63-…/artifacts/case--…hurl
$ echo $?
3
```

The scaffold's default target is `crates/proef-cli/src/init.rs:29`:

```toml
base = "${env:PROEF_BASE_URL:-http://127.0.0.1:8787}"
```

**8787 is proef's own dev fixture port** — `cargo run -p xtask -- fixture`
(`CLAUDE.md:62`; ADR-0011 amendment, `CLAUDE.md:175`). A user who installed via Homebrew
has no `xtask`, no repo, and nothing on 8787. **Every** new user's first `proef test`
therefore fails — the scaffold cannot pass by construction. But it fails one of *two*
ways, and only one of them is N1:

| First `proef test`… | Result |
|---|---|
| before configuring anything (nothing on 8787) | **exit 3**, `system error` — this finding |
| after pointing `[url] base` at any reachable host | **exit 1**, assert failure on the placeholder routes; **no** `system error` |

Both reproduced 2026-08-10. So N1 is not "the error every user hits" — it is the error
every user hits *who runs `proef test` before editing config*. That is where the tool's
own coaching sends them: `init` prints `next: proef test --dry-run`, dry-run prints
`next: proef test`, and neither repeats the configuration hint, which appeared once —
parenthetically — in `init`'s output. The breadcrumb trail leads directly to the
unconfigured run.

**The maintainer blind spot that hides this.** On the machine this review was written on,
an `xtask` fixture *was* listening on 8787 (`lsof -iTCP:8787` → `xtask 39567`), so the
first run partially **passed**:

```
    ✓ suite/case.feature:3 — the service is healthy (6ms)
    ✗ suite/case.feature:4 — the operator searches for "Acme" (0ms)   # 404
```

A maintainer's machine is close to the only environment where the scaffold's first run
does anything but fail. This is why the finding is easy to miss from the inside, and it
is the reason §2 forces an unused port.

**Why this lands hardest on P1.** Four compounding problems, in the order a
non-programmer meets them:

1. **The words say the tool broke.** `system error` is the vocabulary of a malfunction.
   The actual meaning is *"you haven't told me where your API is yet."* A QA or PM
   reads the former and reasonably concludes the install is bad.
2. **Nothing marks the failure as expected.** `proef init` *does* warn —
   `next: proef test --dry-run (then point ${url:base} at your API — the scaffold's
   routes are placeholders)` — but that is two commands earlier, and the failure never
   refers back to it. The scaffold knows it shipped placeholders; the runner does not.
3. **The remedy on offer is maintainer-only.** `TROUBLESHOOTING.md:48–53` covers this
   exactly — *"Exit 3 with connection errors — the target is unreachable"* — then offers
   `cargo run -p xtask -- fixture` as the local target. That command does not exist for
   a binary user. The documented fix for the guaranteed first failure is unavailable to
   the people guaranteed to hit it.
4. **The remaining output is P3 vocabulary** — `artifact`, a raw `curl` line, and a
   `hurl --test` reproduce path into `.proef-runs/<uuid>/`. All three are excellent for
   a backend engineer and inert for a PM.

**Note the asymmetry with §5 of the prior review**, which observed that proef's *errors*
coach and its *successes* do not. F3 fixed the success path. What remains is narrower and
worse: the one error every user is *certain* to hit is the one error that does not coach.

**Non-goal check: passes.** PRD §3 forecloses further engines, mocking, contract testing,
load testing, dashboard/server, OTel, dynamic plugins, hurl import, and static binaries.
Message wording and a scaffold default are none of these.

## 4. N2 — The sentence vocabulary cannot be printed (effort: S · highest value per line changed)

P1's entire job is writing sentences that bind. **There is no command that lists them.**

**Evidence.** `proef macros` prints *identifiers*, not prose:

```console
$ proef macros
builtin:core.yaml
  expectStatus                 0×  (builtin, unused here)
  …
suite/packs/api.yaml
  health                       1×
  search                       1×

10 macro(s) · 0 unused
```

A P1 needs `the service is healthy`. They are shown `health`. The JSON mode does not
help — it reduces the pattern to a **boolean**:

```console
$ proef macros --output json
{"name":"health","pack":"suite/packs/api.yaml","pattern":true,"calls":1,…}
```

**Source.** This is a render-boundary loss, not missing data. In
`crates/proef-cli/src/commands.rs`:

| Line | Code | Effect |
|---|---|---|
| 314 | `near_duplicate_macros(rows.iter().filter_map(\|m\| m.pattern.as_deref()…))` | the pattern **string** is read and linted |
| 326 | `"pattern": m.pattern.is_some(),` | JSON keeps only *whether* one exists |
| 362 | `outln!("  {:<28} {n}×{marker}{near}", m.name)` | text prints the **name** only |

`m.pattern` is `Option<String>`, already in scope and already used. Both renderers
discard it on the way out. Restoring it is one line each — with one contract note the
size hides: the JSON field **changes type**, `true` → the pattern string (`null` when
absent). `AUTHORING.md:46–52` sells `--output json` as a CI-gate surface but names only
`unused`/`nearDuplicateOf`, and **no test pins the JSON shape** — `corpus.rs` asserts text
mode only. So the change breaks nothing today, and nothing would catch it either. Make it
deliberately, not as a side effect of a one-line edit.

**Where the vocabulary is reachable today** — and why neither route serves P1:

- **Reading `match:` lines in the YAML packs.** This requires opening the pack, i.e.
  exactly the artifact ADR-0004's design and PRD §4's persona split exist to keep P1
  out of.
- **LSP completion** (`EDITORS.md:11` — *"step completions offering the suite's macro
  patterns"*). This is genuinely the right feature, but it requires wiring `proef lsp`
  into Neovim/Helix/Emacs. It therefore serves the persona **least** likely to need it
  and is unavailable to a P1 in a terminal or a plain editor.

The capability exists; it is gated behind an editor integration, and the CLI — the
surface everyone has — withholds it.

## 5. N3 — `macros` and `flows` refuse to run when any step is unbound (effort: S–M)

**Evidence.** With one unbound sentence added to the scaffold's feature file:

```console
$ proef macros ; echo $?     # → bind::unbound_step, exit 2
$ proef flows  ; echo $?     # → bind::unbound_step, exit 2
```

Causally verified: deleting the offending line returns both to exit 0 with normal output.

**Source.** Both commands front-load the same whole-suite load and bail on any error —
`flows` at `commands.rs:229`, `macros` at `commands.rs:290`, both
`let front = match load_front(…) { Err(code) => return code }`. Binding failures are
fatal to *listing*, not just to running.

**Why this is the sharpest P1 finding.** It is circular. The moment a test author needs
the vocabulary is the moment a sentence failed to bind — and that is precisely the state
in which the vocabulary listing refuses to answer. The user is told what is wrong and
simultaneously denied the information that would fix it.

`bind::unbound_step` is, correctly, the best diagnostic proef has: span, caret,
did-you-mean, copy-pasteable stub. But its `help:` answers **P2's** question (*"how do I
add a macro?"*). P1's question is *"what am I allowed to say?"* — and today nothing
answers it.

## 6. N4 — Best-in-class diagnostics, addressed to the wrong reader (effort: M, wording)

**Evidence.** The remedy `bind::unbound_step` offers:

```yaml
  help: add a macro to a pack (or fix the step text), e.g.:

        macros:
          newMacro:
            match: the first hit is record {arg1}
            steps:
              - hurl: |
                  GET ${url:base}/PATH
                  HTTP 200
```

For P2 this is exemplary — it is the standard the rest of the tool is measured against.
For a PM it is four unfamiliar concepts (*macro*, *pack*, *hurl*, `${url:…}`) in six
lines, and the actionable half — *"or fix the step text"* — is the parenthetical.

**Corroborating measurement.** Term frequency in `GETTING-STARTED.md`, the document
titled *"your first suite in ten minutes"* and labelled audience *"test authors"*:

| Term | Occurrences |
|---|---|
| `pack` | 18 |
| `hurl` | 14 |
| `YAML` | 9 |
| `artifact` | 7 |
| `macro` | 6 |

**This is not an argument for dumbing down the diagnostic.** The stub is load-bearing for
P2 and must stay. The finding is that the *ordering* is backwards for the reader who
cannot act on it: P1's available action (fix the sentence, or look up a valid one) is
subordinate to P2's (author a macro).

## 7. N5 — No document describes P1's workflow (effort: S, docs only)

**Evidence.** `docs/README.md` assigns audience **"test authors"** to six documents:
GETTING-STARTED, AUTHORING, EDITORS, TROUBLESHOOTING, CONFIG, DIAGNOSTICS.

- **GETTING-STARTED** §3 is *"Bind the prose"* — writing a YAML pack with raw hurl.
- **AUTHORING** is subtitled *"packs and features from the author's seat"* and opens on
  pack structure.
- **EDITORS** is LSP wiring. **CONFIG** is every `proef.toml` key. **DIAGNOSTICS** is a
  greppable code index.

Every one of them assumes the reader owns the pack. PRD §4's split — P1 writes prose
against an existing vocabulary, P2 owns the packs — **is stated in the PRD and reflected
in no user-facing document.** There is no page for *"someone else wrote the pack; here is
your loop."*

That loop is small and already fully supported by the binary: read the available
sentences → write a scenario → `proef test --dry-run` → fix what did not bind →
`proef test`. It has never been written down.

## 8. Minor — `init` announces 4 files and reports 5

```console
  created ./proef.toml
  created ./suite/case.feature
  created ./suite/packs/api.yaml
  created ./.gitignore
  ok ./suite/packs/api.yaml (modeline added)

created 5 file(s), skipped 0
```

`find` confirms five files; `suite/packs/proef-pack.schema.json` is written but never
announced. Cosmetic, but the first output a new user reads does not reconcile, and the
unannounced file is the schema that powers editor completion — worth naming rather than
hiding.

## 9. Recommendations, with reasoning

Packaged rather than itemised, because the items pair: each package closes one user
moment end to end.

### Package A — Make the first failure honest *(recommended, with B)*

**Change.** Detect the untouched scaffold sentinel and answer as a **user error (exit 2)**
with a named remedy, instead of a **system error (exit 3)**. Keep the curl/reproduce block
for those who want it, below the remedy.

**Reasoning — this follows the taxonomy rather than bending it.** ADR-0009 maps
User/TestFailure/System → 2/1/3. "The operator has not configured a target yet" is a
**user** error by that taxonomy's own definition; the current exit 3 is arguably the
misfiling, not the proposed 2. The change is a re-classification of one narrow, detectable
case — *the scaffold default, unmodified, unreachable* — and leaves genuine
connection failures on exit 3 untouched.

**Where the change lands — it is not a wording fix.** Four sites across three crates,
verified against the tree 2026-08-10:

| Concern | Site | Crate |
|---|---|---|
| Classification — the `_ => Infra` arm that decides exit 3 | `session.rs:860` | `proef-engine-hurl` |
| The carrier — `Fault::System(String)`, an **opaque string with no kind** | `runner.rs:152–157` (`public-api.txt:1142`) | `proef-core` |
| Exit derivation for the pool | `exit_code_excluding`, `runner.rs:107–112` | `proef-core` |
| The words `system error:` | `exec.rs:345–346` | `proef-cli` |

Only the last is CLI-edge. The label and the exit code both read the *same* `Fault`
variant, in two different crates, so nothing changes the wording without also changing
the code. Two routes, and picking one is a real decision:

- **(a) Rewrite the variant at the CLI edge** — flip the outcome's fault from `System` to
  `User` before the render loop and before `exit_code_excluding`. No public-API change,
  but the CLI must **string-match the engine's opaque message** to recognise a connection
  failure, informally crossing the ADR-0002 seam.
- **(b) Give the engine a structured signal**, so the CLI matches a kind rather than
  prose. Honest, and it touches `proef-core`'s public surface → `public-api.txt`
  regeneration.

Either way this is **M**, not S — the ratings in §3 and §11 reflect that. The tempting
reading is that a message written in `proef-cli` is a change owned by `proef-cli`; the
verdict it prints is not.

**Risk, stated.** It introduces a sentinel-value special case, a real cost in a codebase
that prizes one-canonical-way. The guard has to be a **conjunction** — one literal fails
in both directions:

- Key on the *raw* `[url] base` string alone, and a user who pointed `PROEF_BASE_URL` at
  their own unreachable API is still told they have not configured a target. §2's own
  transcript is that case.
- Key on the *resolved* value alone, and it mis-fires on a maintainer legitimately
  targeting 8787 with the fixture down.

The rule that holds is all three at once: **the raw `[url] base` is byte-identical to
`init.rs:29`'s default · `PROEF_BASE_URL` is unset** (read through `envvar::read`, the
crate's one reader) **· the connection was refused.** The raw string is available
unexpanded — `${env:…:-…}` resolves in core (`resolve.rs:265`), so the CLI still holds the
literal `init` wrote. If the operator set the override they *did* configure a target, and
a connection failure there is a genuine system fault.

**Also in scope, no special case required:** change the scaffold default away from 8787.
It is a maintainer port that reads as meaningful and is not. Either an obvious placeholder
(`https://api.example.com`) that makes "you must edit this" self-evident, or keep 8787 and
have `init` say what it is. The former is preferred — a value that *looks* configured is
worse than one that looks blank. **This half needs no re-classification at all** — it is
S, it is independent of everything above, and it can ship alone.

**Non-goal check: passes.** PRD §3 forecloses none of this. But "no new surface" holds
only for route (a); route (b) adds a public type to `proef-core` — a cost to weigh, not a
non-goal breach.

### Package B — Print the prose *(recommended, with A)*

**Change.** Two parts:

1. Render `m.pattern` in both `macros` renderers (`commands.rs:326` JSON,
   `commands.rs:362` text). The string is already loaded and already linted at
   `commands.rs:314`. The JSON half is a **field type change** (bool → string|null),
   unpinned by any test — see §4.
2. Let `macros`/`flows` **degrade** instead of refusing when binding fails: list the
   vocabulary that loaded, and report the unbound steps as a note rather than a fatal
   exit. **The call counts must go quiet in that mode** — see the hazard below; it is a
   requirement, not a polish item.

**Reasoning.** Part 1 is the highest value-per-line change available — it converts the
existing `macros` command into the thing P1 actually needs, using data already in scope,
with no new concept, flag, or file. Part 2 removes the circularity in §5, and is cheaper
than it looks: the pipeline is entirely CLI-side (`proef-cli/src/front.rs`), and
`pack::load` completes *before* the per-feature loop — the whole `PackSet` is already in
hand at the moment binding fails. No `proef-core` change, no `public-api.txt`.

**Hazard — ship this with part 2 or part 2 creates the bug it exists to prevent.**
`macros`' counts are accumulated from `front.features[]`, and `front::run` does `continue`
past any feature that fails to parse or bind — so a degraded listing sees fewer callers
than the suite actually has. `is_dead_macro` (`commands.rs:276`) is
`has_pattern && calls == 0 && !builtin`. A macro whose only caller is the file that did
not bind would therefore print **`UNUSED — no scenario binds it`** — confidently, wrongly,
and in precisely the state part 2 exists to serve. That is the bug class v0.6.0–v0.8.0
spent three releases closing: proef reporting success while producing a wrong answer.

**Requirement.** In degraded mode, suppress the `n×` column, the `UNUSED` marker, and the
`unused` JSON field (emit `null`, not `false`), and say why in the note. `flows` needs the
mirror: a scenario list that silently omits an unparsed feature must carry
`N feature(s) not listed`. Listing a vocabulary tolerates partial knowledge; *verdicts
about that vocabulary* do not.

**Design question that needs your call (this is why B is S–M, not S).** Part 2 changes an
exit code contract: `macros`/`flows` currently exit 2 on unbound steps, and exit codes are
a pinned contract tested by assert_cmd. Three options — (a) degrade and exit 0, treating
listing as a query that binding cannot invalidate; (b) degrade, print the list, still
exit 2, so scripts see no change; (c) leave the default and add a flag. **(b) is the
conservative recommendation** — it delivers the information without touching a tested
contract.

**Non-goal check: passes.** No import, no generation, no second way to do anything —
`macros` already exists and already reads this data.

### Package C — Write the page P1 needs *(independent, docs only)*

**Change.** One short document — *writing scenarios* — covering only: what a sentence is,
how to see the ones available (post-B: `proef macros`), the dry-run loop, and how to read
the two errors P1 will actually hit (`unbound_step`, `missing_config_var`). Zero pack
authoring; link to AUTHORING for that. Add the row to `docs/README.md` with audience
**"test authors"**, and re-label the existing six honestly (most are P2/P3).

**Reasoning.** §7's gap is not that the docs are bad — they are unusually good — but that
the persona split PRD §4 defines has no expression outside the PRD, which is a maintainer
document. The prior review's §7 made exactly this argument about non-goals: *"the boundary
is invisible where newcomers look."* The same is true of the persona boundary.

**Reasoning for keeping it independent.** C changes no behavior and cannot regress
anything, so it need not wait on A or B — but it *reads* better after B, because it can
point at a `proef macros` that prints sentences.

### Package D — A scaffold whose first run passes *(not recommended now)*

**Change.** Ship something that makes `init` → `test` green with no API — e.g. promoting
`proef-fixture` into the shipped binary.

**Reasoning for holding it.** It is the deepest fix for §3, and it is the only option here
that touches architecture: `proef-fixture` is **dev-only by ADR-0011**, and shipping it
enlarges the binary, the dependency surface, and the security posture of a *test runner*
with a listening server. That requires a new ADR. More importantly, **A makes it
unnecessary** — the problem in §3 is not that the first run fails, it is that the failure
is unexplained and mis-typed. A failure that says *"the scaffold still points at a
placeholder — set `[url] base` in proef.toml"* is a perfectly good first run.

Reconsider D only if, after A, first-run drop-off is still observed.

## 10. Checked and deliberately not proposed

Recorded so they are not re-raised, in the spirit of FIRST-RUN-UX-REVIEW §7.

- **Anything importing or round-tripping hand-written hurl.** PRD §3 permanent non-goal;
  already withdrawn as W1 in the prior review.
- **Anything OpenAPI-shaped.** ADR-0016: oracle/drift mode permanently rejected, narrow
  scaffolder deferred.
- **A GUI, web UI, or "no-terminal" mode for P1.** PRD §3 forecloses dashboard/server
  mode. P1's non-technical status is an argument about *vocabulary and error text*, not an
  argument for a second interface — and treating it as the latter would be the most
  expensive possible misreading of this review.
- **Simplifying `bind::unbound_step`'s YAML stub.** It is load-bearing for P2 and is the
  single best diagnostic in the tool (§6). Re-order the help, never remove the stub.

## 11. Suggested sequence

| # | Package | Effort | Why this order |
|---|---|---|---|
| 1 | **A** — first failure honest | M *(scaffold default alone: S)* | The failure the tool's own `next:` chain walks users into (§3); nothing else matters if they stop here |
| 2 | **B** — print the prose | S–M | Turns an existing command into P1's core tool; unlocks C's content |
| 3 | **C** — the P1 page | S | Docs-only, cannot regress; reads best once B ships |
| — | **D** — shippable fixture | L | Hold; needs an ADR, and A likely removes the need |

A and B together are the recommendation: both target the same user moment (*first failure
→ "what now?"*), neither adds a flag or a second way to do anything, and neither requires
an ADR.

Neither touches `proef-core`'s **sans-IO** contract — but that is a narrower guarantee than
it reads, and it is not the same claim as *lives in `proef-cli`*. **A is not a wording
change:** its verdict is set in `proef-engine-hurl`, its exit code is derived in
`proef-core`, and only the words are CLI-edge; route (b) additionally touches
`public-api.txt`. **B genuinely is CLI-only** — two render lines, one degrade policy, and
the count-suppression the hazard above makes non-optional.

If only one thing ships, ship the scaffold default (inside A): it is S, it needs no
re-classification, and it removes the *cause* rather than improving the *report*.

## 12. What not to change

Verified good, and load-bearing for the persona above P1 — none of this should be traded
away to serve P1:

- **`bind::unbound_step`.** Span, caret, did-you-mean, and a copy-pasteable stub. Still
  best-in-class; §6 asks only that P1's action be promoted above P2's, not that anything
  be removed.
- **`missing_config_var`'s did-you-mean.** F2 shipped and works
  (``did you mean `base`?``). Its span still points at the feature sentence rather than
  the pack site — correctly deferred with a written reason in FIRST-RUN-UX-REVIEW's
  validation notes. Leave deferred.
- **The dry-run gate.** `--dry-run` validating everything with zero network is exactly
  the right primitive for a nervous first user, and it now coaches (`next: proef test`).
- **Convention-based discovery.** Nothing to register is a genuine P1 asset.
- **The artifact escape hatch.** Under-sold, per the prior review's §9, and still true.
- **`proef init` itself.** It works, it is fast, it never overwrites, and it installs the
  schema. §3 is a criticism of the *default value* it writes, not of the command.
