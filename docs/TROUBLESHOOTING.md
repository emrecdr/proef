# Troubleshooting

First stop, always: `proef doctor` (native libraries, environment, secret
store/key health) and `proef test <suite> --dry-run` (full validation with
located diagnostics, no network). Every diagnostic code is indexed in
[DIAGNOSTICS.md](DIAGNOSTICS.md).

## Reading the output

**Exit codes are a contract:**

| Code | Meaning | Typical fix |
|---|---|---|
| `0` | everything passed (warnings allowed) | — |
| `1` | tests ran; at least one assertion failed (or the run was cancelled) | fix the system under test — or the expectation |
| `2` | your input is at fault: packs, features, flags, filters, secrets, bad `{{var}}`/JSONPath | the diagnostic names the file and line |
| `3` | the environment or proef is at fault: unreachable target, native libs, IO | check the target, `proef doctor`, disk |

**Step glyphs:**

| Glyph | Status | Meaning |
|---|---|---|
| `✓` | passed | ran, all asserts held |
| `✗` | failed | ran, an assert failed (details + `reproduce:` line at the end) |
| `∅` | skipped | not run: `when:` guard, an earlier failure, or cancellation |
| `⚠` | warned | an `optional:` step failed, or a `saveAs: global` promotion was refused — the reason prints on the `↳` line |

## Frequent situations

**"missing secret value(s)"** — the suite references `${secret:NAME}` with no
value available. Fix: `proef secret set NAME` (or `export
PROEF_SECRET_<NAME>=…`). In CI, see
[AUTHORING — Secrets in CI](AUTHORING.md#secrets-in-ci).

**"unknown environment `<name>`"** — `--env <name>` (or `PROEF_ENV`) names an
environment `proef.toml` doesn't define; the error lists the known ones. Fix the
name or add the `[env.<name>]` section. Exit 2.

**"<url|vars> variable `<key>` is not set"** (`resolve::missing_config_var`) — a
pack references `${url:key}` / `${vars:key}` that neither the base `[url]`/`[vars]`
nor the active `[env.<name>]` defines. Add it, or select the environment that has
it with `--env`. See [CONFIG.md](CONFIG.md).

**"no path given and no default suite found"** — `proef test` got no path and there
is no `[run] suite` in `proef.toml` nor a `tests/` directory. Pass a path, set
`[run] suite`, or create `tests/`. Exit 2.

**Exit 3 with connection errors** — the target is unreachable. Check the URL your
`${url:base}` resolves to for the active `--env` (a
`--dry-run` prints nothing wrong because no request is sent), then the network. The
dev fixture (`cargo run -p xtask -- fixture`) binds the default `${url:base}` port
(8787), so with no `PROEF_BASE_URL` set it becomes your local target automatically
(it falls back to an ephemeral port, printing `PROEF_BASE_URL`, only if 8787 is busy).

**"batch budget exceeded — scenario thread abandoned"** — the watchdog killed
a batch that outran its computed budget (timeouts × attempts + delays +
repeats + margin, ADR-0007). Usually a huge `retry:`/`repeat:`/`delay:` value
(the pack lint caps literals; `{{var}}`-driven values only the budget can
catch) or a target that hangs instead of refusing.

**"global state file .proef-state.json is not valid JSON"** — the persistent
World is derived data (only `saveAs: global` promotions live there).
Deleting `.proef-state.json` is safe; the next run recreates it.

**Corrupt `.proef-secrets.json`** — `proef doctor` names it; the next
`proef secret set` moves the wreck to `.proef-secrets.json.corrupt` and
starts fresh. Values are re-enterable; nothing else references the file.

**"no scenarios matched the filters"** — `--tags`/`--scenario` selected
nothing; exit 2 by design so a typo'd filter can never produce a silent
green CI run.

**"payload does not parse: contains no hurl entries"** — the `hurl:` block is
comments/blank lines only (a temporarily commented-out request). proef
refuses it at load: a step that executes nothing must not report green.

**A `Then` step shows `∅ not run (its request entry did not run)`** — the
request it asserts on was skipped (guard or earlier failure); the asserts had
nothing to attach to.

**Windows/`| head` pipelines** — closed pipes are tolerated everywhere; if a
pipeline misbehaves, check the consumer, not proef's exit code.

## Building from source

`proef-engine-hurl` links native libraries. Debian/Ubuntu:
`apt install build-essential pkg-config libssl-dev libcurl4-openssl-dev
libxml2-dev libclang-dev`. macOS: Xcode CLT suffices. Verify with
`proef doctor` — it reports the embedded hurl, parser/libxml2 linkage, and
libcurl. Prebuilt binaries (brew/binstall/GitHub Releases) need none of this.

## Digging deeper

Every run leaves `.proef-runs/<run-id>/`: `events.jsonl` (the machine record —
[EVENTS.md](EVENTS.md)), `run.log` (console mirror), and `artifacts/` with the
exact executed `.hurl` files. `proef explain` summarizes the latest run and
`proef diff [base] [new]` compares two of them (regressions, fixes, flakiness,
perf); the `reproduce: hurl --test …` line under a failure replays the artifact
with stock hurl, taking proef out of the loop entirely.
