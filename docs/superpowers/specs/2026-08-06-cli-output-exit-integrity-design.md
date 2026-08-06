# Design: CLI output & exit integrity (Tier 1, branch 2 of 4)

**Date:** 2026-08-06
**Status:** Approved
**Source:** external v0.5.3 review, validated finding-by-finding. Evidence: `.superpowers/sdd/validation/v053-validation.md`.
**Depends on:** PR #13 merging. Cut the work branch fresh off `main` afterwards.

## Problem

Six validated findings share one shape: **proef reports success while producing
wrong output.** Each was reproduced against the current tree, not inferred.

The governing contract is ADR-0009 — exit codes `0` ok · `1` test failure ·
`2` user error · `3` system error. Two of these findings break it directly by
exiting `0` on a failure; the rest corrupt output that a `0` exit implies is
sound.

## Verified facts (do not re-derive)

| Fact | Citation |
| --- | --- |
| `std::env::var()` returns `Err(NotUnicode)` for non-UTF-8; `.ok()` / `if let Ok(…)` collapses that to "absent" | `crates/proef-cli/src/main.rs:267`, `secretstore.rs:116`, `:284`, `:403` |
| The comment above `key_from_env` states a set-but-invalid key must always error | `crates/proef-cli/src/secretstore.rs:95-97` |
| `outln!` reports a non-`BrokenPipe` stdout failure to stderr and returns; nothing reaches the exit code | `crates/proef-cli/src/render.rs:13-22` |
| A "failure folds into the exit" precedent already exists | `crates/proef-cli/src/exec.rs:390`, `:405`, `:500` (`junit_failed`) |
| Tee re-writes the full slice on every `write_all` retry | `crates/proef-cli/src/exec.rs:816-822` |
| `proef fmt` rewrites CRLF wholesale, beyond its hurl-blocks-only promise | `crates/proef-cli/src/fmt.rs:79-130` |
| `report -o` outside the run dir bakes a relative artifacts href | reproduced live |
| `diff` flags a brand-new retried step as flaky via a `map_or(1, …)` default | reproduced live |

## Decisions

### D1 — A malformed environment variable is an error, never silence

All four sites move from `env::var(…).ok()` to explicit handling that
distinguishes **absent** from **present-but-unreadable**. Non-UTF-8 (or
otherwise invalid) values become a user error (exit `2`) naming the variable.

`PROEF_KEY` is the sharpest case and the code already knows it: the comment at
`secretstore.rs:95-97` says a set-but-invalid key must always error, because
falling through to the key file decrypts with the wrong key and reports
*tampering* instead of the real cause. The code contradicts its own comment.

**One rule for all four**, not a special case for the key. `PROEF_ENV` silently
ignored means running against the wrong environment; `PROEF_SECRET_<NAME>`
silently ignored means a missing-secret error that names the wrong cause. A
variable the user set and proef could not read is always worth saying out loud.

### D2 — A failed stdout write reaches the exit code

`outln!` currently notes a non-`BrokenPipe` stdout failure on stderr and
continues, so `proef … --output json > /full/disk` exits **0** with truncated
or zero-byte JSON. A consumer parsing that output has no signal.

Fold a "stdout write failed" flag into the final exit exactly as `junit_failed`
already is (`exec.rs:390` → `:405` → `:500`) — one mechanism, already proven in
this codebase. The failure maps to `ExitCode::SystemError` (3): the environment
failed, not the user's input and not the test.

`BrokenPipe` stays swallowed. `proef … | head` must still exit cleanly; that
behavior is deliberate and tested.

### D3 — Tee mirrors only the bytes the console accepted

`write_all` loops on short writes. Tee hands the **full** slice to the file on
every iteration, so a short console write causes the tail to be written twice —
`run.log` gains duplicated fragments.

Mirror only the accepted prefix. The file is a faithful copy of what the console
received, which is what a mirror means.

### D4 — `fmt` preserves the file's dominant line ending

`proef fmt` promises to normalize *the raw hurl blocks inside macro packs*. It
rewrites the whole file's line endings, so on an `autocrlf` checkout
`fmt --check` is permanently red through no fault of the author.

Detect the file's dominant ending and preserve it. This is the fix that matches
the promise; the alternative — documenting the normalization — would be
codifying a behavior nobody asked for that breaks a supported checkout style.

### D5 — `report -o` writes hrefs that resolve from the output file

`-o /tmp/out/report.html` bakes `.proef-runs/<id>/artifacts/…`, which a browser
resolves relative to the HTML's own directory. Nothing is there; every artifact
link 404s while the command reports success.

Compute the href relative to the **output file's parent**. Keep the bare
`artifacts` form for the common case where the report sits in the run dir —
compare canonicalized paths so a `./` or symlink spelling does not defeat it.

### D6 — `diff` stops inventing flakiness; the ordinal caveat is documented

Two distinct problems, and they get different treatments.

**A brand-new retried step flagged "flaky" is a bug.** The comparison uses a
`map_or(1, …)` default, so a step absent from the base run is treated as having
had one attempt, and any retry reads as new flakiness. A step with no baseline
has no flakiness to report — skip steps absent in base.

**The ordinal shift is inherent.** Removing an earlier duplicate shifts later
steps' `(text, ordinal)` keys, so a comparison can misattribute one step's
timing to another. That is a property of positional keying, which 0.5.2 chose
deliberately over text-only keying (which lost duplicates entirely). Fixing it
needs stable per-step identity that does not exist in the record today —
out of scope. **Document the caveat** in the diff output's own documentation so
a reader can recognise it, rather than silently shipping a subtly wrong number.

## What is NOT in this branch

- The four record/artifact findings (branch 1 — spec and plan already written).
- `--dry-run` phase validation and Ctrl-C teardown (branch 3). The latter needs
  an **ADR-0014 amendment**: the ADR gates teardown on setup-success and is
  silent on cancellation, so whether cleanup should run when the operator
  interrupts is unanswered policy, not a defect with an obvious fix.
- LSP rooting (branch 4).
- All Tier 2/3 findings.

## Adjacent observations (not scoped here)

A quality pass over the preceding branch surfaced two structural points that
belong with this subsystem rather than with the correctness fixes above.
Recorded so they are not rediscovered.

**The incompleteness banner is spliced into rendered HTML.**
`report.rs`'s `banner_incomplete` inserts the notice with
`html.replacen("<h1>", …, 1)`, because `render_html` has no banner parameter
and the preceding branch deliberately left `proef-core` untouched. The renderer
already knows the fact — it computes `run_finished: Option<…>` in its single
pass, and `None` *is* "incomplete". The renderer emitting its own banner would
remove both the string-matching and the duplicated wording between `explain`
and `report`. Two tests pin the banner's presence and absence, so markup drift
fails the suite rather than silently no-opping; this is a robustness
improvement, not a live defect.

**Phase membership is inferred, not recorded.** `explain` and the HTML renderer
each independently approximate "this failure was a setup/teardown fault" from
the suite-only `failed` count being zero. Neither the event schema nor `Record`
tags which phase a scenario belongs to. The heuristic is correct whenever a
phase failure occurs alone and degrades to an unlabelled failure when a suite
failure co-occurs — a known, accepted limitation. A phase tag computed once in
`record::parse_record` would let every consumer read it directly instead of a
third one reinventing the rule. Note this is additive to the record, not to the
event schema, so ADR-0008's additive-only constraint is not engaged.

## Breaking-change treatment

D1 and D2 turn silent successes into failures. A pipeline that today exits `0`
with a mis-set `PROEF_ENV`, or with truncated JSON on a full disk, will exit
non-zero after this. That is the point — but it belongs under `### Changed` in
the changelog with the consequence stated, not buried under `### Fixed`.

PR #13 already forces **0.6.0**; this rides that release.

D3–D6 change output only where it was previously wrong.

## Testing

| Decision | Test |
| --- | --- |
| D1 | each of the four variables, set to a non-UTF-8 value, produces a user error naming it — not silent fallback |
| D2 | a stdout write failure yields a non-zero exit; and `\| head` still exits cleanly (the `BrokenPipe` case must not regress) |
| D3 | a short console write leaves `run.log` byte-identical to the console stream, with no duplicated tail |
| D4 | a CRLF pack keeps CRLF after `fmt`, and `fmt --check` passes on it |
| D5 | `report -o` into a directory outside the run dir produces hrefs that resolve from that directory |
| D6 | a step present only in the new run is not flagged flaky, however many attempts it took |

Every test must fail without its change, demonstrated. D2's is the subtle one:
asserting "stderr mentions the failure" passes today. It must assert the **exit
code**.

## Constraints

- `proef-cli` only — `proef-core` is untouched by this branch.
- No new dependencies; hurl pins stay `=8.0.1`.
- Exit codes stay within the ADR-0009 taxonomy: `0` ok · `1` test failure ·
  `2` user error · `3` system error. D1 maps to `2`, D2 to `3`.
- No raw print macros in `proef-cli` or `proef-lsp` — `render::outln!` /
  `errln!` only; the guard is line-based and also trips inside comments.
- One canonical mechanism: D2 reuses the `junit_failed` fold rather than
  inventing a second exit-influencing path.
- No task ids, plan numbers, or review-section references in code comments.
- No AI-attribution commit trailers. No version bump.
