# Design: first-run UX pass

**Date:** 2026-08-06
**Status:** Approved
**Source:** `docs/FIRST-RUN-UX-REVIEW.md` (external, 2026-08-06), validated finding by finding — see that document's appended validation notes.
**Scope:** `proef-cli` and `proef-core` diagnostic text, plus docs. `proef-core` stays sans-IO.

## Problem

An experienced engineer evaluating proef for adoption reached a first green
`--dry-run` only by hand-authoring three files and consulting two documents.
Four gaps were reproduced against the shipped 0.5.3 binary:

1. No command scaffolds a project. All 13 subcommands assume a suite exists.
2. `resolve::missing_config_var` neither suggests a near miss nor seeds a
   corpus case, while two sibling codes in the same family do suggest.
3. The dry-run success path prints no next command, while every failure path
   names a remedy.
4. The README never shows a parameterized macro, and `schema --add-to` is
   effectively undiscoverable.

The first-use path is not tracked anywhere: searching `IMPROVEMENT-PLAN.md`
for onboarding / first-run / first-use / new-user / adoption returns one hit
(line 258), about `--force` for skipping a failing scenario — unrelated.

## Verified facts (do not re-derive)

Confirmed against `03b442f`.

| Fact | Citation |
| --- | --- |
| 13 subcommands; none scaffolds | `proef --help` |
| `ctx.config_vars` is keyed by the **full** `"namespace:key"` reference | `crates/proef-core/src/resolve.rs:350-358` |
| `MissingConfigVar` is built with `namespace`, `arg`, and `ctx` all in scope | `crates/proef-core/src/resolve.rs:352-368` |
| The `suggestion: Option<String>` + `— did you mean` pattern already exists | `resolve.rs:79-85` (`UnknownVariable`), `:126-131` (`FakeUnknown`) |
| `matcher::closest(input, candidates) -> Option<&str>` is public in core | `crates/proef-core/src/matcher.rs:316` |
| The dry-run success summary is printed in one place | `crates/proef-cli/src/commands.rs:197` |
| `schema(add_to: &[PathBuf]) -> ExitCode` installs the schema file **and** the modeline | `crates/proef-cli/src/commands.rs:437-459` |
| The tutorial's starting files are `proef.toml`, `suite/case.feature`, `suite/packs/api.yaml` | `docs/GETTING-STARTED.md §2-§3.5` |
| `DIAGNOSTICS.md` records "23 of the 59 codes carry a seeded corpus case" | `docs/DIAGNOSTICS.md:108` |
| `missing_config_var`'s corpus column is empty | `docs/DIAGNOSTICS.md:86` |
| A suite with absolute URLs and no `proef.toml` dry-runs green, 0 warnings | reproduced |
| `ResolveError` carries no position; `resolve()` is "Pure and total" | `resolve.rs:77-145`, `:163` |

## Decisions

### D1 — `proef init [dir]` writes the tutorial's files, never overwriting

Creates any of `proef.toml`, `suite/case.feature`, `suite/packs/api.yaml`
that are absent; skips any that exist; prints created-vs-skipped; exits 0.

**Never overwrites.** There is no `--force`. A flag whose only purpose is to
permit destroying an authored suite is not worth its risk, and refusing
outright would block the common case of adding a suite to a repo that already
has a `proef.toml`. Creating only what is missing makes the command
idempotent: a second run creates nothing and says so.

Any filesystem failure (cannot create a directory, cannot write a file) exits
`ExitCode::SystemError` (3), matching the precedent every other writing command
already sets — `report.rs:36`, `commands.rs:376`, `commands.rs:488`. `init`
takes no user input that can be wrong, so it has no exit-2 path.

### D2 — the template *is* the tutorial

The scaffold emits the files `docs/GETTING-STARTED.md` already teaches
(§2-§3.5), not a new shape. Two canonical starting points would be exactly the
drift the project forbids; the tutorial and the scaffold must agree by
construction. A test asserts the scaffolded suite dry-runs green, so the two
cannot silently diverge.

### D3 — `init` installs the pack schema by calling the existing function

After writing the pack, `init` calls `crate::commands::schema(&[pack_path])`
(`commands.rs:437`) — the same function `proef schema --add-to` runs. This is
reuse, not a second mechanism: there remains exactly one implementation of
"install the schema and the modeline". It is also how F4b lands — schema-backed
completion works on the first run without the user discovering a flag.

If that call returns a non-success `ExitCode`, `init` propagates it.

### D4 — `init` does not run its own dry-run

The review proposed `init` self-validate by running `--dry-run`. Instead a test
asserts the scaffolded project dry-runs green. The guarantee is identical but
enforced once in CI rather than recomputed on every user's machine, and it
avoids inventing an exit-code meaning for a failed self-check. `init` prints
the next command instead.

### D5 — did-you-mean on `missing_config_var` only

At `resolve.rs:352-368`, filter `ctx.config_vars` keys to those prefixed
`"<namespace>:"`, strip the prefix, and pass them to `matcher::closest()`.
Add `suggestion: Option<String>` to `MissingConfigVar` with the same
`#[error(...)]` formatting the two sibling variants already use. Scoping
candidates to the same namespace means a `url:` typo can never suggest a
`vars:` key.

**The review's "same treatment for `missing_env` and `unknown_namespace`" is
declined**, for reasons specific to each:

- `missing_env`'s candidate set is the injected environment snapshot —
  hundreds of names, and suggesting from it risks surfacing unrelated
  environment variable names in diagnostics, which cuts against the
  secret-masking posture. Sibling codes share a *shape*, not a *candidate set*.
- `unknown_namespace` already enumerates all seven valid namespaces in its
  message; a suggestion adds nothing.

Also seeds `tests/errors/resolve__missing_config_var/`, which requires ticking
`DIAGNOSTICS.md`'s corpus column for the code **and** updating its coverage
count from 23 to 24 — a number that drifts silently otherwise.

### D6 — dry-run success prints the next command; no `[url]` warning

One line after the summary at `commands.rs:197`, on success only.

**The review's second half is dropped.** It proposed warning when no `[url]`
key is configured, calling that "the guaranteed next failure". Reproduced and
disproved: a suite with absolute URLs and no `proef.toml` at all dry-runs green
with 0 warnings and runs fine, so the warning would fire on a valid suite. And
when a suite *does* reference an unconfigured `${url:key}`, dry-run already
fails with `missing_config_var` — so the warning is either a false alarm or
redundant.

### D7 — README gains a parameterized macro and a non-goals section

`params:` appears zero times in the README today, and the only `match:` shown
is a static sentence, so the placeholder syntax exists only in GETTING-STARTED
and AUTHORING. Add one parameterized macro showing `match:` with a
`{placeholder}` bound to `params:`.

Add a *"What proef deliberately isn't"* section naming the load-bearing
non-goals (no hurl import — artifacts flow outward only; no mocking or contract
testing; no second engine) plus one line telling existing-hurl users the
supported path is pasting bodies into pack steps. The review's own withdrawn
recommendation (W1, a `proef import`) is the evidence this is needed: an
informed reader who had read the README and GETTING-STARTED proposed a
documented permanent non-goal as a top-two recommendation, because the boundary
is invisible where newcomers look.

### D8 — PRD US-13; no ADR; review doc committed with validation notes

`init` gets a PRD user story (US-13, P1) because the PRD is the scope document
whose `US-N` anchors acceptance criteria. It gets **no ADR**: a new subcommand
is additive and non-breaking (`docs/RELEASING.md` versioning table) and decides
nothing architectural. ADR-0016 already draws the generator boundary, and a
fixed template that reads no spec falls outside it.

`docs/FIRST-RUN-UX-REVIEW.md` is committed verbatim — it is an external
reviewer's document — with a dated validation section appended recording what
was reproduced, the two corrections (D5's declined siblings, D6's dropped
warning), and the F2a/F2b split. It is indexed in `docs/README.md`; nothing
else would catch the omission, since `xtask docs-check` validates only ADR
names and crate names.

## Out of scope

**F2b — retargeting the `missing_config_var` span to the pack site.** The
review rated this S on the basis that the span is "already computed elsewhere".
It is not. `ResolveError` carries no position and `resolve()` is documented
"Pure and total"; E3's pack span comes from hurl's own parser reporting a
line/column that feeds `locate::payload_line_span(…, rel_line)`, and nothing
computes a `rel_line` for a resolve failure. Supplying one means threading an
offset out of a deliberately position-free pure function and carrying pack
identity to the diagnostic site — a design change to a core type. It gets its
own spec.

## Testing

| Item | Test |
| --- | --- |
| D1/D2 | the scaffolded suite dry-runs green; a second `init` creates nothing; a pre-existing file is never overwritten |
| D3 | the scaffold carries the schema file and the pack modeline |
| D5 | unit test asserting the suggestion names the near key; seeded corpus case |
| D6 | assert_cmd: dry-run success stdout carries the next command |
| D7/D8 | `cargo run -p xtask -- docs-check` |

Every test must fail without its change. The scaffold-dry-runs-green test is
the one that matters most: it is what keeps D2's "the template is the tutorial"
true over time.

## Delivery

- One branch, one PR: `feat/first-run-ux`.
- Changelog under `## [Unreleased]` — `### Added` for `init` and the README
  sections, `### Fixed`/`### Changed` for the diagnostic and the success line.
- No version bump. A new subcommand is additive, so the release is
  PATCH-eligible; the version is chosen at release time, not baked in here.
- Full gate: `cargo fmt --all --check`;
  `cargo clippy --all-targets --all-features -- -D warnings`;
  `cargo nextest run --profile ci`; `cargo test --doc`;
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace`;
  `cargo run -p xtask -- docs-check`.

## Constraints

- `proef-core` stays sans-IO: D5 adds a field and a pure suggestion lookup, no
  IO, no clock, no randomness.
- No new dependencies; hurl pins stay `=8.0.1`.
- Package name for `cargo -p` and `assert_cmd::cargo::cargo_bin` is `proef`.
- One canonical mechanism per outcome — D3 reuses `commands::schema` rather
  than reimplementing it; D2 reuses the tutorial's file shapes.
- No raw print macros in `proef-cli` (`render::outln!` / `errln!`), enforced by
  `crates/proef-cli/tests/stderr_hygiene.rs`.
- No task ids, plan numbers, or review-section references in code comments.
- No AI-attribution commit trailers.
