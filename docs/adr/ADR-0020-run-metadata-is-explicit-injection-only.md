# ADR-0020 — Run metadata is explicit-injection-only

**Status:** Accepted · **Date:** 2026-08-24

The RF-audit wave-2 companion to ADR-0019; codifies the boundary R12-1 drew and
the JUnit provenance decisions applied.

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

## Amendment (2026-09-07) — a computed input fingerprint is not harvested metadata

`proef flaky`'s equivalence-class fingerprint (the `inputs.json` sidecar)
prompted the obvious question: does §1 forbid it? It does not, and the
boundary is worth stating so the next reader does not re-litigate it.

§1 forbids proef from **harvesting an environment fact** — reading git state,
the hostname, or CI variables and putting them in the record. The input
fingerprint reads none of those. It is a hash of proef's **own inputs** — the
feature sources, the loaded macros and fragments, the resolved config scope —
the same category as the artifact slug (`emit::artifact_slug`) or the shard
hash: a *derived identifier* over data proef already holds, not a fact lifted
from the surrounding machine. Derived identifiers have never been in scope
here; ADR-0020 governs `[meta]`/`--meta` **metadata**, which this is not.

The git-commit case remains exactly as §1 requires: a user who wants
commit-based grouping **hands the commit over** (`--meta commit=$(git rev-parse
HEAD)`, §1's own worked example) and `proef flaky --by commit` groups on it.
proef never runs git itself. So the survey's "git tree SHA equivalence class"
splits cleanly along this ADR's own axis — a computed fingerprint proef may
derive, plus a commit the user may hand over — and needs no new decision.

The sidecar is a derived aid like `timings.json`, not a second record
(ADR-0008): the JSONL event stream remains the only record format, and the
event schema is untouched (no `run_started` field was added).

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
