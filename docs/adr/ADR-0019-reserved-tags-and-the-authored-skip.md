# ADR-0019 — Reserved tags and the authored skip

**Status:** Accepted · **Date:** 2026-08-24

Emerged from the Robot Framework capability audit (OPEN-FINDINGS, "RF wave 2");
every design fact below was verified against the tree or reproduced empirically
before acceptance.

## Context

proef had no way to park a scenario. A test that must not run — mid-migration,
a known-broken dependency, a seasonal flow — could only be deleted or dodged
with `--tags`, and both are invisible: nothing in any report says "this exists
and was deliberately not run". Robot Framework's SKIP model (its 4.0 headline
design, replacing criticality) is the industry convergence point: skip is a
first-class visible status with a reason that survives into every report.

Mechanically, proef already *had* a scenario-level `Skipped` status — but it
arose only from cancellation, its reason existed nowhere, and two consumers
had baked "Skipped means never-ran" into their logic (`--rerun` re-queues
Skipped-on-cancelled; `diff` reads Failed→Skipped as **fixed** — and
`--fail-on-regression` certified it). An authored skip that ignored those two
would have shipped a laundering bug, not a feature.

## Decision

1. **A reserved tag namespace, recognized at the CLI edge.** `@quarantine`
   and `@skip` are the reserved tags; recognition lives in exactly one place
   (`front::reserved`), and core never reads tags — the front computes an
   instruction (`ScenarioSpec.skip`, like `exclusive` before it) per
   ADR-0014's split. Reserved tags in `[run] setup`/`teardown` features have
   **no effect**: phases never pass through `build_specs`, and skipping your
   whole setup deliberately is spelled by deleting the config key.
2. **The spelling is `@skip` or `@skip:<reason-token>`.** The gherkin grammar
   accepts `skip:migration-pending` as one tag (verified empirically through
   the real pipeline). The recorded reason is the pasteable tag spelling
   itself — `"@skip"` / `"@skip:migration-pending"` — the same philosophy as
   the fragment field's `file.hurl#name`.
3. **Authored reasons start with `@`; mechanical reasons never do.** That is
   the contract `--rerun` keys on: `Skipped ∧ cancelled ∧ reason not
   authored` re-queues as never-ran; an authored skip never re-queues.
   Pre-field records (no reason) read as mechanical, which they were.
4. **No tag-list normalization.** An earlier draft injected a canonical
   `skip` atom beside `@skip:x` so `--tags "not @skip"` excluded both. Tag
   globs shipped first, and `not @skip*` says the same thing without proef
   ever rewriting an authored tag list. Authored tags stay exactly authored.
5. **A skipped scenario is selected, counted, and reasoned in every sink**:
   console (`∅ … — @skip:x`), JUnit (`<skipped message>`), TAP
   (`# SKIP @skip:x`), the record (`ScenarioFinished.reason`, additive,
   schema stays 1), the HTML report, `explain`, `flows --format json`
   (`"skip"`), and the harness (libtest's ignored flag). `--tags` remains
   the *unselection* mechanism — the two semantics stay distinct, as in RF.
6. **All-selected-scenarios-skipped exits 0.** Exit 2 is for faulty *input*;
   the empty-selection refusal exists for the typo'd filter whose silent
   green run nobody sees. An all-skipped run is neither silent (every
   surface prints the totals and reasons) nor accidental (each skip is
   authored, versioned, and visible in review). RF and pytest agree; pytest
   reserves its special code for empty *collection*, which is exactly the
   case that stays exit 2 here.
7. **`diff` gives skip transitions their own bucket.** Into-Skipped is
   neither fixed nor regressed (`now skipped (was failing/passing)`);
   out-of-Skipped has no meaningful baseline and takes the `added` shape.
8. **A quarantined test-failure reaches JUnit as skipped-with-message.** The
   exit code already said "non-gating"; the XML said `<failure>`, so Jenkins
   marked UNSTABLE and every dashboard contradicted the verdict. RF converts
   the status for the same reason. User/System faults stay failures —
   quarantine is for flaky tests, not broken input.
9. **`--dry-run` still validates skipped scenarios.** Skip is an
   execution-time decision, not a validation waiver — a broken-but-skipped
   scenario still fails `--dry-run`, deliberately.

## Consequences

- Library-breaking (clean break, no shims): `ScenarioSpec.skip`,
  `ScenarioOutcome.reason`, `Event::ScenarioFinished.reason`,
  `ScenarioRun.reason`, `write_junit`/`write_ci_reports` gain the
  non-gating list. Wire-additive; `EVENT_SCHEMA_VERSION` stays 1.
- The sink wrappers that rebuild scenario events field-by-field
  (`stamp_scenario_timing`, `phase_sink`) must thread every new field —
  the exhaustive constructions turn forgetting into a compile error, and
  the e2e test pins the stamped stream.
- A related stance this ADR writes down because the audit found it held but
  unwritten: **control flow lives in packs** (`when:` conditional skip at
  step level, `optional:` soft-fail, finite `retry:`) — prose stays
  declarative; there is no scenario-level IF/WHILE/TRY and none is planned.
