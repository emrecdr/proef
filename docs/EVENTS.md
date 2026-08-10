# Event schema — the run record for machine consumers

`.proef-runs/<run-id>/events.jsonl` **is** the run record (ADR-0008): one JSON
object per line, in emission order, no second format. Everything else —
console tree, `--junit`, the GitHub summary, `proef explain` — derives from
this stream. Consume it with `jq`, a log shipper, or anything line-oriented.

Wire shape: serde-tagged with `"event"`, `snake_case` names. The first line is
always `run_started` and declares `schema` (currently `1`); the last is
`run_finished`. The schema is **additive-only**: new variants and new
optional fields may appear, existing fields never change meaning or vanish.
Consumers must ignore unknown variants and unknown fields.

**Truncated records.** A record whose last line is *not* `run_finished` is a
run that did not finish — a hard interrupt (second Ctrl-C, exit 130), a kill,
or a crash. Treat it as **partial**: the scenarios present did happen, but the
totals were never written and no verdict was reached. `explain`, `report` and
`diff` classify these and say "run incomplete" rather than reporting a total
they cannot know. Consumers should do the same rather than inferring zero.

**One exception to "fields never change meaning", recorded rather than hidden.**
`run_finished`'s `passed`/`failed`/`skipped` counted *every* phase before
0.6.0; from 0.6.0 they are the **main-suite verdict** and exclude
`[run] setup`/`teardown` (ADR-0014), so those totals agree with the exit code
and every report. A pre-0.6.0 record read by a current consumer reports the
older meaning.

## Variants

**`run_started`** — head of every stream.
`schema` (u32) · `run_id` (string, uuid-v7).

**`scenario_started`** — `scenario` (string) · `file` (string) · `timestamp_ms`
(u64, run-relative ms — **only present** with injected timing) · `worker` (u64,
0-based worker index — **only present** with injected timing). The timing pair is
stamped at the CLI sink on the worker thread (the sans-IO core leaves it absent),
and powers the HTML report timeline (ADR-0015).

**`batch_started`** — a contiguous same-engine step batch was dispatched.
`scenario` · `engine` (e.g. `"hurl"`) · `steps` (count).

**`entry_running`** — live progress: one event per execution *attempt* of an
artifact entry, retries included. `scenario` · `engine` · `entry` (0-based
ordinal within the scenario's artifact) · `retry` (0 = first attempt).

**`step_finished`** — `scenario` · `engine` · `step` (`{file, line, text}`,
the authored feature anchor) · `status` (`passed | failed | skipped |
warned`) · `attempts` (u32) · `duration_ms` (u64) · `captures` (capture
*names* only — never values) · `detail` (string, **only present** on
failures/warnings/skips-with-reason) · `attempt_details` (array of strings —
the messages from earlier, failed attempts of a step that ultimately passed;
**only present** for a flaky pass, feeds JUnit `<flakyFailure>`).

**`scenario_finished`** — `scenario` · `file` (feature path — with `scenario`,
the run-wide identity: names are unique only within one file; absent in
records that predate the field) · `status` · `timestamp_ms` (u64, run-relative
end ms — **only present** with injected timing; carries no `worker`, since it is
emitted from the main dispatcher thread, not the scenario's worker — ADR-0015).

**`run_finished`** — tail. `passed` · `failed` · `skipped` — the **main-suite
verdict**: scenario counts for the primary suite only (ADR-0014). `[run]
setup`/`teardown` scenarios still appear as their own `scenario_started`/
`scenario_finished` events earlier in the stream, but are excluded from these
totals, so they agree with the console `summary:` line, `proef explain`,
`proef report`'s HTML headline, `--output json`, JUnit, TAP, the SLA gate,
and the exit code · `cancelled` (bool, **only present when true**).

## Example stream

```json
{"event":"run_started","schema":1,"run_id":"019f…"}
{"event":"scenario_started","scenario":"reference","file":"suite/case.feature"}
{"event":"batch_started","scenario":"reference","engine":"hurl","steps":2}
{"event":"entry_running","scenario":"reference","engine":"hurl","entry":0,"retry":0}
{"event":"step_finished","scenario":"reference","engine":"hurl","step":{"file":"suite/case.feature","line":4,"text":"the cookie session is exercised"},"status":"passed","attempts":1,"duration_ms":12,"captures":[]}
{"event":"step_finished","scenario":"reference","engine":"hurl","step":{"file":"suite/case.feature","line":5,"text":"the response status is 200"},"status":"passed","attempts":1,"duration_ms":0,"captures":[]}
{"event":"scenario_finished","scenario":"reference","file":"suite/case.feature","status":"passed"}
{"event":"run_finished","passed":1,"failed":0,"skipped":0}
```

## Guarantees

- **Secrets never appear.** Redaction applies once at the sink boundary
  before any reporter (property-tested); `captures` carries names only.
- **Order:** events of one scenario are internally ordered; scenarios
  running in parallel interleave. Group by `(scenario, file)` before assuming
  sequence across the stream — `scenario` alone collides when two feature
  files reuse a name.
- **Flake-safe assertions:** assert on `attempts` counts and normalized
  event order, never wall-clock (`duration_ms` is engine-measured and
  varies).

## Recipes

```bash
jq -r 'select(.event=="step_finished" and .status=="failed") | "\(.step.file):\(.step.line) \(.detail)"' events.jsonl
jq -r 'select(.event=="run_finished")' events.jsonl          # the suite verdict (setup/teardown excluded)
jq -r 'select(.event=="entry_running") | .retry' events.jsonl | sort | uniq -c   # retry pressure
```
