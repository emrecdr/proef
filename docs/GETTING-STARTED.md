# Getting started — your first suite in ten minutes

proef runs end-to-end API tests written as plain Gherkin prose. The prose stays
readable by anyone; a YAML *pack* binds each sentence to real HTTP work. This
walkthrough builds a two-file suite from nothing and runs it.

Install first (see the [README](https://github.com/emrecdr/proef/blob/main/README.md#installation)), then verify:

```console
$ proef doctor
engine `hurl`:
  [ok  ] embedded hurl            hurl 8.0.1 (exact pin, ADR-0003)
  ...
```

## 0. Or scaffold it: `proef init`

```console
$ proef init
  created ./proef.toml
  created ./suite/case.feature
  created ./suite/packs/api.yaml
  created ./hurl/api.hurl
  created ./.gitignore
  created ./suite/packs/proef-pack.schema.json
  ok ./suite/packs/api.yaml (modeline added)

created 6 file(s), skipped 0
next: proef test --dry-run  (then point ${url:base} at your API — the scaffold's routes are placeholders)
```

`proef init` writes the three files this walkthrough builds by hand below —
`proef.toml`, `suite/case.feature`, `suite/packs/api.yaml` — plus a one-entry
`hurl/api.hurl`, the pack JSON Schema and a `.gitignore`. The `.hurl` file is
there because a step has **two** body forms: the scaffold's pack shows an inline
`hurl:` block and a `ref:` naming that file's `# @proef` entry, so the form an
adopter with an existing hurl corpus wants is visible from the first command
rather than only in the docs. It is a deliberately *smaller* suite than the
one this tutorial builds: no `Then the first hit is record "r-1"` step, no
`firstHit` `expect:` macro, no `${secret:apiToken}`, and `search` targets a
plain `/search` route instead of the tutorial's `/api/v1/admin/search/records` —
while adding one step the tutorial does not, `And the corpus reports its
version`, whose macro is the scaffold's `ref:` example.
Pasting the tutorial's `Then` line into the scaffold's feature file fails with
`bind::unbound_step` — read on to build the fuller suite by hand, or extend
the scaffold yourself once you understand the pieces.

## 1. A suite is two things

```
suite/
  case.feature        # the prose — what the test says
  packs/
    api.yaml          # the vocabulary — what each sentence does
```

Layout is convention, not configuration. `proef test suite` takes one path and
discovers everything under it: every `*.feature` file is a test file (at any
depth), and every `*.yaml`/`*.yml` file directly inside a directory named
`packs` is a macro pack — the `packs` directory itself may sit at any depth.
All packs merge into one vocabulary shared by all feature files — grow the
suite by adding files; nothing needs registering.

## 2. Write the prose

`suite/case.feature`:

```gherkin
Feature: Directory search
  Scenario: A known record is found
    Given the service is healthy
    When the operator searches for "Acme"
    Then the first hit is record "r-1"
```

The feature is pure prose — no URLs, no environment data. The target host and
any variables live in `proef.toml` (§3.5) and reach the packs as
`${url:…}` / `${vars:…}`.

## 3. Bind the prose

`suite/packs/api.yaml`:

```yaml
macros:
  health:
    match: the service is healthy
    steps:
      - hurl: |
          GET ${url:base}/health
          HTTP 200

  search:
    params: [term]
    match: the operator searches for {term}
    steps:
      - name: search records for ${term}
        hurl: |
          GET ${url:base}/api/v1/admin/search/records
          Authorization: Bearer ${secret:apiToken}
          [Query]
          q: ${term}
          HTTP 200
          [Captures]
          recordId: jsonpath "$[0].id"

  firstHit:
    params: [id]
    match: the first hit is record {id}
    expect:
      - hurl: |
          jsonpath "$[0].id" == "${id}"
```

Three macro shapes are on display: a fixed sentence (`health`), a
parameterized one (`{term}` captures the quoted word — quotes are shed), and
an assert-only `expect:` macro whose lines merge into the *previous* request's
asserts. The `hurl:` blocks are raw [Hurl](https://hurl.dev) — validated with
the real parser the moment the pack loads.

## 3.5 Where URLs and variables live (`proef.toml`)

Variables are declared in `proef.toml` — never in the `.feature` files — and
referenced as `${url:…}` / `${vars:…}`. The pack's `${url:base}` above resolves
from here; proef finds the nearest `proef.toml`, searching up from the working
directory (like cargo/git):

```toml
# proef.toml
[run]
suite = "suite"                    # `proef test` needs no path argument

[url]
base = "${env:PROEF_BASE_URL:-http://127.0.0.1:8787}"   # → ${url:base} (env override wins)

[vars]
apiVersion = "v1"                  # → ${vars:apiVersion}

[env.staging.url]                  # per-environment overrides (mirror the base tables)
base = "https://staging.example.com"

[env.prod.url]
base = "https://api.example.com"
[env.prod.http]                    # an env may override a runner setting too
timeout-ms = 60000
```

A macro then reads `GET ${url:base}/api/${vars:apiVersion}/…` with **nothing
declared in the feature**. Pick an environment at run time:

```console
$ proef test                       # base [url]/[vars]; runs `[run] suite` — "suite/" in the file above (`tests/` is only the fallback when the key is unset)
$ proef test --env prod            # [env.prod.*] deep-merged over the base (or set PROEF_ENV=prod)
```

The rule is uniform: under `[env.<name>]`, `url.*` / `vars.*` override variables and
`http.*` / `run.*` override runner settings — anything unlisted **inherits the base**,
so an environment names only what changes (the Cloudflare-Wrangler / Cargo-profile model).
Secrets never live here — they stay in the encrypted store (`${secret:…}`, §5).

## 3.6 A step's body has two forms

Everything above uses an inline `hurl:` block, which is complete and permanent. The other
form is `ref: <name>`, which points at one entry of a **real `.hurl` file** marked
`# @proef <name>`, with values supplied by a `bind:` table:

```toml
# proef.toml — where the corpus lives
[run]
fragments = "tests/hurl"
```

```yaml
    steps:
      - ref: admin.search          # one entry of tests/hurl/admin.hurl
        bind: { q: "${term}" }
```

That file stays valid hurl, so the same bytes run under stock `hurl` *and* under proef —
useful when a corpus already exists and you would rather annotate it once than transcribe
it. Choose by capability, not taste: inline splices text (so `${docstring}` can carry a
multi-line body, which no binding can express), while `ref:` buys a name, reuse, standalone
runnability, and a static check that every variable is supplied. AUTHORING.md §"`hurl:` or
`ref:`" has the full comparison.

## 4. Validate without a network

```console
$ proef test suite --dry-run
```

This binds every sentence, resolves every `${…}`, emits the artifacts, and
parse-validates them — no request is sent. Typos in prose, packs, or hurl
blocks all fail here, with the file, line, and a "did you mean" where one
exists. Wire your editor too: `proef schema --add-to suite/packs/api.yaml`
gives packs autocomplete via the JSON Schema.

## 5. Provide the secret and a target, then run

Point `PROEF_BASE_URL` at any HTTP API you can reach — or start proef's own
dev fixture in a second terminal: from a checkout, `cargo run -p xtask --
fixture` binds the default `base` port (8787), so no `PROEF_BASE_URL` is needed
(it prints a `PROEF_BASE_URL` line to export only when it ends up somewhere
other than 8787 — the port was busy, or you passed a different
`... -- fixture <port>`). It also prints the fixture's own token line:
`export PROEF_SECRET_APITOKEN=fixture-token` — the value the next step needs,
because every `/api/v1/` route rejects anything else with a 401. Then:

```console
$ proef secret set apiToken        # paste fixture-token; or: export PROEF_SECRET_APITOKEN=fixture-token
$ proef test suite
running 1 scenario(s) with 8 job(s) — run 019f…

  Scenario: A known record is found (suite/case.feature)
    ✓ suite/case.feature:3 — the service is healthy (2ms)
    ✓ suite/case.feature:4 — the operator searches for "Acme" (5ms)
    ✓ suite/case.feature:5 — the first hit is record "r-1" (0ms)
    ✓ scenario A known record is found

summary: 1 passed · 0 failed · 0 skipped
```

Secret *values* never appear anywhere — not in artifacts, events, logs, or
reports.

## 6. When it fails

A failing assert names the feature line, the artifact line, and hands you a
reproduce command (absolute paths; captures ride along in a `.vars` file):

```console
  ✗ suite/case.feature:5 — assert failure (artifact case--a-known-record-is-found.hurl:12)
  reproduce: hurl --test /abs/path/.proef-runs/<run-id>/artifacts/case--a-known-record-is-found.hurl --variables-file /abs/path/….vars
```

Because this suite binds `${secret:apiToken}`, the replay also needs the
secret — proef never writes its value anywhere, so add it yourself:
`--secret apiToken=fixture-token` (the artifact's `# replay:` header says so).

Every run leaves a record under `.proef-runs/<run-id>/`: `events.jsonl` (the
machine-readable event stream), `run.log` (the console mirror), and
`artifacts/` — the exact `.hurl` files that were executed, byte for byte.
`proef explain` summarizes the latest run from the record, `proef diff`
compares two runs — surfacing what regressed, what got fixed, and which steps
turned flaky or slower — and `proef report` writes a self-contained HTML page
of a run (scenario tree, timings, and deep-links to the executed artifacts).

## Where next

- [`AUTHORING.md`](AUTHORING.md) — the full pack reference: composition with
  `use:`, retries, optional steps, guards, captures across scenarios, fakes.
- [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md) — exit codes, the glyph legend,
  and the frequent failures; [`DIAGNOSTICS.md`](DIAGNOSTICS.md) indexes every
  error code you might hit.
- `proef flows suite` lists every scenario with tags; `--output json` feeds
  the nextest harness (one IDE test per scenario).
- `proef test suite --watch` reruns on every file change.
- Tag a scenario `@skip` / `@skip:reason` to park it — still counted, its
  reason in every report — or `@quarantine` so a flaky one stops gating CI
  while you fix it ([AUTHORING.md](AUTHORING.md) · Reserved tags).
- Exit codes are a contract: `0` pass · `1` test failure · `2` your input ·
  `3` environment — safe to wire straight into CI, with `--junit` for reports.
