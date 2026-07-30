# Getting started — your first suite in ten minutes

proef runs end-to-end API tests written as plain Gherkin prose. The prose stays
readable by anyone; a YAML *pack* binds each sentence to real HTTP work. This
walkthrough builds a two-file suite from nothing and runs it.

Install first (see the [README](../README.md#installation)), then verify:

```console
$ proef doctor
engine `hurl`:
  [ok  ] embedded hurl            hurl 8.0.1 (exact pin, ADR-0003)
  ...
```

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
# baseURL: ${env:PROEF_BASE_URL:-http://127.0.0.1:8787}
Feature: Directory search
  Scenario: A known record is found
    Given the service is healthy
    When the operator searches for "Acme"
    Then the first hit is record "r-1"
```

The `# baseURL:` line is a *directive*: a named value every step can use as
`${baseURL}`. Directives resolve environment variables with defaults
(`${env:PROEF_BASE_URL:-…}`), so the same suite points at any environment.

## 3. Bind the prose

`suite/packs/api.yaml`:

```yaml
macros:
  health:
    match: the service is healthy
    steps:
      - hurl: |
          GET ${baseURL}/health
          HTTP 200

  search:
    params: [term]
    match: the operator searches for {term}
    steps:
      - name: search records for ${term}
        hurl: |
          GET ${baseURL}/api/v1/admin/search/records
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

## 3.5 Keep URLs and variables out of the tests (`proef.toml`)

The `# baseURL:` directive above lives *inside* the feature. To keep test files
pure — no URLs, no environment data — declare those in `proef.toml` at the
project root and reference them as `${url:…}` / `${vars:…}`:

```toml
# proef.toml
[run]
suite = "suite"                    # `proef test` now needs no path argument

[url]
base = "http://127.0.0.1:8787"     # → ${url:base}

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
$ proef test                       # base [url]/[vars]; discovers the default `tests/` suite
$ proef test --env prod            # [env.prod.*] deep-merged over the base (or set PROEF_ENV=prod)
```

The rule is uniform: under `[env.<name>]`, `url.*` / `vars.*` override variables and
`http.*` / `run.*` override runner settings — anything unlisted **inherits the base**,
so an environment names only what changes (the Cloudflare-Wrangler / Cargo-profile model).
Secrets never live here — they stay in the encrypted store (`${secret:…}`, §5). The
in-feature `# baseURL:` directive still works for one-off per-file overrides.

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
dev fixture in a second terminal (`cargo run -p xtask -- fixture` from a
checkout prints the URL to export). Then:

```console
$ proef secret set apiToken        # encrypted store; or: export PROEF_SECRET_APITOKEN=…
$ proef test suite
running 1 scenario(s) with 8 job(s) — run 019f…

  Scenario: A known record is found (suite/case.feature)
    ✓ suite/case.feature:4 — the service is healthy (2ms)
    ✓ suite/case.feature:5 — the operator searches for "Acme" (5ms)
    ✓ scenario A known record is found

summary: 1 passed · 0 failed · 0 skipped
```

Secret *values* never appear anywhere — not in artifacts, events, logs, or
reports.

## 6. When it fails

A failing assert names the feature line, the artifact line, and hands you the
exact command to reproduce it without proef:

```console
  ✗ suite/case.feature:6 — assert failure (artifact case--a-known-record-is-found.hurl:12)
  reproduce: hurl --test .proef-runs/<run-id>/artifacts/case--a-known-record-is-found.hurl
```

Every run leaves a record under `.proef-runs/<run-id>/`: `events.jsonl` (the
machine-readable event stream), `run.log` (the console mirror), and
`artifacts/` — the exact `.hurl` files that were executed, byte for byte.
`proef explain` summarizes the latest run from the record.

## Where next

- [`AUTHORING.md`](AUTHORING.md) — the full pack reference: composition with
  `use:`, retries, optional steps, guards, captures across scenarios, fakes.
- [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md) — exit codes, the glyph legend,
  and the frequent failures; [`DIAGNOSTICS.md`](DIAGNOSTICS.md) indexes every
  error code you might hit.
- `proef flows suite` lists every scenario with tags; `--output json` feeds
  the nextest harness (one IDE test per scenario).
- `proef test suite --watch` reruns on every file change.
- Exit codes are a contract: `0` pass · `1` test failure · `2` your input ·
  `3` environment — safe to wire straight into CI, with `--junit` for reports.
