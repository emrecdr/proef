# ADR-0020 — Run metadata is explicit-injection-only

## Status

Accepted (2026-08-24). The RF-audit wave-2 companion to ADR-0019; codifies
the boundary R12-1 drew and the JUnit provenance decisions applied.

## Context

A proef record could not say which commit, build, or environment produced it
— `run_started` carried `schema` and `run_id` alone. Robot Framework's
`--metadata name:value` fills this in reports, and CI post-mortems genuinely
need it: `diff` across records from different commits or environments has no
context for what changed.

The hazard is on the other side. R12-1 removed *harvested* machine identity
from every artifact because absolute paths broke the two-checkouts
byte-equality ADR-0010 guarantees, and the JUnit sink deliberately omits
`timestamp`/`hostname` for the same reason. Metadata must not reopen that
door.

## Decision

1. **The axis is harvested vs. handed-over, not automatic vs. manual.**
   proef never reads git, the hostname, wall-clock provenance, or CI
   environment variables (`GITHUB_SHA`, `CI_COMMIT_SHA`, …). If the user
   wants the SHA recorded, their shell harvests it: `--meta
   commit=$(git rev-parse HEAD)`. What the user explicitly hands over,
   proef records verbatim.
2. **One precedence chain, three scopes**: `[meta]` < `[env.<name>.meta]` <
   `--meta k=v` — the same base < env < flags shape as `jobs` and
   `[url]`/`[vars]`. A duplicate key *among the flags* is exit 2 (loud over
   last-wins); a flag overriding a config key is the designed use. There is
   no `PROEF_META_*` — the values that motivate env vars are already in the
   shell where the flag is typed.
3. **The active `--env` profile name is recorded automatically, as its own
   field** (`run_started.env`). It is user-chosen input to the invocation,
   not an observed machine fact — and without it the record is
   uninterpretable: the same suite deep-merges different `[url]`/`[vars]`
   per profile, so `diff` warns loudly on a cross-env comparison.
4. **`shuffled` rides the same head**: with the permutation seeded by
   `run_id`, the bool plus the id reproduces an order exactly (deferred out
   of `--shuffle`'s own change so `run_started` moved once, not twice).
5. **Metadata reaches the record, `explain`, `diff`, the HTML report, the
   GitHub summary and the `--format json` body — and nothing else.** Never
   artifacts (`.hurl` bytes stay identical across checkouts and commits —
   ADR-0010, R12-2); not TAP (no slot a consumer reads); not JUnit
   `<properties>` (GitLab ignores them, Jenkins reads them only behind a
   non-default opt-in — same named-consumer method as R3-6, additive later
   if a consumer asks); not the console (the record and `explain` own it).
6. **Everything passes the sink-boundary mask** — keys and values both: a
   secret-bearing URL pasted into either position must not survive into the
   record or the body. The known limit stands recorded: a token proef was
   never told is a secret matches no needle, the same standing as any CLI
   argument.

## Consequences

- `run_started` gains `env`, `metadata`, `shuffled` — additive,
  skip-serialized when unset, `EVENT_SCHEMA_VERSION` stays 1 (ADR-0008
  erratum extended). The empty case is byte-identical to every existing
  record.
- Library-breaking: `ProjectConfig`/`EnvProfile` gain `meta`,
  `exec::execute` takes the merged map, `RunRecord::open` takes the head
  trio; clean break per policy.
- proef stays sans-IO in core: the CLI merges and injects; core never reads
  an environment.
