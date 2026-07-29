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

## 2. Write the prose

`suite/case.feature`:

```gherkin
# baseURL: ${env:PROEF_BASE_URL:-http://127.0.0.1:8787}
Feature: Client search
  Scenario: A known client is found
    Given the service is healthy
    When the admin searches for "Bakker"
    Then the first hit is client "c-1"
```

The `# baseURL:` line is a *directive*: a named value every step can use as
`${baseURL}`. Directives resolve environment variables with defaults
(`${env:PROEF_BASE_URL:-…}`), so the same suite points at any environment.

## 3. Bind the prose

`suite/packs/api.yaml`:

```yaml
templates:
  health:
    match: the service is healthy
    steps:
      - hurl: |
          GET ${baseURL}/health
          HTTP 200

  search:
    params: [term]
    match: the admin searches for {term}
    steps:
      - name: search clients for ${term}
        hurl: |
          GET ${baseURL}/api/v1/admin/search/clients
          Authorization: Bearer ${secret:apiToken}
          [Query]
          q: ${term}
          HTTP 200
          [Captures]
          clientId: jsonpath "$[0].id"

  firstHit:
    params: [id]
    match: the first hit is client {id}
    expect:
      - hurl: |
          jsonpath "$[0].id" == "${id}"
```

Three macro shapes are on display: a fixed sentence (`health`), a
parameterized one (`{term}` captures the quoted word — quotes are shed), and
an assert-only `expect:` macro whose lines merge into the *previous* request's
asserts. The `hurl:` blocks are raw [Hurl](https://hurl.dev) — validated with
the real parser the moment the pack loads.

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

  Scenario: A known client is found (suite/case.feature)
    ✓ suite/case.feature:4 — the service is healthy (2ms)
    ✓ suite/case.feature:5 — the admin searches for "Bakker" (5ms)
    ✓ scenario A known client is found

summary: 1 passed · 0 failed · 0 skipped
```

Secret *values* never appear anywhere — not in artifacts, events, logs, or
reports.

## 6. When it fails

A failing assert names the feature line, the artifact line, and hands you the
exact command to reproduce it without proef:

```console
  ✗ suite/case.feature:6 — assert failure (artifact case--a-known-client-is-found.hurl:12)
  reproduce: hurl --test .proef-runs/<run-id>/artifacts/case--a-known-client-is-found.hurl
```

Every run leaves a record under `.proef-runs/<run-id>/`: `events.jsonl` (the
machine-readable event stream), `run.log` (the console mirror), and
`artifacts/` — the exact `.hurl` files that were executed, byte for byte.
`proef explain` summarizes the latest run from the record.

## Where next

- [`AUTHORING.md`](AUTHORING.md) — the full pack reference: composition with
  `use:`, retries, optional steps, guards, captures across scenarios, fakes.
- `proef flows suite` lists every scenario with tags; `--output json` feeds
  the nextest harness (one IDE test per scenario).
- `proef test suite --watch` reruns on every file change.
- Exit codes are a contract: `0` pass · `1` test failure · `2` your input ·
  `3` environment — safe to wire straight into CI, with `--junit` for reports.
