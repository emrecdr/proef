# proef in CI

Everything proef's differentiators buy — typed exit codes, JUnit, sharding,
the rerun overlay, the regression gate — pays off in CI, and each piece is
documented on its own page. This page is the missing last mile: one
paste-ready workflow, then the pieces it composes.

## A complete GitHub Actions workflow

```yaml
name: e2e
on: [push, pull_request]

jobs:
  e2e:
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        shard: [1, 2, 3]
    steps:
      - uses: actions/checkout@v4
      - name: Install proef
        run: |
          curl -LsSf https://github.com/cargo-bins/cargo-binstall/releases/latest/download/cargo-binstall-x86_64-unknown-linux-musl.tgz | tar -xz -C /usr/local/bin
          cargo-binstall proef --no-confirm
      - name: Run the suite
        env:
          # Secrets reach proef only through the environment — never files,
          # never flags (values would land in the process listing).
          PROEF_SECRET_APITOKEN: ${{ secrets.API_TOKEN }}
        run: |
          proef test tests/features \
            --env staging \
            --shard ${{ matrix.shard }}/3 \
            --junit auto \
            --meta commit=${{ github.sha }}
```

The pieces:

- **`--junit auto`** writes `report.junit.xml` into the run dir — under
  `GITHUB_ACTIONS` only, so local runs stay clean. Failures also reach the
  job summary and the PR's changed-files gutter as `::error` annotations
  automatically; no extra step.
- **`--shard I/N`** partitions by a stable hash of `(file, scenario)`:
  adding a scenario never re-buckets the others, so shard timings stay
  comparable across commits. Every matrix job runs the same expression and
  the shards partition exactly.
- **`--meta commit=…`** records provenance the run cannot harvest itself:
  proef never reads `GITHUB_SHA` or any CI variable (ADR-0020) — what the
  workflow hands over explicitly is what the record carries.
- **`PROEF_SECRET_<NAME>`** supplies `${secret:name}` values. They never
  appear in artifacts, events, logs, or reports — including base64/hex/
  percent-encoded reflections.

## Gating on regressions between runs

`proef diff` compares two run records; `--fail-on-regression` makes it a
gate. Download the base branch's record (uploaded as an artifact by its own
run) and compare:

```bash
proef test tests/features --junit auto        # today's run
proef diff path/to/base-events.jsonl          # vs the downloaded baseline
# exit 1 on a regression; new flakiness and perf deltas print either way
```

## Continuing a cancelled run

A run stopped by `--max-fail` or a runner timeout records what it never
reached. `proef test --rerun` re-runs the last run's failures *and* the
scenarios it never got to — and its JUnit and HTML report cover the whole
suite via the rerun overlay (`rerun_of` in the record), so one report stands
for the composed result, never a false green.

## Flakiness over history

Keep a few records (`[run] keep-runs` in `proef.toml` sets the rotation) and
`proef flaky` renders verdicts over them — flapping, passes-only-on-retry,
always-failing — from the same records CI already produced.

It also audits `@quarantine` itself, which nothing else can: a quarantined
scenario failing every run is `DISABLED` (switched off — its failures gate
nothing, so no job ever reports them), and one green throughout is `recovered`
(the tag can come off). `--format json` carries `quarantined` and the verdict
key, so a scheduled job can gate on either without parsing the table.

`--by <key>` splits the same history per run context — `--by env`, or any
`[meta]`/`--meta` key such as `--by runner`. A scenario that flaps in one
environment and is solid in another is not flaky but *context-dependent*, and a
pooled history cannot tell those apart: it reports the one conclusion the
merged view can never reach, naming the scenarios whose verdict changes with
where they ran. A run that never set the key is its own `(unset)` bucket rather
than being folded in with the runs that did.
