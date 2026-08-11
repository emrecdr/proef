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
    bind: { q: ${q}, index: ${index} }   # macro scope
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
- **Every `{{placeholder}}` must be bound, or produced as a `[Captures]` name by a
  preceding step.** Both sets are read off hurl's own AST, so a fragment's interface
  needs no declaration anywhere.

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
  and LSP go-to-definition have to earn that back.
- **Fragment URLs cannot come from `${url:<name>}` when the value contains `{{…}}`.**
  hurl's `eval_template` is a single non-recursive pass — a rendered variable's value is
  never re-templated — so `channelState = "${url:base}/…/{{channelId}}/state"` would put
  the literal characters `{{channelId}}` on the wire. Paths therefore live in the
  `.hurl` file and `[url]` supplies `base`. **Fragments only**; inline blocks keep the
  full path table (amends ADR-0012).
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
