# `proef.toml` — the project configuration reference

`proef.toml` lives in the project root (the directory you run `proef` from)
and is **committed project config** — it describes the suite, not your
machine. Every key is optional; an absent file means all defaults. Unknown keys
are rejected (`deny_unknown_fields`), so typos fail loudly instead of being
ignored.

The file holds two kinds of thing, kept in distinct sections: **runner config**
(how proef behaves — `[run]`, `[http]`) and **suite variables** (data your tests
reference — `[url]`, `[vars]`), plus per-environment overrides (`[env.<name>]`).
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
jobs     = 8                # parallel scenario workers
runs-dir = ".proef-runs"    # where run records land

[http]
timeout-ms      = 30000     # per-request timeout (batch-level default)
follow-location = false     # follow redirects

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
| `[run] runs-dir` | `.proef-runs` | run records rotate here (newest 200 kept; only uuid-named run dirs are ever touched) |
| `[http] timeout-ms` | `30000` | per-entry `[Options]` in a hurl block override it |
| `[http] follow-location` | `false` | per-entry `[Options]` override it |
| `[url] <key>` | *(none)* | URL variables, referenced as `${url:<key>}` |
| `[vars] <key>` | *(none)* | non-secret variables, referenced as `${vars:<key>}` |
| `[env.<name>.<section>]` | inherits base | per-environment override of any base section (`url`/`vars`/`http`/`run`) |

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
`http.*` / `run.*` override runner settings. A named-but-undefined `--env` is a user
error (exit 2) listing the known environments. A `${url:key}` / `${vars:key}` referenced
in a pack but defined in neither the base nor the active environment fails at lower time
(`proef::resolve::missing_config_var`).

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

A starter file ships as `proef.toml.example` in the repository root.
