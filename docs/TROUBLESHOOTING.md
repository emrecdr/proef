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
| `1` | at least one check failed: a test assertion, a cancelled run — or a `--check`-style gate (`fmt --check`, `fragments --check`, `diff --fail-on-regression`) that found what it gates on | fix the system under test — or the expectation |
| `2` | your input is at fault: packs, features, flags, filters, secrets, bad `{{var}}`/JSONPath | the diagnostic names the file and line |
| `3` | the environment or proef is at fault: unreachable target, native libs, IO, output proef could not write (full disk, failing device) | check the target, `proef doctor`, disk |
| `130` | interrupted twice — the second Ctrl-C is a hard exit (128 + SIGINT), so cleanup and the record's tail are skipped | the run record will read as *incomplete*; a single Ctrl-C cancels gracefully and still runs `[run] teardown` |

**Step glyphs:**

| Glyph | Status | Meaning |
|---|---|---|
| `✓` | passed | ran, all asserts held |
| `✗` | failed | ran, an assert failed (details + `reproduce:` line at the end) |
| `∅` | skipped | not run: `when:` guard, an earlier failure, cancellation, or an authored `@skip` on the scenario (the tag spelling prints as the reason) |
| `⚠` | warned | an `optional:` step failed, or a `saveAs: global` promotion was refused — the reason prints on the `↳` line |

Under `--console dotted` the tree collapses to one glyph per *scenario* —
`.` passed, `F` failed, `s` skipped, `w` warned (lowercase = non-gating) —
and failures still print in full after the pool; `--console quiet` keeps only
the run line and the summary. On a terminal the status vocabulary is
colored; `NO_COLOR` (or a non-terminal stream) turns it off, and `run.log`
never carries the paint either way. The record and the exit code are
identical in every mode. Every run's last line names its run id and
wall-clock — the id is the reproduction key `--shard`, `--shuffle` and
`${fake:…}` all hang off.

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
repeats + margin, ADR-0007). Usually a huge `retry:`/`delay:` (or the hurl-side `[Options] repeat:`) value
(the pack lint caps literals). A `{{var}}`-driven
`retry:`/`delay:`/`[Options] repeat:`/`max-time:` cannot be estimated at all — it resolves
inside hurl at run time — so the batch falls back to the default budget
(`[http] timeout` × 4, at least 60s) rather than to an estimate that assumes no
retries. If a legitimately long templated retry is being abandoned, raise
`[http] timeout`.

**"global state file .proef-state.json is not valid JSON"** — the persistent
World is derived data (only `saveAs: global` promotions live there).
Deleting `.proef-state.json` is safe; the next run recreates it.

**Corrupt `.proef-secrets.json`** — `proef doctor` names it; the next
`proef secret set` moves the wreck to `.proef-secrets.json.corrupt` and
starts fresh. Values are re-enterable; nothing else references the file.

**"no scenarios matched the filters"** — `--tags`/`--scenario` selected
nothing; exit 2 by design so a typo'd filter can never produce a silent
green CI run.

**A failure line carries `(via tests/hurl/admin.hurl#admin.search)`** — not an error. The
step ran a *named fragment* (ADR-0018) rather than an inline `hurl:` block, and that is
the third file involved: the request lives there, not in the feature or the pack. The
spelling is the one `ref:` accepts, so it pastes straight back into a pack. Every failure
sink carries it — console, `explain`, TAP, `JUnit`, the GitHub summary, the HTML report.

**"`ref:` names no loaded fragment"** (`pack::unknown_ref`) — either the name is a typo
(the message suggests the closest) or **no fragment files were loaded at all**, which the
message says plainly. The usual cause is a missing `[run] fragments` in `proef.toml`:
without it nothing is scanned, so every `ref:` is unknown. Note the root resolves against
the **config file's directory**, not your working directory.

**"reads `name`, which nothing supplies"** (`lower::unbound_placeholder`) — a
fragment's variables are not implicit: what the `.hurl` file reads must be bound at pack,
macro or step scope, captured by an earlier step, supplied by the fragment's own
`[Options] variable:`, or carried by a secret of that name — and inside a `bind:`
*value*, an earlier-sorting sibling literal counts too (injected lines evaluate in name
order). `--dry-run` reports it without a network. With `proef lsp` running, completing
inside a `bind:` table offers what the pack's `ref:`ed fragments still need — the union
over every fragment the pack refs (ranked by the nearest `ref:`, each labelled with its
owner), minus the names a fragment supplies itself.

**"`index` is supplied twice"** (`pack::option_declared_twice`) — the fragment sets the
name in its own `[Options] variable:` *and* a `bind:` supplies it. Both reach the entry
as `variable: index=` and hurl takes the last, which is the fragment's — so the bound
value would silently never be sent. Delete whichever is not authoritative: the `bind:`
if the file's own default is right, the `variable:` line if the pack should decide. The
same rule already applies to `retry:`/`delay:`.

**Go-to-definition on a `ref:` does nothing** — check that `[run] fragments` is set and
that the editor's workspace root matches where `proef.toml` lives; the server resolves the
corpus relative to that file's directory.

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
exact executed `.hurl` files. `proef explain` summarizes the latest run,
`proef diff [base] [new]` compares two of them (regressions, fixes, flakiness,
perf), and `proef report [run]` writes a self-contained HTML page of a run; the
`reproduce: hurl --test …` line under a failure replays the artifact with stock
hurl, taking proef out of the loop entirely.

A **cancelled** run (Ctrl-C) is a *complete* record, not a truncated one —
`proef explain`/`proef report` never banner it as incomplete, because its
nonzero `skipped` count already says what didn't get to run. `proef diff
--fail-on-regression` disagrees on purpose: it still refuses to certify "no
regressions" against a cancelled run, since a regression could be hiding among
the scenarios it never reached. The three commands' differing treatment of
`cancelled` is a deliberate choice, not an inconsistency.

`proef diff` takes each side as a **run id, a record directory, or an events
`.jsonl` file** — the stream is the record, so a file means the same thing
under any name. That third form is the CI baseline flow: upload
`.proef-runs/<id>/events.jsonl` as an artifact on your main branch, download
it in the PR job as (say) `baseline.jsonl`, then

```console
$ proef test tests/features --run-id pr
$ proef diff baseline.jsonl pr --fail-on-regression   # exit 1 on passed → failed
```

and a scenario that regressed against main fails the PR without a run record
store shared between jobs.

`proef diff` keys steps by `(text, ordinal)` so that line shifts don't lie and
repeated steps stay distinct. The trade-off is positional: if a scenario loses
an earlier duplicate of a step, every later instance shifts down one ordinal,
and the comparison lines up two different runs' steps. Timing and attempt
counts for the shifted steps can then be attributed to the wrong one. Renaming
or reordering steps between the two runs being compared is worth a second look
at the numbers; adding or removing steps at the end is not affected.
