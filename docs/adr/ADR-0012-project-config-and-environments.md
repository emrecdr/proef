# ADR-0012 — Project configuration & environments in `proef.toml`

**Status:** Accepted · **Date:** 2026-07-30

## Context

Test files must stay pure prose — no URLs, no environment data, no variable
definitions (operator requirement: "test files for testing, not for variable
definitions"). Before this, the only non-secret variable source was the in-feature
`# baseURL:` directive plus `${env:…}`, which put configuration *inside* the
`.feature` files. Suites also had to be given an explicit path on every
`proef test` invocation.

## Decision

`proef.toml` (already the config file — ADR-0004 kept TOML for it) gains
variable-bearing sections and per-environment override profiles, modeled on the
Cloudflare-Wrangler `wrangler.toml` `[env.<name>]` pattern (and Cargo `[profile.*]`):

- `[url]` and `[vars]` — non-secret variables, referenced in packs as `${url:<key>}`
  and `${vars:<key>}` (new lower-time resolver namespaces, within the ADR-0005 `${…}`
  tier).
- `[env.<name>.<section>]` — per-environment overrides that **deep-merge** over the
  base tables, key by key: `[env.prod.url]` / `.vars` override variables,
  `.http` / `.run` override runner settings. Unlisted keys inherit the base.
- `--env <name>` / `PROEF_ENV` selects the active environment.
- `[run] suite` — the default suite path, so `proef test` needs no path argument;
  the `tests/` directory is the zero-config fallback convention.

Core stays sans-IO: the CLI loads `proef.toml`, validates the `--env` name,
deep-merges, and injects the resolved scope as `LowerCtx::config_vars` (keyed
`"<namespace>:<key>"`). The resolver reads that injected map — it never touches a
file. Secrets remain on their own encrypted channel (`${secret:…}`) and never
appear in `proef.toml`.

## Consequences

- Feature files become pure prose; URLs / credentials / env data live in one
  external file — the single variable-definition mechanism (the legacy `# key:`
  directive was later removed; see the 2026-07-31 amendment).
- A referenced-but-undefined `${url:…}` / `${vars:…}` is a user error at lower time
  (`proef::resolve::missing_config_var`) — the same strictness as `${env:…}`.
- Deep-merge (not Wrangler's non-inheritable `vars`) means an environment lists only
  deltas; this is the deliberate divergence from Wrangler's known footgun.
- `proef test` / `flows` / `artifacts` accept an optional path plus a `--env` flag.

## Alternatives considered

- **Per-environment files** (`proef.<env>.toml`, Spring / dotenv style) — cleaner git
  diffs per env, but more files; the single-file `[env.<name>]` model keeps one mental
  model and reuses the existing loader. Recorded as the fallback if one env grows large.
- **Wrangler-exact non-inheritable vars** — rejected: forces re-listing every var per
  env (the documented Wrangler footgun); deep-merge is the more ergonomic default.
- **A `${baseURL}` magic bare name** — rejected: namespaced `${url:base}` is
  collision-free and consistent with `${secret:…}` / `${env:…}` (one way to do one thing).

## Amendment (2026-07-31) — the `# key:` directive mechanism is removed

The original decision kept the in-feature `# key: value` directive (e.g.
`# baseURL:`) working "for one-off per-file overrides." That left **two** ways to
define a variable — a directive *inside* a `.feature` file, and `[url]`/`[vars]`
in `proef.toml` — violating one-way-to-do-one-thing (operator: "variables should
be defined in 1 way, not multiple ways"). The directive mechanism is therefore
**removed**:

- `FeatureFile::directives`, `collect_directives`, `lower::resolve_directives`, and
  the `ResolveCtx::directives` scope are deleted. `${…}` plain-name resolution is now
  `args > defaults` only; feature files carry no variable definitions.
- Config becomes the **single** variable source. Config values may themselves embed
  `${env:NAME:-default}` (resolved recursively), so the env-override + default that
  `# baseURL: ${env:PROEF_BASE_URL:-…}` provided is preserved as
  `base = "${env:PROEF_BASE_URL:-…}"` under `[url]`.
- `proef.toml` is now discovered by walking **up** from the working directory (like
  cargo/git), so config is found from any subdirectory (needed once the value moved
  out of the self-contained feature file — e.g. the libtest-mimic harness invokes
  `proef` from its own crate dir).

`#` comment lines before `Feature:` remain valid gherkin comments; they are simply no
longer parsed as directives.
