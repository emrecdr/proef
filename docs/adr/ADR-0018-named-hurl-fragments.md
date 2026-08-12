# ADR-0018 — Named hurl fragments: a second macro body form

**Status:** Accepted · **Date:** 2026-08-11

## Context

ADR-0004 made a macro step's HTTP payload a raw hurl block embedded in the pack YAML.
Field evidence says that was right: a real 844-line, 14-file hurl corpus was ported onto
proef through the paste path with **100% coverage** — all 844 lines used only
`[Asserts]` (75) and `[Captures]` (15), with seven ordinary predicates, every one
passing through untouched (OPEN-FINDINGS §"Positive evidence"). Nothing here weakens
that.

Two things the embedded form cannot do, both structural rather than incidental:

1. **The block is not valid hurl.** It carries `${url:…}` / `${secret:…}`, so
   `parse_hurl_file` only ever sees it after a probe substitution that tries `{{probe}}`,
   then `1`, and accepts whichever parses (`pack/validate.rs`, `probe_lower`). Editors,
   hurl's own tooling, and `hurl` itself see a file they cannot read.
2. **A block has no name, so it cannot be shared.** `use:` composes *macros*, not
   payloads; two macros wanting the same request duplicate its text. For a corpus
   somebody else owns, the only route in is transcription — and a transcript drifts
   from its original the day after it is made.

The adoption question the worklist is now on (M1–M3) is not "can a suite be ported" —
it can — but whether a team can keep both suites alive long enough to trust the new one.
That needs one source of truth, not two copies.

## Decision

A macro step's body is `hurl: |` (as today) **or** `ref: <fragment>`, never both. A
**fragment** is one hurl entry in a real `.hurl` file, named by a comment directly above
it:

```hurl
# tests/hurl/admin.hurl — runs under stock hurl, unmodified
# @proef admin.search
GET {{base}}/api/v1/admin/search/{{index}}
Authorization: Bearer {{apiToken}}
[Query]
q: {{q}}
HTTP 200
[Captures]
recordId: jsonpath "$[0].id"
```

```yaml
bind:                                    # pack scope
  base:     ${url:base}
  apiToken: ${secret:apiToken}
macros:
  searchRecords:
    params: [q, index]
    defaults: { index: records }
    bind: { q: "${q}", index: "${index}" }   # macro scope (quote in flow style)
    steps:
      - ref: admin.search
```

- **The annotation carries a name and nothing else, permanently.** No `retry=`, no
  key/value growth. A comment holding one identifier and zero behaviour cannot become a
  second configuration language, needs no parser or schema of its own, and cannot drift
  from the YAML. All orchestration stays in the pack.
- **Names** are free-form dotted, globally unique (as macro names already are), with
  `file.hurl#name` as a disambiguator (as `pack.yaml#name` already is).
- **One annotation claims exactly one entry.** Forced, not preferred: a fragment reused
  by several macros must be composable into any position of any of them, and a
  multi-entry region is a fixed sequence only its original neighbours can reuse.
- **`bind:`** maps a fragment's `{{names}}` to proef values, at pack, macro and step
  scope, most specific winning, **nothing implicit**. A foreign corpus names its own
  variables; convention-matching would only work on files we wrote. Values resolve once
  per scope instantiation — one binding is one value, two bindings are two values.
- **Non-secret bindings emit as per-entry `[Options] variable:`**; `${secret:…}` never
  does, and routes to `insert_secret` as it always has, so no secret value enters an
  artifact (ADR-0005 intact).
- **Every `{{placeholder}}` must be bound, produced as a `[Captures]` name by a
  preceding step, or supplied by the fragment's own `[Options] variable:`.** A
  fragment's interface needs no declaration anywhere: what it *reads* is read off
  hurl's own AST and crosses the seam as `ScannedFragment::placeholders`.

  The third source was missing from this list until it was found by audit, and its
  absence contradicted the decision above it: a file that answers its own question
  needs fewer variables passed in, which is precisely what makes it runnable on its
  own — so refusing those files refused the ones this ADR exists to accept. What a
  fragment supplies itself crosses the seam as `ScannedFragment::supplied_variables`.

  It is a *supplier*, so it also **collides** with a `bind:` of that name. Both reach
  the entry as `variable: <name>=`, hurl takes the last, and the fragment's own line
  is last — so the bound value would never be sent, and hurl's `variable:` assigning
  into the run-level set rather than scoping means the loss persists into every later
  entry. Refused as `pack::option_declared_twice`, the same rule a doubly-declared
  `retry:` gets, rather than resolved by an implicit precedence nobody wrote down.

  What earlier steps *produce* is derived differently, and deliberately: the core
  scans the emitted text of the steps it has already lowered (`emit::capture_names`).
  It has to, because a preceding step may be an inline `hurl:` block, which no
  scanner ever saw — there is no `ScannedFragment` for it. So the two halves of this
  check reach the core by different routes, and the produced half is the one that
  knows hurl's `[Captures]` syntax inside `proef-core`. A second engine would need
  produced names on the seam instead; nothing is scheduled, and the note is here so
  the asymmetry is a recorded decision rather than a discovery.

### Both body forms stay, because they are not two spellings

Inline does lower-time **text splicing**; `ref:` does run-time **binding**. Neither
subsumes the other, and the boundary is demonstrable rather than stylistic:
`tests/features/packs/breadth.yaml`'s `postCustomNote` splices `${docstring}` — a
multi-line Gherkin docstring — in as a request body. A hurl `[Options] variable:` value
is a single-line scalar (`VariableValue` is Null/Bool/Number/String), so no binding can
express it. Conversely no inline block can be named, shared, or run by stock `hurl`.

Splicing can substitute anything anywhere and is private to one macro. Binding is
limited to what hurl can template, and buys a name, reuse, standalone runnability, and a
static interface check. Choose by capability, not taste.

## Consequences

The same file runs under `proef test` and under `hurl file.hurl --variables-file …` —
one source of truth, no transcription, and ADR-0004's "copy-paste flows both ways"
becomes "no copy at all". A fragment's required and produced variables are machine-known,
so an unbound placeholder is a load/lower-time error with a real `file:line` — a check
the inline form structurally cannot perform. `probe_lower`'s two-candidate guessing does
not apply to fragments: they parse as authored.

Costs, stated rather than discovered later:

- **A test spans three files** (`.feature` → pack → `.hurl`) instead of two. `explain`
  and LSP go-to-definition have to earn that back — and both now do. Go-to-definition
  on a `ref:` line lands on the annotation; a `ref:` step records the fragment it ran
  as `file.hurl#name` in `step_finished` (an additive event field — ADR-0008 — absent
  for inline steps, so no pre-existing record changes a byte), which `explain` prints
  under a failure as `via …`. The qualified spelling is the one `ref:` itself accepts,
  so a post-mortem line pastes straight back into a pack.

  The record carries the name because it must stand alone: by the time anyone reads it,
  the pack that named the fragment may say something else. That is also why the path is
  shortened to a project-relative spelling before it is stored — `[run] fragments`
  resolves against the config file's directory, and an absolute root would put a
  machine-specific path in a durable artifact and stop two checkouts' records from
  comparing equal.
- **A bound value gets exactly one expansion pass, and no more.** hurl's `eval_template`
  is a single non-recursive pass, so a *rendered* variable's value is never re-parsed as
  a template. But a `[Options] variable:` value is itself evaluated as a template before
  it is stored (`hurl-8.0.1/src/runner/options.rs:508-511`), which is one pass more than
  the entry body gets. So `bind: { recordUrl: "${url:record}" }`, where `record` is
  `"${url:base}/api/v1/records/{{recordId}}"`, does work: `{{recordId}}` expands at
  option-eval time from a preceding capture, and `GET {{recordUrl}}` then renders the
  finished URL. `[url]`'s path table keeps working for fragments, and ADR-0012 is
  unchanged.

  The limit is the *second* level: a value that expands to text still containing `{{…}}`
  will emit those characters literally. In practice that is the same requirement the
  bound-or-captured rule already enforces — every placeholder must be in scope at the
  entry that reads it.

  *(An earlier draft of this ADR concluded that fragment paths had to move into the
  `.hurl` file and that `[url]` would narrow to `base`. That was wrong: it read the
  non-recursive `eval_template` as applying to the binding path too. Recorded because
  the wrong version would have forced a needless `proef.toml` migration.)*
- **Two ways to write a request body** — accepted deliberately above, and the reason is
  recorded so the 844-line evidence is not forgotten by someone later tempted to
  deprecate the inline form.
- **proef reads files it does not own**, so it must never write them: `fmt` refuses
  `.hurl` in directory discovery, and a foreign corpus stays byte-untouched.

## Charter

PRD §3's hurl non-goal is amended in the same change (PRD "Amendment (2026-08-11)"):
it forbids **generating** Gherkin/macros/prose from hurl, not hurl text being an input
source. Nothing here generates anything — features and macros stay hand-authored, and a
fragment is inert until a macro names it. ADR-0016 stays declined on the untouched
reasoning. The amendment records that OPEN-FINDINGS M3 asked for this re-examination to
arrive with a measured port cost, and that it has not.

This is **not** M2 (mechanical equivalence between a hurl corpus and its proef port).
The integration test that runs one fragment both ways proves the file is dual-runnable;
it does not compare two suites' results, and M2 stays open.

## Alternatives considered

**Region-claiming annotations** (one comment claims every entry until the next) — fewer
comments, but a claimed region is a fixed sequence, so the reuse requirement kills it;
and pairing YAML modifiers positionally with claimed entries is exactly the coupling
`emit.rs`'s `MapEntry.step` comment already warns against ("explicit, never positional").
**One file per macro** — trivial rule, but it dictates the layout of a corpus we do not
own. **Metadata in the annotation** (`# @proef.step retry=10x300ms`) — maximum locality,
at the price of a second configuration language with its own parser, schema and
finite-retry lint, inside somebody else's file. **Replacing inline entirely** — rejected:
the 844-line corpus is evidence the paste path is sufficient for real work, and
`${docstring}` splicing has no binding equivalent. **Implicit binding from config keys**
— shortest packs, but a name would bind from a file the pack never mentions, and a
foreign corpus's names rarely match anyway.

## Prior art

The shape is well-established: `-- name:` magic comments naming queries inside valid
`.sql` files (yesql, HugSQL, aiosql, sqlc); `# @name` naming requests in JetBrains'
`.http` client, which can import and run them by name across files; and the OpenAPI
Initiative's **Arazzo** specification, a separate declarative workflow document whose
steps reference operations by `operationId` defined elsewhere — the same dependency
direction chosen here, where the definition file knows nothing about its consumers.
Reuse across hurl files is an acknowledged, unresolved gap upstream
(Orange-OpenSource/hurl #317, #4574), so nothing here conflicts with a shipped hurl
feature. The known failure mode of magic comments — invisible coupling — is answered by
the name-only rule and by reading the annotation off hurl's own AST, where
comment-to-entry attachment is already modelled.

## Amendment — 2026-08-12 (the scanner reports unannotated entries, by line)

`FragmentScanner` returns `ScannedFile { fragments, unannotated }` rather than
`Vec<ScannedFragment>`. The named half is unchanged; the addition is the 1-based start
line of every entry carrying **no** `# @proef` annotation.

The original contract dropped those entries at scan time, on the argument — still
correct — that nothing downstream can use one, and that a corpus proef did not write is
expected to be mostly unannotated, so building a `ScannedFragment` for each would be the
bulk of a scan for nobody's benefit. That reasoning covers *building fragments*. It does
not cover *counting*, and the difference showed up in the field: a 97-entry corpus port
found that missing an annotation on one entry produces a green dry-run and a silently
absent test, with no signal at scan, bind, or run time — because the entry that would
prove it was never built. Neither could any command state how many entries a corpus
held, so there was no denominator against which the gap could be noticed.

A line number costs a push and is all a listing can point at, there being no name to
print. The performance argument is therefore preserved intact: nothing extra is
constructed, and `proef fragments` consumes what the scan already had to walk past.

**Unannotated is not an error.** `proef fragments --check` fails on annotated fragments
no scenario runs; failing on unannotated entries requires `--require-annotated`. During
a port "unannotated" means *not done yet*; in steady state it means *deliberately not
exposed*, which is the premise that lets this ADR promise that pointing at a corpus you
did not write costs nothing. Gating every adopter on the porting reading would have
contradicted it, so the porting team asks for that check explicitly.

`StepKindSpec` also gained `options`, an engine-contributed recogniser mapping a raw
option key to what the core's ADR-0007 budget rules should make of it. The fragment half
of that rule already crossed the seam (`ScannedFragment::declared_options`) while the
inline half matched `"retry-interval:"` as a literal inside `proef-core` — one rule at
two altitudes, and a second engine would have had its fragments linted and its inline
blocks not. Option *spellings* now live only in the engine that owns them. Option
*baking* (`lower.rs`) still writes hurl syntax directly; the emitter is hurl-shaped by
ADR-0010 and is a separate question this amendment does not address.
