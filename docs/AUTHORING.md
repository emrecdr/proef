# Authoring reference — packs and features from the author's seat

Everything here is validated at load or at `--dry-run` time; nothing fails
only at execution that could have failed earlier. Start with
[GETTING-STARTED](GETTING-STARTED.md) if this is your first suite.

## Feature files

Standard Gherkin: `Feature:`, `Scenario:`, `Background:` (prepended to every
scenario), `Rule:`, `Scenario Outline:` + `Examples:` (expanded, `#N`-deduped —
note the `#N` is *positional*: inserting an Examples row above renames every
instance below it, which re-keys their JUnit history and re-buckets them
across `--shard`. A column placeholder in the outline's name
(`Scenario Outline: search finds <q>`) keeps each instance's identity tied to
its data instead of its row number),
data tables (rows become step arguments), and docstrings (delivered to the
macro as the `docstring` param). Keywords (`Given/When/Then/And`) don't affect
binding — only the sentence text does. Prose between the `Feature:` line and
the first scenario is the feature's *description*: never executed, but
`proef flows` prints it (JSON: `featureDescription`) so a file's intent
travels with its inventory.

An outline's `<column>` placeholders substitute into **the docstring too**, not
just the scenario name, step text and table cells. That is how a request body
gets data-driven without leaving the feature file:

```gherkin
  Scenario Outline: Posting <label>
    When a record is posted
      """
      {"label": "<label>", "priority": "<priority>"}
      """
    Then the response status is 201

    Examples:
      | label | priority |
      | alpha | high     |
      | beta  | low      |
```

A `<name>` that is not an Examples column is a parse-time error wherever it
appears, docstrings included.

**Variables** are declared in `proef.toml` (`[url]` / `[vars]`), never in the
feature file, and referenced from packs as `${url:key}` / `${vars:key}` — see
[CONFIG.md](CONFIG.md). Feature files stay free of URLs and environment data.

**Tags** (`@smoke`) accumulate feature→scenario; `proef test --tags <expr>`
selects scenarios by a boolean expression over them — `and`, `or`, `not`, and
parentheses, with the `@` optional (e.g. `--tags "@api and not @slow"` or
`--tags "(smoke or nightly) and not wip"`). A bare tag is a valid expression; a
selection matching nothing is an error, not a silent green run. Atoms may
glob: `*` matches any run of characters, `?` exactly one — anchored to the
whole tag and case-sensitive — so `--tags "JIRA-*"` selects every
ticket-tagged scenario, while a metachar-free atom stays plain equality.

## Reserved tags

Two tag names carry behavior; every other tag is yours (selection via
`--tags`, grouping, traceability):

- **`@quarantine`** — the scenario runs and reports, but a test-failure does
  not gate the exit code, reaches JUnit as `<skipped
  message="quarantined failure (non-gating): …">`, and maps to `# TODO` in
  TAP. For flaky tests while they are being fixed — a User/System fault
  still fails the run.

  Quarantine is a holding pen, not a destination, and `proef flaky` is what
  keeps it one: over the retained records it separates a quarantined scenario
  that fails *every* run (`DISABLED` — switched off, and nobody is watching
  those failures because by design nothing reports them) from one that has
  been green throughout (`recovered` — the tag outlived the problem and is now
  suppressing the next real regression). Neither is visible any other way.
- **`@skip`** / **`@skip:<reason-token>`** — the scenario is parked: never
  prepared or run, counted as skipped, and the reason (the tag spelling
  itself) appears in the console, JUnit, TAP, the record, the report,
  `explain` and `flows`. `--tags "not @skip*"` unselects both spellings when
  you want them gone from the totals too. All-skipped exits 0; `--dry-run`
  still validates a skipped scenario (skip is not a validation waiver).
  In `[run] setup`/`teardown` features, reserved tags have no effect.

Conditional, data-dependent skipping is a *step* concern and stays in packs:
`when:` guards a step, `optional:` soft-fails one, `retry:` bounds one.
That split is deliberate and permanent — scenario prose stays declarative;
there is no scenario-level IF/WHILE/TRY and none is planned (ADR-0019).

## Macros (`macros:` in a pack)

```yaml
macros:
  name:
    match: the record {name} is resolved   # sentence pattern (optional)
    params: [name]                         # declared parameters
    defaults: { index: records }           # defaults for optional params
    description: One line for humans.
    tags: [Admin]
    steps: [...]                           # OR expect: [...] — never both
```

- `match:` binds prose. Patterns need at least one literal word (no
  capture-only patterns), captures are `{name}`, quoted arguments shed their
  quotes, and matching is leftmost with ambiguity rejected — two macros that
  could claim the same sentence fail pack load.
- Every `{capture}` must be a declared param; `defaults:` keys must be
  declared params; adjacent captures (`{a} {b}` with nothing between) are
  rejected.
- A macro without `match:` is composition-only (reachable via `use:`).
- `proef macros` lists every macro with its `match:` prose — the sentence a
  feature file may say — and its call count across the corpus, flagging pattern
  macros no scenario binds (dead prose bindings); `use:`-only helpers and unused
  builtins are listed but not flagged. It also flags **near-duplicate** pattern
  macros — two that differ only in their `{capture}` names (the same literal
  skeleton), which are confusable to authors. Both are advisory only: they never
  change the exit code, and `--format json` carries `pattern`, `unused` and
  `nearDuplicateOf` fields for a CI hygiene gate.
- When a step does not bind, `macros` still lists the vocabulary (that is when
  you most need it) and keeps exit 2. Counts are **withheld**, not zeroed:
  `calls`/`unused` render as `—`/`null`, because an unbound feature contributes
  no calls and would make its own macros look dead. `proef flows` still refuses
  — it promises *every* scenario, and a silently partial list is a wrong answer.

## Steps

```yaml
steps:
  - name: human label (${…} resolves here too)
    optional: true                    # failure warns instead of failing
    retry: { count: 10, interval_ms: 300 }   # finite; 1..=10000
    delay: 250                        # ms before the request (capped at 1 hour)
    when: "${env:RUN_SLOW:-}"         # skips when empty or false/0 after resolution
    saveAs: { recordId: global }      # promote a capture to the global store
    hurl: |                           # the payload — raw hurl, one or more entries
      GET ${url:base}/api/v1/records/{{recordId}}
      HTTP 200
  - use: otherMacro                   # composition (cycle-checked, depth ≤ 32)
    with: { term: "${name}" }
```

`retry:`/`delay:` are baked into the entry's `[Options]` so the emitted
artifact replays with identical semantics under stock hurl. `optional:` steps
run as their own batch so a failure cannot poison neighbours.

**`expect:` macros** carry no requests: `status: 200` and/or raw `hurl:`
assert lines merge into the *previous* request entry (a `Then` before any
`When` is an error).

### `hurl:` or `ref:` — two body forms, chosen by capability

A step's body is an inline `hurl:` block **or** a `ref:` naming an entry in a
real `.hurl` file (ADR-0018). They are not two spellings of one thing, so the
choice is not a matter of taste:

| | `hurl: \|` | `ref: name` |
|---|---|---|
| variables | `${…}` **spliced** before hurl parses | `{{…}}` **bound** via `bind:` |
| can substitute | anything, anywhere — including a whole multi-line docstring body | only what hurl can template |
| reuse | none: the block has no name | any number of macros, each binding differently |
| runs under stock `hurl` | no | yes, unchanged |
| unknown variable | caught when the artifact is parsed | caught at `--dry-run`, by name |

**Reach for inline** for a request only this macro makes, and always when you
need to splice something hurl cannot template — `${docstring}` as a request
body is the clearest case, since a bound value is a single-line scalar.

**Reach for `ref:`** when the same request serves several macros, when the hurl
was written by somebody else, or when the file must stay runnable on its own.

```yaml
bind:                              # pack scope — every macro in this file
  base:     ${url:base}
  apiToken: ${secret:apiToken}     # injected at run time, never into an artifact
macros:
  search:
    params: [q]
    match: "the operator searches for {q}"
    bind: { q: "${q}" }            # macro scope
    steps:
      - ref: admin.search
        bind: { index: records }   # step scope — the most specific wins
```

**`hurl:` and `ref:` are chosen per step, not per suite.** A step is one or the
other, but a macro mixes them freely — which is what adopting an existing corpus
looks like in practice: `ref:` the requests the corpus already has, write inline
for the ones it doesn't.

```yaml
  archiveFirstResult:
    match: the operator archives the first result
    steps:
      - ref: admin.search        # the corpus already has this request
      - hurl: |                  # this one is new, and splices ${…}
          POST ${url:base}/api/v1/admin/records/{{recordId}}/archive
          HTTP 204
```

`recordId` is captured by the fragment and read by the inline step: the World
threads captures across both forms, and contiguous same-engine steps batch
together whichever form they were written in. Pick per step by capability —
inline splices `${…}` anywhere (including a multi-line `${docstring}` body, which
a single-line bound scalar cannot express); `ref:` keeps the file runnable on its
own and checks its interface by name.

`proef fragments` lists the corpus: which entries exist, how many scenarios run
each, and — the two questions a listing exists for — which are annotated but
reached by nothing, and which carry no `# @proef` at all and so cannot be
referenced. `--check` exits 1 on the first; add `--require-annotated` to include
the second, which is opt-in because an unannotated entry is inert *by design*
(pointing at a corpus you did not write costs nothing), and only a team
mid-port means "not done yet" by it.

Set `[run] fragments` to the directory holding those files (see
[CONFIG.md](CONFIG.md)). Every `{{variable}}` a fragment reads must be bound in
one of the three scopes, captured by an earlier step, or supplied by the fragment
itself; nothing is implicit, because hurl's per-entry `variable:` assigns into one
shared set rather than scoping, so an unbound name would quietly inherit an
earlier entry's value.

A fragment supplies its own value with an ordinary `[Options] variable:` line —
which is how a corpus file stays runnable on its own, with fewer variables to
pass in:

```hurl
# @proef admin.search
GET {{base}}/api/v1/admin/search/{{index}}
[Options]
variable: index=records      # the file answers its own question
HTTP 200
```

Do **not** then also `bind:` that name. Both spellings reach the entry as
`variable: index=`, hurl takes the last, and the fragment's own line is last — so
the bound value would never reach the request. Proef refuses the pair
(`pack::option_declared_twice`) rather than picking one silently; delete whichever
is not authoritative.

Bindings resolve **once per scope instantiation** — pack scope once per scenario,
macro scope once per invocation, step scope per step — so one `bind:` entry is one
value, and two are two. That is what makes a pack-scope `${fake:email}` a single
identity for the whole scenario.

A binding nothing can read is refused rather than dropped, at two levels:

- **The table** — a `bind:` with no `ref:` in scope to read it
  (`proef::pack::bind_without_ref`), checked at all three scopes.
- **One key** — a `bind:` entry no fragment in that scope reads
  (`proef::pack::unread_bind_key`), with did-you-mean over the names that *are*
  read. This is the one a typo produces: `bind: { token: …, toekn: … }` binds one
  real key and one that never arrives.

The key check is a union over the scope, never against a single fragment — a
pack-scope table is the plumbing every macro in the file needs, so a key serving
one macro and not its siblings is correct usage.

Note the one that surprises people: a macro-scope `bind:` does **not** reach a
`use:` target — the target resolves its own pack and macro scopes — so the table
belongs on the macro that actually carries the `ref:`.

You do not have to memorise a corpus you did not write: with `proef lsp` running,
completing inside a `bind:` table offers the `{{variables}}` the fragments this
pack `ref:`s actually read, each labelled with the fragment that wants it. The
names are read off the `.hurl` file itself, so they cannot drift from it. If a
name still goes unsupplied, `proef::lower::unbound_placeholder` names it at lower
time — `--dry-run` is enough to surface that, no server needed.

The same check reads *inside* your bind values — with hurl's own parser, so
what hurl calls a function is never mistaken for a variable (`{{newUuid}}` and
`{{newDate}}` need no supplier). A `{{name}}` that *is* a variable is templated
when the entry runs, so it must be supplied by then: an earlier step's capture
(the usual shape: `bind: { path: "records/{{recordId}}" }`), the fragment's own
`[Options] variable:` (its lines evaluate before the injected ones), a secret
in scope, or a sibling literal bind whose name sorts before this one — the
injected lines are written and evaluated in name order, so `base` can feed `q`
but not the other way around. And the reverse collision warns rather than
fails: a literal `bind:` that re-uses a name an earlier step *captured* wins
silently from that entry on (hurl's `variable:` assigns into one shared set),
so `proef::lower::bind_shadows_capture` names it — rename the binding if the
capture was the point.

### Where a `file,…;` asset lives

A file body — a request body, a multipart part, or a `file,…;` an assert
compares against — sits **beside the source that names it**:

| The step is | The file goes | Because |
|---|---|---|
| `hurl: \|` (inline) | beside the **feature** | the suite is one unit; `tests/features/fixture.jpg` serves `tests/features/*.feature` |
| `ref: <name>` | beside the **fragment** | that is where stock `hurl` looks, so the file keeps running on its own |

This is hurl's own rule (its `--file-root` defaults to the `.hurl` file's
directory), and the one Karate, pytest and Jest use for fixtures. You never
set a root: proef stages each asset from its own source into the run's
`artifacts/assets/<slug>/`, and the emitted artifact carries the matching
`--file-root` in its replay line, so the hand-off runs unchanged under stock
`hurl`.

Two consequences worth knowing. The path must be a plain relative one — no
leading `/`, no `..` — because it names a file inside your suite, not a
location on the machine. And a missing file fails the scenario *before* the
request, naming the directory it was looked for in
(`proef::run::asset_unstageable`), rather than letting hurl report an
unreadable body against the artifact.

## Recipes — the three shapes every real suite needs

### Log in, then use the token

The most common real-world flow: the API mints a token at a login endpoint,
and every later request carries it. A capture crosses steps through the
World, so the shape is one `login` macro capturing the token and any number
of authed macros reading it:

```yaml
macros:
  logIn:
    match: the operator logs in
    steps:
      - hurl: |
          POST ${url:base}/auth/login
          {"user": "${vars:user}", "password": "${secret:password}"}
          HTTP 200
          [Captures]
          token: jsonpath "$.token"

  listRecords:
    match: the records are listed
    steps:
      - hurl: |
          GET ${url:base}/records
          Authorization: Bearer {{token}}
          HTTP 200
```

`{{token}}` is hurl run-time vocabulary (the second tier): the capture is
assigned when the login entry runs, and every later entry in the scenario
reads it — no `bind:`, no config. To reuse one login across *scenarios*, add
`saveAs: { token: global }` to the login step and read `${global:token}`;
note the promotion is refused if the value carries a secret (ADR-0005).

### Wait for an eventually-consistent result

A `POST` answers `202` and the resource appears a moment later. Don't sleep —
put a finite `retry:` on the step that polls, and let its asserts be the
condition:

```yaml
  awaitRecord:
    params: [id]
    match: record {id} is eventually visible
    steps:
      - retry: { count: 10, interval_ms: 300 }
        hurl: |
          GET ${url:base}/records/${id}
          HTTP 200
```

The retry re-runs the entry until its asserts pass or the count runs out —
and the count **must** be finite: hurl cannot be interrupted mid-call, so an
unbounded retry is a hang the watchdog would have to abandon (the same
reason `retry: -1` is refused at load). The retry also bakes into the
emitted artifact's `[Options]`, so a replay under stock `hurl` polls the
same way.

### Seed data before, clean up after

Three scopes, three mechanisms — pick by lifetime:

- **Per scenario**: a `Background:` in the feature file runs its steps
  before every scenario in that file — provisioning prose, bound to macros
  like any other step.
- **Per suite**: `[run] setup` / `[run] teardown` in `proef.toml` name
  feature files that run once around the whole pool — seed a database,
  then delete the run's residue. Teardown runs even when the suite fails
  or is interrupted; a teardown failure is exit 3, never silent. The keys
  are documented in [Configuration](CONFIG.md).
- **Across scenarios**: a `saveAs: { id: global }` capture in setup (or any
  scenario) persists into later scenarios and runs as `${global:id}` — the
  handle teardown needs to delete what setup created.

## Asserting responses — the hurl vocabulary

Assertions live inside a step's raw `hurl:` block (or an `expect:` macro), so the
whole **hurl 8.0** assert grammar is available *untouched*: proef resolves `${…}`
before the run and hands the rest to the embedded engine verbatim (ADR-0005). The
block is parsed at pack-load time, so a grammar slip fails fast with a diagnostic
— only JSONPath *semantics* (a query that lexes but never matches at run time) can
slip through. An assert reads `<query> [filters…] <predicate>`; `HTTP <status>` is
the implicit status assert. The authoritative list is hurl's own
[asserting-response](https://hurl.dev/docs/asserting-response.html) and
[filters](https://hurl.dev/docs/filters.html) docs; the common shape:

```hurl
GET ${url:record}
Authorization: Bearer ${secret:apiToken}
HTTP 200
[Asserts]
jsonpath "$.id"        isUuid                 # type/shape checks — schema-lite
jsonpath "$.status"    == "active"
jsonpath "$.createdAt" isIsoDate
jsonpath "$.tags"      count == 3
header "Content-Type"  contains "application/json"
duration               < 1000                 # response-time budget (ms)
```

- **Queries** (what to read): `status`, `header "<n>"`, `cookie "<n>"`, `body`,
  `bytes`, `jsonpath "<expr>"`, `xpath "<expr>"`, `regex "<pat>"`, `sha256`,
  `md5`, `url`, `redirects`, `variable "<n>"`, `duration`, `certificate "<f>"`.
- **Predicates** (the check): `== != > >= < <=`, `startsWith`, `endsWith`,
  `contains`, `includes`, `matches "<regex>"`, `exists` / `not exists`,
  `isEmpty`, and the type family `isBoolean` `isInteger` `isFloat` `isNumber`
  `isString` `isCollection` `isList` `isObject` `isDate` `isIsoDate` `isUuid`
  `isIpv4` `isIpv6`.
- **Filters** transform the value before the predicate, chained left→right:
  `count`, `nth <n>`, `first`, `last`, `split "<sep>"`, `replace`/`replaceRegex`,
  `toInt`/`toFloat`/`toString`, `toDate "<fmt>"`, `format "<fmt>"`,
  `base64Decode`/`base64Encode`, `urlDecode`/`urlEncode`, `daysAfterNow`/
  `daysBeforeNow`, `jsonpath "<expr>"`, `regex "<pat>"`, `utf8Decode`.

```hurl
[Asserts]
jsonpath "$.items"     count > 0
header "Set-Cookie"    split ";" nth 0 startsWith "session="
```

**RFC 9535 JSONPath** (hurl 8.0): filter expressions and functions are in scope —
`jsonpath "$.books[?(@.price < 10)]"`, `length()`, `count()`, `match()`,
`search()`. Use the bracket form for names with hyphens (`$['x-custom-id']`), and
assert a missing path with `not exists` (a non-matching path yields *no value*,
not `count == 0`). The same query+filter grammar drives `[Captures]`, threading a
value into later steps as `{{name}}`.

### Built-in shape macros

For the most common single-value shape checks, the built-in `Core` pack ships a
small, product-neutral set of `expect:` macros so you rarely hand-write the
predicate. Each reads a JSONPath (`{path}`) into the previous response and merges
one assert:

```gherkin
When the record is fetched
Then the value at "$.id" is a uuid
And  the value at "$.name" is a string
And  the value at "$.tags" is a non-empty list
```

Available: `the value at {path} is a string` / `… a number` / `… a boolean` /
`… a uuid` / `… an ISO date` / `… present` / `… a non-empty list`. Quote the path
in prose when it contains spaces (the quotes are optional and shed). These are a
convenience layer over the predicates above — they deliberately do not cover
whole-body structural matching; reach for a raw `expect:` `hurl:` block for
anything they omit.

### Negative cases — one macro per malformation, one shared expectation

A validation suite is naturally combinatorial, and its cases differ
*structurally* rather than by value: one omits a key, one empties it, one adds a
key the caller may not set. A single parameterised macro cannot express that —
the bodies are different shapes, not one shape with a hole in it. Outline
placeholders do substitute into docstrings, but an `Examples` cell cannot
practically hold JSON, and a raw body in the feature file defeats the prose the
design exists to protect.

**Name each malformation.** One macro per case, each sentence saying what is
wrong in business terms:

```yaml
macros:
  createWithEmptyTitle:
    match: creating a task with an empty title is refused
    steps:
      - hurl: |
          POST ${url:base}/tasks
          Content-Type: application/json
          {"title": "", "priority": "high"}

  createWithServerOwnedField:
    match: creating a task that sets its own id is refused
    steps:
      - hurl: |
          POST ${url:base}/tasks
          Content-Type: application/json
          {"title": "ok", "id": "caller-chosen"}
```

**Then let one `expect:` macro serve the whole catalogue.** Because an `expect:`
merges its asserts into the *previous* request entry, a single parameterised
expectation covers every case in the set — typically the largest de-duplicator
in a validation pack:

```yaml
  expectErrorCode:
    params: [code]
    match: "the error code is {code}"
    expect:
      - hurl: |
          jsonpath "$.error.code" == "${code}"
```

The scenarios then read as the specification they are, and each one's asserts
land on its own request:

```gherkin
  Scenario: An empty title is rejected
    When creating a task with an empty title is refused
    Then the response status is 422
    And the error code is TITLE_REQUIRED
```

```
# emitted
POST http://127.0.0.1:8787/tasks
Content-Type: application/json
{"title": "", "priority": "high"}
HTTP *
[Asserts]
status == 422
jsonpath "$.error.code" == "TITLE_REQUIRED"
```

**The trade, stated plainly:** the pack grows with the malformation catalogue —
one macro per case, where a value-driven test would have used one row. That is
the cost of feature files that read as prose, and it buys a suite a
non-engineer can review. The expectation side does not grow with it.

## Variables — the two tiers

| Syntax | Resolves | When | Examples |
|---|---|---|---|
| `${…}` | proef | at lowering (before execution) | `${param}`, `${env:NAME:-default}`, `${url:key}`, `${vars:key}`, `${run:id}`, `${global:key}`, `${secret:NAME}`, `${fake:name}` |
| `{{…}}` | hurl | at run time | captures (`{{recordId}}`), secrets (`{{apiToken}}`) |

`${…}` is recursive (captured arguments may themselves contain `${…}`, depth
≤ 8) and `$${` escapes a literal `${`. `${secret:NAME}` never inlines the
value — it lowers to `{{NAME}}` and the engine injects it through hurl's
redaction, so artifacts carry placeholders only.

**Config variables** — `${url:key}` and `${vars:key}` come from `proef.toml`'s
`[url]` / `[vars]` tables, deep-merged with the active `--env` profile. This is how
you keep URLs and settings out of the feature files entirely; a referenced-but-undefined
one is a lower-time error. See [`CONFIG.md`](CONFIG.md) (ADR-0012).

**Captures** (`[Captures]` in a hurl block) flow forward within the scenario
as `{{name}}`. `saveAs: { name: global }` additionally promotes the captured
value into the persistent global store (`.proef-state.json`), where later
scenarios and later runs read it at lowering time as `${global:name}`.

**Fakes** are deterministic synthetic data: `firstName`, `lastName`, `name`,
`fullName`, `email`, `username`, `phoneNL`, `postCode`, `city`, `street`,
`int`, `number`, `digits4`, `digits8`, `bool`, `word`, `uuid` (unknown
generators fail at load). Each `${fake:kind}` **reference** gets its own
value — an occurrence counter advances every time a scenario resolves one, so
independent `${fake:email}` references in the same scenario never collide,
however many a step ends up resolving. A step's `name:` label is the one
deliberate exception: it is not independent of its own payload, so it
replays from the start of the step's own occurrence window instead of
minting new ones — the label's Nth `${fake:…}` reference reuses whichever
occurrence the payload's (and `when:`'s) Nth reference consumed, matched by
*position*, not by generator kind. When the label's `${fake:…}` references
mirror the payload's in kind and order — the common case, e.g. a label that
names the same field the payload sends — this reproduces the payload's own
value exactly. When they diverge in kind (say the label's first reference is
`${fake:fullName}` but the payload's first reference is `${fake:email}`),
the label instead shows whatever that occurrence generated for *its own*
kind — a value the request did not send. That includes a label
with *more* `${fake:…}` references than its payload: each extra one still
reserves its own place in the sequence, so a later step can never be handed
a value the label already displayed. That whole sequence is a pure function
of `${run:id}`: the same `--run-id` reproduces the same fakes, byte for
byte, across runs. The counter restarts at zero for every scenario —
**known limitation:** two *different* scenarios that each resolve
`${fake:email}` at the same position in their own step order (typically each
scenario's first fake reference) get the same address, because both count
from zero independently. If two scenarios must not collide, key the value
yourself (fold in `${run:id}` or a captured id) rather than relying on
`${fake:*}` alone.

## Secrets

Reference with `${secret:NAME}`. Values come from `PROEF_SECRET_<NAME>`
environment variables first, then the encrypted store (`proef secret set NAME`,
or `… | proef secret set NAME --stdin` for scripts (never argv — `ps` shows
it); `proef secret list` names,
`proef secret rm NAME` removes
— XChaCha20-Poly1305, key auto-created `0600` under `~/.config/proef/`).
Values never appear in artifacts, events, logs, or reports; events carry
capture *names* only. The store file (`.proef-secrets.json`, mode `0600`)
holds ciphertext only and is gitignored by default; the key
(`~/.config/proef/keys/default.key`) must never leave your machine.

## Artifacts — the executed input

Every scenario emits `<feature>--<scenario>.hurl` — the exact bytes the
engine executes — plus `.map.json` (artifact lines ↔ feature lines, batch and
step indices) and `.vars` (referenced globals as values, secrets as names).
The header's `# replay:` line is a complete stock-hurl command, including
`--secret NAME=<value>` placeholders for you to fill. `proef artifacts <dir>
-o out/ --run-id ci` emits the same set deterministically for hand-off.

## Runs, records, CI

- Exit codes: `0` pass · `1` test failure (including cancelled runs) · `2`
  input error · `3` system error.
- `.proef-runs/<run-id>/events.jsonl` is the record — one JSON event per
  line (scenario/step lifecycle, per-attempt progress, failure `detail`);
  `proef explain` summarizes it; `--junit` and the GitHub Actions summary
  derive from the same run.
- `--format json` prints one machine-readable summary object on stdout (the
  human report moves to stderr); `--jobs N` controls parallelism;
  `proef.toml` holds project defaults (`[run] jobs`, `[http] timeout-ms`).
- `proef fmt suite --check` keeps hurl blocks canonically formatted;
  `proef flows` lists scenarios.

### IDE integration — one test per scenario

The harness crate bridges proef into any nextest/libtest UI (rust-analyzer,
RustRover, `cargo nextest run`): it lists scenarios via
`proef flows --format json` and runs each as its own test via
`proef test --scenario <name>`.

```bash
PROEF_HARNESS_SUITE=suite cargo nextest run -p proef-harness
```

Set `PROEF_BIN=/path/to/proef` when the binary is not on `PATH`. With
`PROEF_HARNESS_SUITE` unset the harness exposes nothing, so a plain
`cargo test` stays green. Each scenario appears as a separate test in the
IDE's runner — click-to-run one scenario without touching the terminal.

Unset is not the same as unreadable. A variable set to bytes that are not
valid UTF-8 means you asked for something the harness cannot read, so it
exposes a single failing `proef::config` trial naming the variable — it will
not fall back to `proef` on `PATH`, and it will not report green having
listed no tests.

### Secrets in CI

Two working setups:

- **Values via env** (simplest): set `PROEF_SECRET_<NAME>` from your CI's
  secret storage — no store, no key, nothing on disk.
- **Committed ciphertext store**: commit `.proef-secrets.json` (it holds
  ciphertext only) and supply the project key as the `PROEF_KEY` CI secret
  (`base64 < ~/.config/proef/keys/default.key`). A set-but-invalid
  `PROEF_KEY` is always an error, never a silent fallthrough.

Note: `.proef-state.json` (the persistent World) is plaintext and therefore
gitignored — captures promoted with `saveAs: global` land there. A capture
whose value equals a known secret is refused with a warning; it never
persists (`proef doctor` also reports store/key health).

## Environment variables

| Variable | Read by | Purpose |
|---|---|---|
| `PROEF_ENV` | `proef test`/`flows`/`macros`/`artifacts`/`lsp` | Active environment profile — the `--env` flag wins over it |
| `PROEF_SECRET_<NAME>` | `proef test` | Secret value override (beats the encrypted store) |
| `PROEF_KEY` | `proef test`/`secret` | Base64 project key override — decrypt a committed store without the key file |
| `PROEF_CONFIG_DIR` | `proef secret` | Key-file location (default: XDG config dir) |
| `PROEF_HARNESS_SUITE` | nextest harness | Suite directory the harness lists and runs |
| `PROEF_BIN` | nextest harness | Path to the `proef` binary the harness invokes |

**Set-but-unreadable is an error, never silence.** A variable whose value is
not valid UTF-8 is something you asked for and proef cannot read, so it is
reported rather than treated as unset: the commands above exit 2 naming the
variable (`proef doctor` reports it as a failed check and exits 3, with its
other environment findings), and the harness exposes a failing `proef::config`
trial. Reading a malformed `PROEF_KEY` as absent used to mean decrypting with
the wrong key and reporting *tampering*; a malformed `PROEF_ENV` meant running
against the wrong environment. `PROEF_CONFIG_DIR` is a path and is read as raw
bytes, so it has no such failure mode.

Suite-defined env vars (like `PROEF_BASE_URL` in the guides) are a convention
of `${env:…}` references in `proef.toml`, not built-ins — name yours freely.

## Style

Keep prose at business level — no URLs, headers, or JSON in feature files;
that's what packs are for. Prefer several small macros composed with `use:`
over one large one. Give steps `name:` labels — they anchor artifacts,
events, and failure output.

Label a macro's steps whenever it has more than one. A macro with three steps
turns one feature sentence into three engine steps that share a `step` anchor
exactly — same file, same line, same text — so on the console, in the HTML
report, in `JUnit`, TAP and the job summary they arrive as the same sentence
repeated, one row per step. The `name:` is the only thing that distinguishes
them:

```
✓ tests/features/a.feature:9 — the workspace is provisioned › fixture warm-up probe (4ms)
✗ tests/features/a.feature:9 — the workspace is provisioned › provision the environment (7ms)
```

Without the labels those two lines differ only by their glyph.
