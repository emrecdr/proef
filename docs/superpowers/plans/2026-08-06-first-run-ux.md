# First-Run UX Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the four first-use gaps an external reviewer reproduced against 0.5.3 — no scaffold, a diagnostic below its siblings' standard, a silent success path, and two README blind spots.

**Architecture:** A new `proef init` writes exactly the files `GETTING-STARTED.md` teaches and installs the pack schema by calling the existing `commands::schema`, so there stays one implementation of each. A `suggestion` field on one core error variant copies a pattern two sibling variants already use. Everything else is one line of output and documentation.

**Tech Stack:** Rust 2024, clap, `cargo-nextest`, `assert_cmd`, `tempfile`.

**Approved spec:** `docs/superpowers/specs/2026-08-06-first-run-ux-design.md` — it carries the verified `file:line` facts. Cite them; do not re-derive.

**Branch:** `feat/first-run-ux`, off `main` (`03b442f`). Spec committed at `c1a9b18`.

## Global Constraints

- **ONE branch, ONE PR** — `feat/first-run-ux`. All five tasks land here.
- `proef-core` stays sans-IO. Task 2 adds a field and a pure lookup only — no IO, no clock, no randomness.
- No new dependencies. hurl pins stay exactly `hurl = "=8.0.1"`, `hurl_core = "=8.0.1"`.
- The package name for `cargo -p` and `assert_cmd::cargo::cargo_bin` is **`proef`**, NOT `proef-cli`.
- **No raw print macros in `proef-cli`.** Use `crate::render::outln!` for stdout and `crate::render::errln!` for stderr. `crates/proef-cli/tests/stderr_hygiene.rs` fails the build otherwise.
- No task ids, plan numbers, or review-section references in code comments. The changelog carries those; cite durable ADRs instead.
- No AI-attribution commit trailers.
- **Every test must genuinely fail without its change.** Demonstrate RED before GREEN — this repo has repeatedly caught vacuous tests.
- No version bump. Everything rides the next release under `## [Unreleased]`.

## File Structure

| File | Change | Responsibility |
| --- | --- | --- |
| `crates/proef-cli/src/init.rs` | Create | The scaffold: template bytes, never-overwrite writes, next-step line |
| `crates/proef-cli/src/main.rs` | Modify (`:78-166` enum, `:376` dispatch) | One clap variant + one dispatch arm |
| `crates/proef-cli/tests/init.rs` | Create | Scaffold behavior, including the dry-runs-green guarantee |
| `crates/proef-core/src/resolve.rs` | Modify (`:100-110`, `:352-368`) | `suggestion` field + namespace-scoped lookup |
| `crates/proef-core/public-api.txt` | Regenerate | Snapshot of the changed public surface |
| `tests/errors/resolve__missing_config_var/` | Create | Seeded corpus case |
| `crates/proef-cli/src/commands.rs` | Modify (`:197`) | Next-command line on dry-run success |
| `README.md`, `docs/PRD.md`, `docs/DIAGNOSTICS.md`, `docs/README.md`, `docs/CHANGELOG.md`, `docs/FIRST-RUN-UX-REVIEW.md` | Modify / add | Documentation |

---

### Task 1: `proef init [dir]`

**Files:**
- Create: `crates/proef-cli/src/init.rs`
- Modify: `crates/proef-cli/src/main.rs` (add a `Command` variant near `:161`, a dispatch arm near `:376`, and `mod init;`)
- Test: `crates/proef-cli/tests/init.rs` (create)

**Interfaces:**
- Consumes: `crate::commands::schema(add_to: &[PathBuf]) -> ExitCode` (`crates/proef-cli/src/commands.rs:437`) — installs `proef-pack.schema.json` beside each given pack **and** adds the editor modeline to it. `proef_core::error::ExitCode` with variants `Success`, `UserError`, `SystemError`.
- Produces: `pub fn init(dir: &Path) -> ExitCode` in `crate::init`.

**Why the template is what it is:** it must mirror the shapes `docs/GETTING-STARTED.md` §2–§3 teaches — a fixed-sentence macro and a parameterized one — because the scaffold and the tutorial must not become two different starting points. It is deliberately trimmed of the tutorial's `${secret:apiToken}` step so the scaffold dry-runs green with no secret store configured.

- [ ] **Step 1: Write the failing tests**

Create `crates/proef-cli/tests/init.rs`:

```rust
//! `proef init` scaffolds a working suite: the files it writes must dry-run
//! green, it must never overwrite authored work, and running it twice must be
//! a no-op.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::*;

fn proef(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("proef").unwrap();
    cmd.current_dir(dir).env("NO_COLOR", "1");
    cmd
}

/// The load-bearing test: whatever `init` writes must actually validate. This
/// is what stops the scaffold and the tutorial from silently diverging.
#[test]
fn scaffold_dry_runs_green() {
    let tmp = tempfile::tempdir().unwrap();
    proef(tmp.path()).arg("init").assert().code(0);
    proef(tmp.path())
        .args(["test", "--dry-run"])
        .assert()
        .code(0)
        .stdout(contains("dry-run OK"));
}

/// Running init twice creates nothing the second time.
#[test]
fn init_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    proef(tmp.path()).arg("init").assert().code(0);
    proef(tmp.path())
        .arg("init")
        .assert()
        .code(0)
        .stdout(contains("skipped").or(contains("nothing to create")));
}

/// An authored file is never overwritten.
#[test]
fn init_never_overwrites_an_existing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("proef.toml");
    std::fs::write(&config, "# authored by hand\n").unwrap();
    proef(tmp.path()).arg("init").assert().code(0);
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        "# authored by hand\n",
        "init overwrote an existing file"
    );
}

/// The schema install runs as part of init, so editor completion works on the
/// first run without discovering a flag.
#[test]
fn scaffold_carries_the_pack_schema_and_modeline() {
    let tmp = tempfile::tempdir().unwrap();
    proef(tmp.path()).arg("init").assert().code(0);
    assert!(
        tmp.path()
            .join("suite/packs/proef-pack.schema.json")
            .exists(),
        "schema file missing from the scaffold"
    );
    let pack = std::fs::read_to_string(tmp.path().join("suite/packs/api.yaml")).unwrap();
    assert!(
        pack.contains("yaml-language-server: $schema=./proef-pack.schema.json"),
        "pack modeline missing: {pack}"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
cargo nextest run -p proef --test init
```

Expected: **FAIL** — `error: unrecognized subcommand 'init'`, so every test exits non-zero. Record the output in the task report.

- [ ] **Step 3: Create the scaffold module**

Create `crates/proef-cli/src/init.rs`:

```rust
//! `proef init` — write a minimal working suite into an empty (or partly
//! populated) directory.
//!
//! The files mirror the ones `docs/GETTING-STARTED.md` teaches, so the
//! scaffold and the tutorial cannot drift into two different starting shapes.
//! Nothing is ever overwritten: an existing file is reported and left alone,
//! which makes a second run a no-op and removes any need for a `--force` flag
//! that could destroy authored work.

use std::path::{Path, PathBuf};

use proef_core::error::ExitCode;

const CONFIG: &str = r#"# proef.toml — project configuration.
# Variables live here, never in .feature files: packs read them as ${url:…} / ${vars:…}.
[run]
suite = "suite"                    # `proef test` needs no path argument

[url]
# ${url:base} resolves from here; PROEF_BASE_URL overrides it when set.
base = "${env:PROEF_BASE_URL:-http://127.0.0.1:8787}"
"#;

const FEATURE: &str = r#"Feature: Directory search
  Scenario: A known record is found
    Given the service is healthy
    When the operator searches for "Acme"
"#;

const PACK: &str = r#"macros:
  health:
    match: the service is healthy
    steps:
      - hurl: |
          GET ${url:base}/health
          HTTP 200

  search:
    params: [term]
    match: the operator searches for {term}
    steps:
      - name: search records for ${term}
        hurl: |
          GET ${url:base}/search
          [Query]
          q: ${term}
          HTTP 200
"#;

/// Scaffold a suite under `dir`, then install the pack schema and print the
/// next command.
pub fn init(dir: &Path) -> ExitCode {
    let pack_path = dir.join("suite/packs/api.yaml");
    let files: [(PathBuf, &str); 3] = [
        (dir.join("proef.toml"), CONFIG),
        (dir.join("suite/case.feature"), FEATURE),
        (pack_path.clone(), PACK),
    ];

    let mut created = 0usize;
    let mut skipped = 0usize;
    for (path, contents) in &files {
        if path.exists() {
            crate::render::outln!("  skipped {} (already exists)", path.display());
            skipped += 1;
            continue;
        }
        let parent = crate::fsutil::parent_dir(path);
        if let Err(err) = std::fs::create_dir_all(&parent) {
            crate::render::errln!("error: cannot create {}: {err}", parent.display());
            return ExitCode::SystemError;
        }
        if let Err(err) = std::fs::write(path, contents) {
            crate::render::errln!("error: cannot write {}: {err}", path.display());
            return ExitCode::SystemError;
        }
        crate::render::outln!("  created {}", path.display());
        created += 1;
    }

    // The same install path `proef schema --add-to` runs — one implementation
    // of "write the schema and the modeline", not two.
    let schema_exit = crate::commands::schema(std::slice::from_ref(&pack_path));
    if schema_exit != ExitCode::Success {
        return schema_exit;
    }

    if created == 0 {
        crate::render::outln!("\nnothing to create — {skipped} file(s) already present");
    } else {
        crate::render::outln!("\ncreated {created} file(s), skipped {skipped}");
    }
    crate::render::outln!("next: proef test --dry-run");
    ExitCode::Success
}
```

If `ExitCode` does not implement `PartialEq`, compare with `matches!(schema_exit, ExitCode::Success)` instead of `!=`.

- [ ] **Step 4: Wire the subcommand**

In `crates/proef-cli/src/main.rs`, add `mod init;` beside the other `mod` declarations. Add this variant to `enum Command` immediately after the `Schema` variant (which ends at `:166`):

```rust
    /// Scaffold a minimal working suite in a new or existing directory
    Init {
        /// Target directory (default: the current directory)
        dir: Option<PathBuf>,
    },
```

Add the dispatch arm beside `Command::Schema` (`:376`):

```rust
        Command::Init { dir } => init::init(&dir.unwrap_or_else(|| PathBuf::from("."))),
```

- [ ] **Step 5: Run the tests to verify they pass**

Run:

```bash
cargo nextest run -p proef --test init
```

Expected: **PASS**, all four.

- [ ] **Step 6: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

Expected: all green. `tests/stderr_hygiene.rs` must still pass — `init.rs` uses only `render::outln!`/`errln!`.

- [ ] **Step 7: Commit**

```bash
git add crates/proef-cli/src/init.rs crates/proef-cli/src/main.rs crates/proef-cli/tests/init.rs
git commit -m "feat(cli): add proef init to scaffold a working suite

Every subcommand assumed a suite already existed, so the first-run path began
with a blank page and two documents. init writes the files GETTING-STARTED
teaches — config, feature, pack — and installs the pack schema by calling the
same function schema --add-to runs, so there is one implementation of that.

Nothing is ever overwritten: an existing file is reported and left alone, which
makes a second run a no-op and removes any need for a --force flag that could
destroy authored work. A test asserts the scaffold dry-runs green, so the
template and the tutorial cannot drift apart unnoticed."
```

---

### Task 2: did-you-mean on `resolve::missing_config_var`

**Files:**
- Modify: `crates/proef-core/src/resolve.rs` (variant at `:100-110`, construction at `:352-368`)
- Modify: `crates/proef-core/public-api.txt` (regenerated)
- Modify: `docs/DIAGNOSTICS.md` (`:86` corpus column, `:108` coverage count)
- Test: unit test in `crates/proef-core/src/resolve.rs`; corpus case `tests/errors/resolve__missing_config_var/`

**Interfaces:**
- Consumes: `proef_core::matcher::closest(input: &str, candidates: impl Iterator<Item = &str>) -> Option<&str>` (`crates/proef-core/src/matcher.rs:316`). `ctx.config_vars` is a map keyed by the **full** `"namespace:key"` reference (`resolve.rs:350-358`).
- Produces: `ResolveError::MissingConfigVar { namespace, key, suggestion }`.

**Sans-IO note:** this is a pure lookup over data already in `ResolveCtx`. No IO, no clock, no randomness.

- [ ] **Step 1: Write the failing unit test**

Add to the `#[cfg(test)] mod tests` block in `crates/proef-core/src/resolve.rs`, beside the existing `missing_config_var_errors_in_strict_and_dry_run_but_probes` test (`:506`):

```rust
    #[test]
    fn missing_config_var_suggests_the_closest_key_in_the_same_namespace() {
        let f = Fixture::new();
        // The fixture defines `url:base`; `bse` is one edit away.
        let err = resolve("${url:bse}", &f.ctx(ResolveMode::Strict)).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("did you mean `base`"),
            "expected a suggestion naming the near key, got: {message}"
        );
    }
```

If the fixture does not define `url:base`, read the `Fixture` helper in the same module and use whichever `url:` key it does define, adjusting the typo accordingly. Do not weaken the assertion.

- [ ] **Step 2: Run it to verify it fails**

Run:

```bash
cargo nextest run -p proef-core missing_config_var_suggests
```

Expected: **FAIL** — the message has no `did you mean` clause today. Record the output.

- [ ] **Step 3: Add the field**

In `crates/proef-core/src/resolve.rs`, change the `MissingConfigVar` variant (`:100-110`) to carry a suggestion, using the same formatting the two sibling variants already use (`:79` and `:126`):

```rust
    /// `${url:key}` / `${vars:key}` referencing a value defined in neither the
    /// base `proef.toml` table nor the active `[env.<name>]` profile.
    #[error(
        "{namespace} variable `{key}` is not set — define `[{namespace}]` `{key}` in proef.toml (or in the active `[env.<name>.{namespace}]`){}",
        suggestion.as_ref().map(|s| format!(" — did you mean `{s}`?")).unwrap_or_default()
    )]
    MissingConfigVar {
        /// The namespace as written (`url` or `vars`).
        namespace: String,
        /// The referenced key.
        key: String,
        /// Closest key defined in the same namespace, when one is near.
        suggestion: Option<String>,
    },
```

- [ ] **Step 4: Compute the suggestion at the construction site**

Replace the body of `resolve_config_var` (`:352-368`) with:

```rust
fn resolve_config_var(
    name: &str,
    namespace: &str,
    arg: &str,
    ctx: &ResolveCtx<'_>,
) -> Result<String, ResolveError> {
    match ctx.config_vars.get(name) {
        Some(value) => Ok(value.clone()),
        None => {
            // Candidates are scoped to the same namespace, so a `url:` typo can
            // never suggest a `vars:` key. Keys are stored as `namespace:key`.
            let prefix = format!("{namespace}:");
            let suggestion = crate::matcher::closest(
                arg,
                ctx.config_vars
                    .keys()
                    .filter_map(|k| k.strip_prefix(&prefix)),
            )
            .map(str::to_owned);
            probe_or(
                ResolveError::MissingConfigVar {
                    namespace: namespace.to_owned(),
                    key: arg.to_owned(),
                    suggestion,
                },
                ctx.mode,
            )
        }
    }
}
```

Fix any other construction sites the compiler flags — the new field is mandatory.

- [ ] **Step 5: Run the unit test to verify it passes**

Run:

```bash
cargo nextest run -p proef-core missing_config_var
```

Expected: **PASS**, both the new test and the pre-existing `missing_config_var_errors_in_strict_and_dry_run_but_probes`.

- [ ] **Step 6: Seed the corpus case**

Create `tests/errors/resolve__missing_config_var/` with exactly two files, matching the layout of `tests/errors/resolve__unknown_variable/`.

**Do not add a `proef.toml`.** The corpus is dry-run from the repo root, and the repo's own `proef.toml:15-16` already defines `[url] base` — that is precisely what gives the suggestion a candidate to find. A local config would shadow it and the case would assert nothing.

`tests/errors/resolve__missing_config_var/case.feature`:

```gherkin
Feature: E
  Scenario: S
    When I fetch the page
```

`tests/errors/resolve__missing_config_var/packs/broken.yaml`:

```yaml
macros:
  fetch:
    match: I fetch the page
    steps:
      - hurl: |
          GET ${url:bse}/health
          HTTP 200
```

Verify it fails with the right code and nothing else:

```bash
cargo run -q -p proef -- test --dry-run tests/errors/resolve__missing_config_var
```

Expected: exit 2, one error, code `proef::resolve::missing_config_var`, and the message carries `did you mean \`base\``.

- [ ] **Step 7: Regenerate the public-api snapshot**

Adding a field to a public enum variant changes `proef-core`'s public surface, which is snapshot-locked.

```bash
PROEF_PUBLIC_API_UPDATE=1 cargo run -p xtask -- public-api
git diff --stat crates/proef-core/public-api.txt
```

Expected: the diff shows only the `MissingConfigVar` variant gaining `suggestion`. Record the delta in the task report. If the diff is larger than that, stop and report — something else changed.

- [ ] **Step 8: Update DIAGNOSTICS.md**

Two edits in `docs/DIAGNOSTICS.md`:

1. Line ~86: tick the corpus column for `resolve::missing_config_var` so the row matches its now-seeded neighbours (copy the exact marker used by a seeded row, e.g. `resolve::unknown_variable` at `:84`).
2. Line ~108: the sentence reads "23 of the 59 codes carry a seeded corpus case today". Change **23** to **24**.

- [ ] **Step 9: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

Expected: all green. The corpus guard test that walks `tests/errors/` must pick up the new case.

- [ ] **Step 10: Commit**

```bash
git add crates/proef-core/src/resolve.rs crates/proef-core/public-api.txt tests/errors/resolve__missing_config_var docs/DIAGNOSTICS.md
git commit -m "fix(core): suggest the closest key when a config variable is unset

resolve::missing_config_var named the missing key but not the near miss, while
two sibling codes in the same family — unknown_variable and fake_unknown —
already suggest. A typo'd \${url:bse} now points at \`base\`.

Candidates are scoped to the referenced namespace, so a url: typo can never
suggest a vars: key. The lookup is pure: it reads the config map already in
ResolveCtx, so the core stays sans-IO. Seeds the corpus case the code was
missing and ticks the coverage count."
```

---

### Task 3: next command on dry-run success

**Files:**
- Modify: `crates/proef-cli/src/commands.rs` (after the summary at `:197`)
- Test: `crates/proef-cli/tests/init.rs` (append) — the scaffold gives a ready-made green suite to assert against

**Interfaces:**
- Consumes: the scaffold from Task 1 (`proef init`), used to produce a green dry-run in the test.
- Produces: nothing other tasks depend on.

**Scope note:** print the next command only. Do **not** add a warning when no `[url]` key is configured. The spec records why: a suite with absolute URLs and no `proef.toml` dry-runs green with 0 warnings, so the warning would fire on a valid suite — and when a suite *does* reference an unconfigured `${url:key}`, dry-run already fails with `missing_config_var`, making the warning redundant.

- [ ] **Step 1: Write the failing test**

Append to `crates/proef-cli/tests/init.rs`:

```rust
/// A passing dry-run names the next command. Every failure path already names
/// a remedy; the success path is where a new user decides whether to continue.
#[test]
fn dry_run_success_names_the_next_command() {
    let tmp = tempfile::tempdir().unwrap();
    proef(tmp.path()).arg("init").assert().code(0);
    proef(tmp.path())
        .args(["test", "--dry-run"])
        .assert()
        .code(0)
        .stdout(contains("dry-run OK"))
        .stdout(contains("next: proef test"));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run:

```bash
cargo nextest run -p proef --test init dry_run_success_names
```

Expected: **FAIL** — the summary line is the last thing printed today. Record the output.

- [ ] **Step 3: Print the next command**

In `crates/proef-cli/src/commands.rs`, immediately after the `crate::render::outln!` call that emits the `dry-run OK:` summary (`:197`), and inside the same success branch, add:

```rust
    crate::render::outln!("next: proef test");
```

Keep it inside the success path only — a failing dry-run must not print it.

- [ ] **Step 4: Run the test to verify it passes**

Run:

```bash
cargo nextest run -p proef --test init dry_run_success_names
```

Expected: **PASS**.

- [ ] **Step 5: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

Expected: all green. Watch for insta snapshots covering dry-run console output — if one moves, review the diff deliberately with `cargo insta review` and be able to say why it changed. Never blind-accept.

- [ ] **Step 6: Commit**

```bash
git add crates/proef-cli/src/commands.rs crates/proef-cli/tests/init.rs
git commit -m "feat(cli): name the next command after a successful dry-run

Every failure path ends with a remedy; the success path stopped talking at
exactly the moment a new user decides whether to continue. A passing dry-run
now names the command that runs the suite for real."
```

---

### Task 4: README — parameterized macro and the non-goals section

**Files:**
- Modify: `README.md`

**Interfaces:**
- Consumes: nothing. Documentation only.
- Produces: nothing other tasks depend on.

- [ ] **Step 1: Confirm the gap still exists**

Run:

```bash
rg -c 'params:' README.md ; echo "exit=$?"
```

Expected: no output, `exit=1` — `params:` appears zero times. If it appears, the gap has already been closed; report and skip to Step 3.

- [ ] **Step 2: Add a parameterized macro to the pack example**

Find the existing pack example in `README.md` (the macro with `match: the workspace is provisioned` near `:43`). Add a second macro beside it demonstrating a bound placeholder. The canonical syntax is `docs/AUTHORING.md:27-36` — `match:` carries `{name}`, `params:` declares it, `defaults:` supplies optional values:

```yaml
  search:
    params: [term]
    match: the operator searches for {term}
    steps:
      - hurl: |
          GET ${url:base}/search
          [Query]
          q: ${term}
          HTTP 200
```

Add one sentence naming what the reader is seeing: every `{capture}` must be a declared param, and quoted arguments shed their quotes.

- [ ] **Step 3: Add the non-goals section**

Add a `## What proef deliberately isn't` section to `README.md`, after `## Why proef` (`:69`). Use the PRD's own wording (`docs/PRD.md:41-46`) rather than a loose paraphrase:

```markdown
## What proef deliberately isn't

These are settled non-goals, not gaps awaiting a contribution:

- **No importing or round-tripping hand-written hurl files.** Artifacts flow
  outward only: `proef artifacts` emits `.hurl` you can run with stock hurl,
  and nothing reads `.hurl` back in.
- **No API mocking or contract testing**, and no load testing.
- **No second engine.** The factory/session seam exists for dependency
  hygiene (ADR-0002), not as a roadmap.
- **No desktop dashboard or server mode**, and no dynamic plugin loading.

**Already have a hurl corpus?** The supported path is pasting your existing
request bodies into a pack's `steps[].hurl` blocks — they are raw hurl,
validated by the real parser at pack load, so they carry over unmodified.
```

- [ ] **Step 4: Verify**

```bash
rg -c 'params:' README.md
rg -n 'What proef deliberately' README.md
cargo run -p xtask -- docs-check
```

Expected: `params:` now appears at least once, the section exists, docs-check aligned.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: show a parameterized macro and state the non-goals in the README

The README never showed a bound placeholder — params: appeared zero times and
the only match: was a static sentence — so the syntax that makes packs worth
writing existed only in GETTING-STARTED and AUTHORING. An experienced reader
wrote a dumber test rather than guess it.

The non-goals section states the boundary where newcomers actually look. The
same reader proposed a documented permanent non-goal (importing hurl) as a
top-two recommendation after reading the README, which is evidence the charter
was invisible outside the maintainer documents."
```

---

### Task 5: PRD story, review doc, index, changelog

**Files:**
- Modify: `docs/PRD.md` (after US-12 at `:98`)
- Modify: `docs/FIRST-RUN-UX-REVIEW.md` (currently untracked — commit it and append a section)
- Modify: `docs/README.md` (the corpus index table)
- Modify: `docs/CHANGELOG.md` (`## [Unreleased]`)

**Interfaces:**
- Consumes: nothing. Documentation only.
- Produces: nothing. Final task.

- [ ] **Step 1: Add the PRD user story**

In `docs/PRD.md`, after US-12 (`:98`), add a story in the existing format (`US-N (Pn) Title. *AC:* …`):

```markdown
US-13 (P1) I can start from something that works. *AC:* `proef init` writes a
minimal suite (`proef.toml`, one `.feature`, one matching pack) that passes
`--dry-run` unchanged, installs the pack JSON Schema for editor completion, and
never overwrites an existing file.
```

- [ ] **Step 2: Append validation notes to the review**

`docs/FIRST-RUN-UX-REVIEW.md` is an external reviewer's document. Leave its body **verbatim** and append:

```markdown
---

## Validation notes (2026-08-06, maintainer)

Every finding above was re-checked against the tree at `03b442f`.

**Reproduced exactly:** the 13-command inventory with no scaffold; the single
unrelated `IMPROVEMENT-PLAN.md` hit at line 258; `params:` absent from the
README; `DIAGNOSTICS.md`'s "23 of 59" coverage note and the empty corpus column
for `missing_config_var`; the PRD §3/§4 and ADR-0016 quotations; E4's missing
suggestion and feature-sentence span; E3's pack-relative span; and the silent
dry-run success line.

**Two corrections.**

1. **§4's sibling proposal is narrowed.** Extending did-you-mean to
   `resolve::missing_env` is declined: its candidate set is the injected
   environment snapshot, so suggesting from it risks surfacing unrelated
   environment variable names in diagnostics, against the secret-masking
   posture. `resolve::unknown_namespace` already enumerates all seven valid
   namespaces in its message. Sibling codes share a *shape*, not a *candidate
   set*. `missing_config_var` is implemented.
2. **§5's `[url]` warning is dropped.** The claim that a missing `[url]` key is
   "the guaranteed next failure" does not hold: a suite with absolute URLs and
   no `proef.toml` at all dry-runs green with 0 warnings and executes fine, so
   the warning would fire on a valid suite. When a suite *does* reference an
   unconfigured `${url:key}`, dry-run already fails with
   `missing_config_var` — making the warning redundant in the case it targets.
   The next-command half of §5 is implemented.

**F2 is split.** The did-you-mean half is small, as §8 says. The span retarget
is not: §8 justifies it as "a span already computed elsewhere", but
`ResolveError` carries no position and `resolve()` is documented "Pure and
total". E3's pack span comes from hurl's own parser reporting a line/column
that feeds `locate::payload_line_span(…, rel_line)`; nothing computes a
`rel_line` for a resolve failure. Supplying one means threading an offset out
of a deliberately position-free pure function and carrying pack identity to the
diagnostic site. It is deferred to its own spec.
```

- [ ] **Step 3: Index the review**

Add a row to the "Reading order" table in `docs/README.md`. The table's columns are `| # | Document | What it answers | Audience |`. Place the row beside the other maintainer-audience entries (near `IMPROVEMENT-PLAN.md` at `:31`), using `—` in the `#` column as those rows do:

```markdown
| — | [FIRST-RUN-UX-REVIEW.md](FIRST-RUN-UX-REVIEW.md) | External first-use review of 0.5.3 with maintainer validation notes: what the first thirty minutes cost, and which findings were acted on | maintainers |
```

- [ ] **Step 4: Update the changelog**

Add to `## [Unreleased]` in `docs/CHANGELOG.md`, creating the sections that do not yet exist and keeping Keep-a-Changelog order (Added → Changed → Fixed → Documentation):

```markdown
### Added

- **`proef init` scaffolds a working suite.** It writes the files
  `GETTING-STARTED.md` teaches — `proef.toml`, one `.feature`, one matching
  pack — installs the pack JSON Schema for editor completion, and prints the
  next command. Nothing is ever overwritten, so a second run is a no-op and no
  `--force` flag exists to destroy authored work. A test asserts the scaffold
  passes `--dry-run` unchanged.
- The README now shows a parameterized macro and states the load-bearing
  non-goals, including the supported path for teams that already have a hurl
  corpus.

### Changed

- A passing `--dry-run` now names the next command. Every failure path already
  named a remedy; the success path stopped talking at the moment a new user
  decides whether to continue.

### Fixed

- `resolve::missing_config_var` now suggests the closest key defined in the
  same namespace, matching `resolve::unknown_variable` and
  `resolve::fake_unknown`. Candidates are namespace-scoped, so a `${url:…}`
  typo can never suggest a `[vars]` key. The code also gains the seeded corpus
  case it was missing.
```

- [ ] **Step 5: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
```

Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add docs/PRD.md docs/FIRST-RUN-UX-REVIEW.md docs/README.md docs/CHANGELOG.md
git commit -m "docs: record the first-run UX story, the source review, and the changelog

Adds US-13 for proef init, commits the external first-run review verbatim with
maintainer validation notes appended — recording what reproduced, the two
corrections, and why the span retarget is larger than the review estimated —
and indexes it in the docs corpus."
```

---

## Definition of Done

- `proef init` exists, scaffolds a suite that passes `--dry-run` unchanged, is idempotent, and never overwrites an existing file.
- The scaffold carries `proef-pack.schema.json` and the pack modeline, installed via `commands::schema` rather than a second implementation.
- `resolve::missing_config_var` suggests a namespace-scoped near key; `crates/proef-core/public-api.txt` is regenerated with only that delta; `tests/errors/resolve__missing_config_var/` is seeded; `DIAGNOSTICS.md` shows the corpus tick and the count reads 24.
- A passing dry-run names the next command; a failing one does not.
- `README.md` contains at least one `params:` example and the non-goals section.
- `docs/PRD.md` carries US-13; `docs/FIRST-RUN-UX-REVIEW.md` is committed with validation notes and indexed in `docs/README.md`.
- Every new test was observed failing before its change. The RED output is recorded in each task report.
- The full six-command gate is green, and `## [Unreleased]` carries all three sections with no version bump.
