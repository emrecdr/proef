# Authoring reference — packs and features from the author's seat

Everything here is validated at load or at `--dry-run` time; nothing fails
only at execution that could have failed earlier. Start with
[GETTING-STARTED](GETTING-STARTED.md) if this is your first suite.

## Feature files

Standard Gherkin: `Feature:`, `Scenario:`, `Background:` (prepended to every
scenario), `Rule:`, `Scenario Outline:` + `Examples:` (expanded, `#N`-deduped),
data tables (rows become step arguments), and docstrings (delivered to the
macro as the `docstring` param). Keywords (`Given/When/Then/And`) don't affect
binding — only the sentence text does.

**Directives** are `# key: value` comment lines before `Feature:`. They resolve
`${env:NAME:-default}` / `${run:id}` and become `${key}` in every step of the
file. Convention: `# baseURL:` for the target host.

**Tags** (`@smoke`) accumulate feature→scenario; `proef test --tags a,b`
selects scenarios carrying any of them (a selection matching nothing is an
error, not a silent green run).

## Macros (`templates:` in a pack)

```yaml
templates:
  name:
    match: the client {name} is resolved   # sentence pattern (optional)
    params: [name]                         # declared parameters
    defaults: { index: clients }           # defaults for optional params
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

## Steps

```yaml
steps:
  - name: human label (${…} resolves here too)
    optional: true                    # failure warns instead of failing
    retry: { count: 10, interval_ms: 300 }   # finite; 1..=10000
    delay: 250                        # ms before the request (capped at 1 hour)
    when: "${env:RUN_SLOW:-}"         # skips when empty or false/0 after resolution
    saveAs: { clientId: global }      # promote a capture to the global store
    hurl: |                           # the payload — raw hurl, one or more entries
      GET ${baseURL}/api/v1/clients/{{clientId}}
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

## Variables — the two tiers

| Syntax | Resolves | When | Examples |
|---|---|---|---|
| `${…}` | proef | at lowering (before execution) | `${param}`, `${env:NAME:-default}`, `${run:id}`, `${global:key}`, `${secret:NAME}`, `${fake:name}` |
| `{{…}}` | hurl | at run time | captures (`{{clientId}}`), secrets (`{{apiToken}}`) |

`${…}` is recursive (captured arguments may themselves contain `${…}`, depth
≤ 8) and `$${` escapes a literal `${`. `${secret:NAME}` never inlines the
value — it lowers to `{{NAME}}` and the engine injects it through hurl's
redaction, so artifacts carry placeholders only.

**Captures** (`[Captures]` in a hurl block) flow forward within the scenario
as `{{name}}`. `saveAs: { name: global }` additionally promotes the captured
value into the persistent global store (`.proef-state.json`), where later
scenarios and later runs read it at lowering time as `${global:name}`.

**Fakes** are deterministic synthetic data seeded by the run id — stable
within a run, fresh across runs: `firstName`, `lastName`, `name`, `fullName`,
`email`, `username`, `phoneNL`, `postCode`, `city`, `street`, `int`, `number`,
`digits4`, `digits8`, `bool`, `word`, `uuid` (unknown generators fail at load).

## Secrets

Reference with `${secret:NAME}`. Values come from `PROEF_SECRET_<NAME>`
environment variables first, then the encrypted store (`proef secret set NAME`,
or `proef secret set NAME --value V` for scripts; `proef secret list` names,
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
- `--output json` prints one machine-readable summary object on stdout (the
  human report moves to stderr); `--jobs N` controls parallelism;
  `proef.toml` holds project defaults (`[run] jobs`, `[http] timeout-ms`).
- `proef fmt suite --check` keeps hurl blocks canonically formatted;
  `proef flows` lists scenarios; the nextest harness
  (`PROEF_HARNESS_SUITE=<dir> cargo nextest run -p proef-harness`) exposes
  one test per scenario to IDEs.

## Environment variables

| Variable | Read by | Purpose |
|---|---|---|
| `PROEF_SECRET_<NAME>` | `proef test` | Secret value override (beats the encrypted store) |
| `PROEF_CONFIG_DIR` | `proef secret` | Key-file location (default: XDG config dir) |
| `PROEF_HARNESS_SUITE` | nextest harness | Suite directory the harness lists and runs |
| `PROEF_BIN` | nextest harness | Path to the `proef` binary the harness invokes |

Suite-defined variables (like `PROEF_BASE_URL` in the guides) are a
convention of `${env:…}` directives, not built-ins — name yours freely.

## Style

Keep prose at business level — no URLs, headers, or JSON in feature files;
that's what packs are for. Prefer several small macros composed with `use:`
over one large one. Give steps `name:` labels — they anchor artifacts,
events, and failure output.
