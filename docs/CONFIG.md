# `proef.toml` — the project configuration reference

`proef.toml` lives in the project root (the directory you run `proef` from)
and is **committed project config** — it describes the suite, not your
machine. Every key is optional; absent file means all defaults. Unknown keys
are rejected (`deny_unknown_fields`), so typos fail loudly instead of being
ignored.

Precedence: built-in defaults < `proef.toml` < command-line flags.

## Reference

```toml
[run]
jobs = 8               # parallel scenario workers
runs-dir = ".proef-runs"   # where run records land

[http]
timeout-ms = 30000     # per-request timeout (batch-level default)
follow-location = false    # follow redirects
```

| Key | Default | Notes |
|---|---|---|
| `[run] jobs` | available parallelism | `--jobs` flag wins; live threads never exceed the scenario count |
| `[run] runs-dir` | `.proef-runs` | run records rotate here (newest 200 kept; only uuid-named run dirs are ever touched) |
| `[http] timeout-ms` | `30000` | per-entry `[Options]` in a hurl block override it |
| `[http] follow-location` | `false` | per-entry `[Options]` override it |

## What does *not* live here

- **Secrets** — `proef secret set` / `PROEF_SECRET_<NAME>` env
  (see [AUTHORING — Secrets](AUTHORING.md#secrets)).
- **Suite variables** (target URLs, tenant names, …) — `# key: value`
  feature directives with `${env:NAME:-default}`.
- **Machine-specific paths** — nothing in `proef.toml` should differ
  between teammates; that is what environment variables are for.

A starter file ships as `proef.toml.example` in the repository root.
