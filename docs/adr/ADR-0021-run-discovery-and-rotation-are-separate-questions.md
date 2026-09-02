# ADR-0021 — Run discovery and rotation are separate questions

**Status:** Accepted · **Date:** 2026-09-02

## Context

`--run-id` accepts any single path component, and `TROUBLESHOOTING.md`
demonstrates `--run-id pr`. A run named that way writes a complete, valid
record. It is then invisible to `proef explain`, `diff`, `flaky`, `report` and
`--rerun` whenever they resolve *the latest run*, because every one of them
enumerates through `record::all_runs`, which admits only 36-character uuid
names.

That predicate is not an oversight. `rotate_runs` consumes the same function
and says so:

> `record::all_runs` is the one answer to "what is a run record here" —
> sorted, uuid-named directories only. Rotation adds a single further
> exclusion (the in-flight run), not a second enumeration rule.

and `is_run_id` states the reason:

> rotation deletes the oldest run-shaped directories, so breadth here is a
> deletion hazard when the runs dir points somewhere shared.

Both are right. `runs-dir` may be `.`, so a broad predicate would let rotation
delete directories proef never created. That risk is real and the narrow
answer is the correct one — **for rotation**.

The defect is that one predicate serves two questions whose risks point in
opposite directions:

| Question | Unsafe when | Consequence |
|---|---|---|
| *May I delete this directory?* | too **broad** | destroys user data |
| *Is this a run I can show you?* | too **narrow** | hides a real record |

Sharing the predicate meant the deletion-safety choice silently became a
visibility choice. `CONFIG.md` documented the rotation consequence of a custom
id ("this will never delete it") and not the discovery one, so the surprising
half was the undocumented half.

## Decision

**Split the predicate along the risk, not along the file.**

1. **Rotation keeps the uuid rule** (`is_rotatable`, formerly `is_run_id`) —
   uuid-named directories only. Unchanged in behaviour, for the reason it was
   written; renamed for the question it answers. A custom-id record is still never deleted by
   `[run] keep-runs`, and that stays documented.

2. **Discovery admits any directory containing an `events.jsonl`.** A
   directory holding proef's own record file is a proef record; the test
   cannot mistake `target/` or `node_modules/` for one, and — decisively — it
   authorises no deletion. Reading a directory that turns out not to be a
   record fails as a parse error naming the file, which is already how a
   corrupt record behaves.

3. **Ordering stops relying on the name — but keeps relying on the uuid.**
   `all_runs` documented that "uuid-v7 names sort chronologically, so lexical
   order *is* time order", which a `pr` directory breaks. The fix takes the
   *timestamp* rather than the spelling: a uuid-v7 name carries 48 bits of
   unix milliseconds — the moment proef minted it — which is precisely why the
   lexical sort worked. Runs order by that where it exists, and by directory
   mtime where it does not (a custom `--run-id` carries no time). Both are
   wall-clock unix time, so the two sources compare directly.

   **Not read from the record**, though the first draft of this ADR said it
   would be. The head event carries `event`/`run_id`/`schema` and no timestamp
   at all — per-event times are injected observability on `scenario_started`
   (ADR-0015) — so "the record's own `run_started` timestamp" does not exist.
   Reading it returned `None` for every real record and silently ordered
   everything by mtime; the test that covered it passed only because its
   fixtures fabricated a field no record has. An ordering that depended on the
   suite having run at least one scenario would not be an ordering anyway.

## Consequences

- `proef explain`, `diff`, `flaky`, `report` and `--rerun` find custom-id runs.
  "Latest" means latest *in time* rather than latest *in the alphabet*, which
  is what every one of those commands already claimed to mean.
- Rotation's blast radius is unchanged — the one property that could have made
  this dangerous.
- Two predicates now exist where the code deliberately had one. That is the
  cost, and it is why this is an ADR rather than a patch: the earlier
  single-predicate statement was a considered position, and superseding it
  needs to be on the record. The two are named for their questions
  (`is_rotatable` / `holds_a_record`) so a future reader cannot reach for the
  wrong one by picking the shorter name.
- Ordering costs a `stat` per *custom-id* run directory and nothing at all for
  a uuid-named one, whose time is read straight out of its name. Bounded by
  `[run] keep-runs` (200 by default) and paid only by commands that resolve
  "latest".

## Alternatives considered

- **Leave it, document it.** The status quo before this ADR. Rejected: the
  invisibility surprises exactly the user who chose a memorable id *so they
  could find the run again*, and `--rerun` silently operating on a different
  run than the one just produced is the worst shape of it.
- **Make `--run-id` reject non-uuid names.** Honest, and it would remove the
  trap — but it also removes the feature's point. CI archiving
  `.proef-runs/pr-1234/` by a known path is the use case `--run-id` exists for.
- **Rotate custom-id directories too, and keep one predicate.** Rejected
  outright: it makes `runs-dir = "."` a data-loss configuration, which is the
  hazard `is_run_id` was written to prevent.
