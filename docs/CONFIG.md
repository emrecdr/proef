# `proef.toml` — the project configuration reference

`proef.toml` lives in the project root (the directory you run `proef` from)
and is **committed project config** — it describes the suite, not your
machine. Every key is optional; an absent file means all defaults. Unknown keys
are rejected (`deny_unknown_fields`), so typos fail loudly instead of being
ignored.

The file holds two kinds of thing, kept in distinct sections: **runner config**
(how proef behaves — `[run]`, `[http]`, `[sla]`) and **suite variables** (data your
tests reference — `[url]`, `[vars]`), plus per-environment overrides (`[env.<name>]`).
Secrets never live here (see below).

Precedence (highest wins): built-in defaults < `proef.toml` base tables <
active `[env.<name>]` < command-line flags. For a suite variable that means
`[url]`/`[vars]` < `[env.<active>]`; for a runner setting like `jobs`, the
`--jobs` flag still wins over both.

## Reference

```toml
# ── runner config (how the tool behaves) ────────────────────────
[run]
suite    = "tests"          # default suite path — `proef test` needs no argument
fragments = "tests/hurl"    # root of the hurl files packs may `ref:` (optional)
jobs     = 8                # parallel scenario workers
runs-dir = ".proef-runs"    # where run records land
keep-runs = 200             # past records kept there (0 = only the run in flight)
setup    = "tests/setup.feature"      # run once before the pool (optional)
teardown = "tests/teardown.feature"   # run once after the pool (optional)
exclusive-tags = "@serial"  # scenarios matching this run with the pool to themselves

[http]
timeout-ms      = 30000     # per-request timeout (batch-level default)
follow-location = false     # follow redirects

[sla]                       # opt-in run-level latency budget (omit = no gate)
p95-ms = 250                # 95th-percentile per-step duration ceiling
max-ms = 1000               # slowest single-step ceiling

# ── suite variables (data your tests reference) ─────────────────
[url]
base = "http://127.0.0.1:8787"   # → ${url:base}; the dev fixture's default port

[vars]
apiVersion = "v1"                 # → ${vars:apiVersion}
username   = "dev@example.com"    # → ${vars:username}

# ── per-environment overrides (mirror the base tables) ──────────
[env.staging.url]
base = "https://staging.example.com"

[env.prod.url]
base = "https://api.example.com"
[env.prod.vars]
username = "release@example.com"
[env.prod.http]                   # an environment may override a runner setting too
timeout-ms = 60000
```

| Key | Default | Notes |
|---|---|---|
| `[run] suite` | *(unset)* | default path for `proef test`/`flows`/`artifacts`; falls back to the `tests/` convention, then errors. An explicit path always wins |
| `[run] jobs` | available parallelism | `--jobs` flag wins; live threads never exceed the scenario count |
| `[run] runs-dir` | `.proef-runs` | run records rotate here; only uuid-named run dirs are ever touched |
| `[run] keep-runs` | `200` | how many **past** records `runs-dir` retains, besides the one being written; `0` keeps none but the run in flight |
| `[run] setup` | *(unset)* | feature run **once before** the pool (suite setup); its `saveAs: global` reaches every scenario; a failure aborts the run |
| `[run] teardown` | *(unset)* | feature run **once after** the pool (suite teardown), only if setup succeeded; its failure is a distinct exit 3 |
| `[run] exclusive-tags` | *(unset)* | tag expression (same language as `--tags`) selecting scenarios that run with the pool to themselves; a malformed expression is exit 2 |
| `[http] timeout-ms` | `30000` | per-entry `[Options]` in a hurl block override it |
| `[http] follow-location` | `false` | per-entry `[Options]` override it |
| `[sla] p95-ms` | *(unset)* | 95th-percentile per-step duration ceiling; unset = no gate |
| `[sla] max-ms` | *(unset)* | slowest single-step ceiling; unset = no gate |
| `[url] <key>` | *(none)* | URL variables, referenced as `${url:<key>}` |
| `[vars] <key>` | *(none)* | non-secret variables, referenced as `${vars:<key>}` |
| `[env.<name>.<section>]` | inherits base | per-environment override of any base section (`url`/`vars`/`http`/`run`/`sla`) |

## Environments

An `[env.<name>]` profile mirrors the base tables and **deep-merges** over them,
key by key — so an environment lists only what changes; everything else inherits
from the base (the Cloudflare-Wrangler / Cargo-`[profile.*]` model). Select the
active environment at run time:

```console
$ proef test                 # base [url]/[vars]; discovers the default suite
$ proef test --env prod      # [env.prod.*] merged over the base
$ PROEF_ENV=staging proef test   # same, via the environment variable
```

The rule is uniform: under `[env.<name>]`, `url.*` / `vars.*` override variables and
`http.*` / `run.*` / `sla.*` override runner settings. A named-but-undefined `--env` is a
user error (exit 2) listing the known environments. A `${url:key}` / `${vars:key}` referenced
in a pack but defined in neither the base nor the active environment fails at lower time
(`proef::resolve::missing_config_var`).

## SLA gate (`[sla]`)

The optional `[sla]` table is a **run-level latency budget**: after a run, proef folds
every step's wall-clock duration into two aggregates and fails the run if either exceeds
its ceiling. `p95-ms` caps the 95th-percentile step duration (the typical slow request);
`max-ms` caps the single slowest step. Skipped steps (which never hit the network) are
excluded from the population.

The gate is **opt-in and off by default** — with no `[sla]` table a run behaves exactly as
before. A breach prints the offending metrics and the slowest steps on stderr and maps to
**exit 1** (a test failure), the same code as a failed assertion. It never introduces a new
exit code and never downgrades a `User`/`System` fault (exit 2/3), so a breach can only turn
an otherwise-green run red. Tighten or loosen per environment the usual way:

```toml
[sla]                       # baseline budget
p95-ms = 250
max-ms = 1000

[env.staging.sla]           # staging tolerates slower responses
p95-ms = 800
```

This is distinct from hurl's per-request `duration < <ms>` assert (which fails one step
inside a `hurl:` block): the `[sla]` gate is an aggregate budget over the *whole run*, while
`duration <` is a targeted per-request check. Use the assert for a hard per-endpoint SLA and
`[sla]` for a suite-wide budget.

## Hurl fragments (`[run] fragments`)

Names the root directory holding the `.hurl` files a pack may `ref:` (ADR-0018), scanned
recursively. Those files stay **valid hurl** — the `# @proef <name>` annotation is an
ordinary comment — so the same file runs under `hurl` and under proef, and a corpus you
already own is annotated once instead of transcribed into YAML where it drifts.

```toml
[run]
fragments = "tests/hurl"
```

```hurl
# tests/hurl/admin.hurl — runs as-is under `hurl --variables-file …`
# @proef admin.search
GET {{base}}/api/v1/admin/search/{{index}}
Authorization: Bearer {{apiToken}}
[Query]
q: {{q}}
HTTP 200
```

```yaml
# the pack supplies the fragment's variables; nothing is implicit
bind:
  base:     ${url:base}          # pack scope — every macro in the file
  apiToken: ${secret:apiToken}   # injected at run time, never into an artifact
macros:
  search:
    params: [q, index]
    defaults: { index: records }
    match: "the operator searches for {q}"
    bind: { q: "${q}", index: "${index}" }   # macro scope
    steps:
      - ref: admin.search
```

Relative paths resolve against **this file's directory**, not the working directory, so
the root may sit outside the suite — even outside the repo — and the config still works
from any subdirectory. There is no convention fallback: with the key unset a `ref:`
reports `proef::pack::unknown_ref` and says no fragment files were loaded, which beats
guessing at a directory. Entries with no `# @proef` annotation are inert, and the files
are not scanned at all until some pack names a fragment — so pointing at a corpus you did
not write neither changes what runs nor costs anything to parse. When a pack *does* name
one, the corpus is scanned **once per command**, not once per pack load: a run with
`[run] setup`/`teardown` loads packs four times against the same files, and on a
200-file corpus rescanning each time was most of the run.

`proef fmt` never touches these files: it normalizes hurl blocks *inside packs* by
locating them in YAML, and a corpus you do not own is not proef's to rewrite.

**The read is bounded.** A single corpus file over **8 MiB** is skipped with
`proef::pack::oversized_fragment_file`, and the reader stops once the corpus as a whole
passes **64 MiB** (`proef::pack::fragment_corpus_too_large`). Both name the file, both
leave every other file loading, and a `ref:` into a skipped file then reports
`unknown_ref` beneath them.

These are not tuning knobs — they are the line past which the input is not a test corpus.
A hurl file is human-authored text, so 8 MiB is on the order of 200,000 lines; the largest
corpus anyone has reported is 15 files and 112 fragments. The bound exists because the
root is *one config line*: pointed one directory too high it reads whatever is underneath,
and a 279 MB file cost 601 MB of resident memory on `proef flows` — a command that never
looks at a fragment. If you hit either limit, the fix is almost always to narrow
`[run] fragments` to the directory that actually holds your hurl files.

## Suite setup & teardown (`[run] setup` / `[run] teardown`)

`[run] setup` and `[run] teardown` each name a **feature file** run once around the whole
suite (the model Playwright/Jest `globalSetup` use, ADR-0014). `setup` runs before the
parallel pool and **merges its `saveAs: global` promotions into the shared store before any
scenario lowers**, so it is the place to seed a fixture or provision shared state that every
scenario then reads via `${global:…}`. `teardown` runs once after the pool for cleanup.

```toml
[run]
setup    = "tests/setup.feature"
teardown = "tests/teardown.feature"
```

Failure semantics are deliberate: a **setup** failure **aborts the run before the pool** and
maps to a **user (2)** or **system (3)** fault — a broken fixture is not a failing test, so it
never becomes exit 1. **Teardown runs only if setup succeeded** (it still runs after ordinary
scenario failures, for reliable cleanup), and a **teardown** failure is a **distinct exit 3**
cleanup fault — the suite's own verdict stands, but the failure is never silently swallowed.
Both features are excluded from the pool, so a setup/teardown feature inside the suite
directory never also runs as an ordinary scenario. Auth is already covered by pre-set
secrets, so setup is for **seeding/provisioning**, not obtaining a runtime token.

## Scenarios that cannot share the pool (`[run] exclusive-tags`)

Some scenarios cannot run beside anything: one asserting absolute positions
(`items[0]`) needs a store no concurrent scenario writes to. `exclusive-tags`
is a tag expression — the same language `--tags` takes — selecting those:

```toml
[run]
jobs = 8
exclusive-tags = "@serial"
```

```gherkin
  @serial
  Scenario: The report lists every record in order
```

A matching scenario waits for the pool to drain, runs alone, and the pool refills
after it. Ordering is unchanged: scenarios still start in discovery order, so an
exclusive one never loses its place — and that is also what the cost is made of.
Queueing is strict FIFO, so while an exclusive scenario sits at the head **no new
scenario starts**: the pool drains to empty, the exclusive one runs by itself,
and only then does parallelism return to `jobs` width. Expect a throughput dip
around each exclusive scenario roughly as long as the slowest scenario already
running, plus the exclusive one's own duration. Put another way, the cost is
bounded and predictable rather than free.

**Exclusivity is enforced against the dispatcher's own bookkeeping.** A scenario
the watchdog abandons for blowing its batch budget (ADR-0007) leaves the active
set immediately, but its detached thread keeps going until the request in flight
returns — hurl cannot be cancelled mid-entry. In that window an exclusive
scenario can start while an abandoned neighbour is still issuing requests. It
takes a budget blowout in the same moment to happen, and it is the one caveat the
isolation guarantee carries; a run that never trips the watchdog never meets it.

**This is exclusion, not ordering.** A scenario that must run *before* the
others — installing a fixture the rest depend on — belongs in
[`[run] setup`](#suite-setup--teardown-run-setup--run-teardown), which already
runs once before the pool exists.

**A tag expression in config, not a reserved tag name**, deliberately: with a
bare convention, a scenario added months later lands untagged in the parallel
pool and breaks isolation intermittently — which reads as flakiness rather than
as a missing declaration. The expression keeps the rule in one reviewable place.
A malformed one is a user error (exit 2), never a silently-ignored key.

## What does *not* live here

- **Secrets** — `proef secret set` / `PROEF_SECRET_<NAME>` env, on their own
  encrypted channel (`${secret:…}`); never in `proef.toml`
  (see [AUTHORING — Secrets](AUTHORING.md#secrets)).
- **Per-machine values** — `proef.toml` is committed and shared; anything that
  differs between teammates belongs in an environment variable (`${env:NAME:-default}`)
  or an environment profile you don't commit.

Feature files reference `${url:…}` / `${vars:…}` / `${secret:…}` and declare **none**
of them — variables have exactly one home, this file, and test files stay pure prose.
proef discovers the nearest `proef.toml` by searching up from the working directory
(like cargo/git), so it is found from any subdirectory.

**The search only goes up, so a discovered file must sit at or above the directory
you run from — that is a requirement, not a convention.** Putting `proef.toml`
beside the suite (`tests/proef/proef.toml`) and running from the repository root
does not work by discovery: nothing searches downward, so the config is simply not
found and every `${url:…}` reads as unset.

`--config <path>` names the file instead and removes the constraint entirely:

```console
$ proef test --dry-run --config tests/proef/proef.toml
```

It is global — every subcommand accepts it, including `proef lsp` and
`proef test --watch`. The editor loading a *different* config than the runner is
the drift that makes diagnostics untrustworthy, and `--watch` watches the file
the run actually resolved through, so editing it retriggers.

For `proef lsp` the flag also outranks the workspace root the client announces:
the flag names a file, and a named file is not a guess to be improved on.

A named file that does not exist is a user error (exit 2) for the runner, not a
fall back to defaults: discovery finding nothing means "no project here", but a
named path that is not there is a typo, and answering it with a silently
unconfigured run is the thing worth refusing. `proef lsp` makes the opposite call
and starts anyway — an editor offering less is better than one that will not
boot, and unlike a run it produces no results to be wrong.

It applies to every subcommand in the same way, including the ones that read
nothing from the file: `proef fmt --config missing.toml` exits 2 rather than
formatting and reporting success. The flag names a file, and a named file that is
not there is a typo whatever the command was going to do with it.

### Which directory a path is relative to

**Paths written in `proef.toml` resolve against the directory holding
`proef.toml`. Paths typed on the command line resolve against the working
directory.** That is the whole rule, and it has no exceptions: `suite`,
`fragments`, `setup`, `teardown` and `runs-dir` all follow it, as do the two
files proef keeps beside them — `.proef-state.json` (the persistent World) and
`.proef-secrets.json` (the secret store). An absolute value is taken as written,
so a corpus or a record store may sit outside the project entirely.

The consequence worth stating plainly: **a project is where its `proef.toml` is,
not where your shell is.** Running from a subdirectory finds the same config by
walking up, and now resolves the same suite, writes the same run records, and
reads the same World and the same secrets as running from the root. It is the
convention Cargo (`path`/`members` are manifest-relative), `tsconfig.json` and
pytest's rootdir all follow.

With no `proef.toml` in scope there is nothing to be relative *to*, so paths stay
relative to the working directory, unchanged.

A config that does not sit at the project root spells its paths from where it
sits — a `proef.toml` in `tests/proef/` beside a suite in `tests/features/`
writes `suite = "../features"`.

### How a path is spelled in what proef writes

The same rule, read backwards. **Nothing proef records names the machine it ran
on**: a path that reaches an artifact, a sidecar, an event, a report or a
diagnostic is spelled relative to the directory holding `proef.toml`.

That matters because ADR-0010 makes the emitted `.hurl` a contract — the same
inputs must give the same bytes — and a run record is meant to travel from a
laptop to CI and back. So all four of these produce the *same* record:

```console
$ proef test                              # path derived from [run] suite
$ proef test suite                        # typed
$ proef test /home/you/proj/suite         # typed absolutely
$ cd sub && proef test                    # from a subdirectory
```

Two cases keep an absolute path, because no project-relative spelling of them
exists: a suite or fragment corpus that genuinely sits outside the project
(`fragments = "/opt/shared-corpus"`), and a run with no `proef.toml` in scope. A
path *typed* relative is recorded exactly as typed — it is machine-independent
already, and it is the spelling your terminal can open.

### How long run records live

Each run writes one directory under `runs-dir` holding its artifacts, its
`events.jsonl` and its `run.log`. `[run] keep-runs` bounds how many are kept:
the default of 200 suits an archive, and a suite re-run on every save wants far
fewer. The artifacts are the largest part and are byte-identical between runs of
an unchanged suite — but they are *what that run executed*, and once the corpus
moves on `proef artifacts` no longer reproduces them, so the cost is bounded
rather than dropped.

Rotation only ever deletes directories named by a **generated** run id, because
`runs-dir` may be `.` or otherwise shared with your own files. A record written
under `--run-id <name>` is therefore never rotated: if CI mints a fresh name per
build into a persistent directory, prune it yourself.

A starter file ships as `proef.toml.example` in the repository root.
