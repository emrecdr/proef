# CLI Output & Exit Integrity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `proef` reporting success while producing wrong, incomplete, or silently-ignored output — six validated findings in the CLI's output and exit paths.

**Architecture:** All six live in `proef-cli`; `proef-core` is not touched. Two findings introduce a shared mechanism (an env-var reader that distinguishes absent from unreadable, and a process-level stdout-failure flag folded into the single exit funnel); four are local corrections to a tee, a formatter, an href, and a diff comparison.

**Tech Stack:** Rust 1.97.1, edition 2024. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-06-cli-output-exit-integrity-design.md` (decisions D1–D6). It was re-validated against `8e52f99`; the citations below were re-derived at `2961fd2` (v0.7.0).

## Global Constraints

- **`proef-core` is not modified.** Every change is in `crates/proef-cli/`. If a task appears to need a core change, stop and report — that is a scope error, not a licence.
- **No new dependencies.** hurl pins stay `=8.0.1`. No version bump (this rides `[Unreleased]`).
- **One canonical mechanism per outcome.** D1 gets one shared reader used by all four call sites, not four local matches. D2 gets one flag checked once, not a per-command check.
- **Exit codes are a contract (ADR-0009):** `0` ok · `1` test failure · `2` user error · `3` system error. The typed enum is `proef_core::error::ExitCode`; `tests/cli.rs` pins the mapping.
- **Raw print macros are banned in `proef-cli`.** Use `crate::render::outln!` / `errln!` — a source-scanning test enforces this.
- **`std::env::set_var`/`remove_var` are `unsafe` in edition 2024.** Tests that set env vars need an `unsafe` block and a safety comment. `cargo nextest` runs one process per test, which is what makes them safe; give each test a uniquely-named variable anyway so a threaded `cargo test` run cannot interfere.
- **`std::env::var_os` call sites are NOT in scope.** `var_os` returns an `OsString` and never collapses non-UTF-8 into absence. `secretstore.rs:82-85` (`PROEF_CONFIG_DIR`, `XDG_CONFIG_HOME`, `HOME`), `ci_reports.rs:106`, `render.rs:44`, and `exec.rs:393` are correct as written — do not "fix" them.
- **Every test must genuinely fail without its change — demonstrate RED.** Run the test, paste the real failure output into the report, then implement. This project has repeatedly caught vacuous tests and tests that silently stopped discriminating.
- **No task ids, plan numbers, or review-section references in code comments.** Cite durable ADRs or state the invariant. No AI-attribution commit trailers.
- **Changelog:** each task adds its own entry under `## [Unreleased]` in `docs/CHANGELOG.md`, in the same commit.

**Gate — all seven, green before every commit:**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --profile ci
cargo test --doc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace
cargo run -p xtask -- docs-check
cargo check --manifest-path fuzz/Cargo.toml --all-targets --locked
```

## File Structure

| File | Change | Task |
|---|---|---|
| `crates/proef-cli/src/envvar.rs` | **new** — one reader that separates absent from unreadable | 1 |
| `crates/proef-cli/src/main.rs` | register `mod envvar`; `PROEF_ENV` site; fold the stdout flag into the exit funnel | 1, 2 |
| `crates/proef-cli/src/secretstore.rs` | three env sites (`PROEF_KEY` ×2, `PROEF_SECRET_<NAME>`) | 1 |
| `crates/proef-cli/src/render.rs` | stdout-failure flag + accessors; `outln!` records it | 2 |
| `crates/proef-cli/src/exec.rs` | `Tee::write` mirrors only accepted bytes | 3 |
| `crates/proef-cli/src/fmt.rs` | preserve the file's dominant line ending | 4 |
| `crates/proef-cli/src/report.rs` | `artifacts_href`'s else-branch becomes absolute | 5 |
| `crates/proef-cli/src/diff.rs` | skip steps absent from base | 6 |
| `docs/TROUBLESHOOTING.md` | the ordinal-shift caveat | 6 |
| `docs/CHANGELOG.md` | one entry per task | all |

---

### Task 1: A malformed environment variable is an error, never silence

**Files:**
- Create: `crates/proef-cli/src/envvar.rs`
- Modify: `crates/proef-cli/src/main.rs` (add `mod envvar;` beside the other `mod` lines near `:8-29`; `active_env` at `:266-268`)
- Modify: `crates/proef-cli/src/secretstore.rs` (`load_or_create_key` `:116`, `resolve_secrets` `:284`, `doctor_checks` `:403`)
- Modify: `crates/proef-cli/src/lsp.rs` (`:45` — the **fifth** site, in the LSP's own startup path)

**There are five sites, not four.** The spec names four; `lsp.rs:45` (`let active_env = std::env::var("PROEF_ENV").ok();`) is a separate code path with its own local, and the workspace-wide sweep that found it is `rg -n 'env::var\(' crates/ --type rust | rg -v 'env::var_os'`. Run that yourself before starting and confirm the list is still exactly these five — if a sixth has appeared, it is in scope, because the point of this task is that the rule is uniform.

**Interfaces:**
- Produces: `pub(crate) fn envvar::read(name: &str) -> Result<Option<String>, String>` — `Ok(None)` means genuinely unset; `Err(message)` means set-but-unreadable, with a user-facing message naming the variable. Later tasks do not consume this.

**The bug.** `std::env::var` returns `Err` for two different situations — the variable is absent, and the variable holds bytes that are not valid UTF-8. `.ok()` and `if let Ok(…)` erase the distinction, so a value proef cannot read becomes indistinguishable from one the user never set. Four sites do this, and each has a different bad consequence:

- `PROEF_KEY` (`:116`) — falls through to the key file and decrypts with the wrong key, reporting **tampering** instead of the real cause. The doc comment at `:95-97` already states this must always error; the code contradicts its own comment.
- `PROEF_KEY` in `doctor_checks` (`:403`) — `doctor` reports the key source as the file when the user set an override.
- `PROEF_SECRET_<NAME>` (`:284`) — falls through to the store and reports a *missing secret*, naming the wrong cause.
- `PROEF_ENV` (`main.rs:267`) — runs against the wrong environment silently.

**One rule for all four.** A variable the user set and proef could not read is always worth saying out loud.

- [ ] **Step 1: Write the failing test**

Create `crates/proef-cli/src/envvar.rs` with the test module only (the function comes in Step 3), so the test compiles against a missing symbol and fails loudly:

```rust
//! Reading environment variables so a value proef cannot read is never
//! mistaken for one the user did not set.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_variable_reads_as_absent() {
        assert_eq!(read("PROEF_TEST_DEFINITELY_UNSET_XYZ"), Ok(None));
    }

    #[test]
    fn a_set_variable_reads_as_its_value() {
        let name = "PROEF_TEST_ENVVAR_PLAIN";
        // SAFETY: nextest runs one test per process, and this variable name
        // is unique to this test, so no other thread observes the mutation.
        unsafe { std::env::set_var(name, "value") };
        let got = read(name);
        unsafe { std::env::remove_var(name) };
        assert_eq!(got, Ok(Some("value".to_owned())));
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_value_is_an_error_not_absence() {
        use std::os::unix::ffi::OsStrExt as _;
        let name = "PROEF_TEST_ENVVAR_NON_UTF8";
        // 0xFF is not valid UTF-8 in any position.
        let bad = std::ffi::OsStr::from_bytes(&[0x66, 0xff, 0x6f]);
        // SAFETY: as above — one process per test, name unique to this test.
        unsafe { std::env::set_var(name, bad) };
        let got = read(name);
        unsafe { std::env::remove_var(name) };
        let Err(message) = got else {
            panic!("a non-UTF-8 value must not read as absent: {got:?}");
        };
        assert!(
            message.contains(name),
            "the message must name the variable so the user can find it: {message}"
        );
    }
}
```

Register the module in `crates/proef-cli/src/main.rs`, keeping the `mod` list alphabetical — `mod envvar;` goes between `mod disk_provider;` and `mod exec;`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p proef envvar`
Expected: FAIL — `cannot find function 'read' in this scope`. A compile failure is the correct RED here; the symbol does not exist yet.

- [ ] **Step 3: Write the reader**

Add above the test module in `crates/proef-cli/src/envvar.rs`:

```rust
/// The value of `name`, or `None` when it is genuinely unset.
///
/// `std::env::var` collapses two different situations into `Err`: the
/// variable is absent, and the variable is set to bytes that are not valid
/// UTF-8. `.ok()` erases that distinction, so a value proef cannot read
/// silently becomes "the user did not set it" — and proef then acts on the
/// wrong input and reports the wrong cause. Callers that need the raw bytes
/// (paths, presence checks) use `std::env::var_os`, which has no such
/// ambiguity and is not affected.
pub(crate) fn read(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!(
            "environment variable `{name}` is set but its value is not valid UTF-8 — \
             unset it, or correct the value"
        )),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p proef envvar`
Expected: PASS, 3 tests.

- [ ] **Step 5: Convert the four call sites**

Each site keeps its own error path — the shared piece is `envvar::read`, not the handling.

`secretstore.rs:116` in `load_or_create_key`:

```rust
    match crate::envvar::read("PROEF_KEY") {
        Ok(Some(value)) => return key_from_env(&value),
        Ok(None) => {}
        Err(message) => return Err(SecretError::User(message)),
    }
```

`secretstore.rs:403` in `doctor_checks` — a doctor check reports, it does not abort:

```rust
    match crate::envvar::read("PROEF_KEY") {
        Ok(Some(value)) => match key_from_env(&value) {
            Ok(_) => checks.push((
                S::Pass,
                "secret key",
                "PROEF_KEY env override (32 bytes)".to_owned(),
            )),
            Err(err) => checks.push((S::Fail, "secret key", err.message().to_owned())),
        },
        Ok(None) => {}
        Err(message) => checks.push((S::Fail, "secret key", message)),
    }
```

Keep whatever `else`/fall-through branch already follows this block for the key-file case; only the `if let Ok(value) = std::env::var(…)` head changes.

`secretstore.rs:284` in the `for name in names` loop:

```rust
        match crate::envvar::read(&env_override(name)) {
            Ok(Some(value)) => {
                secrets.insert(name.clone(), value);
            }
            Ok(None) => from_store.push(name),
            Err(message) => return Err(vec![message]),
        }
```

Check the function's actual error type before writing this — it returns `Result<_, Vec<String>>`, so a single message is wrapped in a `vec![]`. If the signature differs from that, match what is there.

`main.rs:266-268` — `active_env` currently returns `Option<String>`; it must be able to fail. Change it to return `Result<Option<String>, proef_core::error::ExitCode>`, emit the message via `crate::render::errln!`, and have its caller propagate with `?`. The caller is `prepare` at `:275`, which already returns a `Result<…, ExitCode>`:

```rust
/// The active environment: the `--env` flag wins, else `PROEF_ENV`, else none.
fn active_env(flag: Option<String>) -> Result<Option<String>, proef_core::error::ExitCode> {
    if let Some(flag) = flag {
        return Ok(Some(flag));
    }
    crate::envvar::read("PROEF_ENV").map_err(|message| {
        crate::render::errln!("error: {message}");
        proef_core::error::ExitCode::UserError
    })
}
```

The blast radius is one call: `active_env(env)` is called only from `prepare` at `main.rs:278`, which already returns `Result<…, ExitCode>` — so `?` is all that is needed there. The many other `active_env` matches in the codebase are *parameters and locals of that name*, not calls to this function; do not touch them. Verify with `rg -n 'active_env\(' crates/proef-cli/src/`.

`lsp.rs:45` is the fifth site and needs its own treatment, because it is not in the CLI preamble — it runs as the language server starts, before the LSP protocol loop. Read the surrounding function to see what it can return. A malformed `PROEF_ENV` there should stop the server from starting with a message on stderr rather than silently analysing against the wrong environment — an editor showing diagnostics from the wrong config profile is precisely the "reports the wrong cause" failure this task exists to remove. If the function's signature makes that awkward, say so in the report and propose the shape rather than falling back to `.ok()`.

- [ ] **Step 6: Add an end-to-end exit-code test**

`proef-cli/tests/cli.rs` holds the exit-code suite. Add a test asserting a non-UTF-8 `PROEF_ENV` exits `2` (user error), following the file's existing `assert_cmd` idiom — read a neighbouring test first and match it. Set the variable on the `Command`, not on the test process:

```rust
#[cfg(unix)]
#[test]
fn a_non_utf8_env_var_is_a_user_error() {
    use std::os::unix::ffi::OsStrExt as _;
    let bad = std::ffi::OsStr::from_bytes(&[0x66, 0xff, 0x6f]);
    Command::cargo_bin("proef")
        .unwrap()
        .args(["flows", "tests/features"])
        .env("PROEF_ENV", bad)
        .assert()
        .code(2);
}
```

Observe it RED against the pre-Step-5 code (it will exit 0), then GREEN. The binary name is `proef`, not `proef-cli`.

- [ ] **Step 7: Run the full gate and commit**

```bash
git add crates/proef-cli/src/envvar.rs crates/proef-cli/src/main.rs \
        crates/proef-cli/src/secretstore.rs crates/proef-cli/tests/cli.rs docs/CHANGELOG.md
git commit -m "fix(cli): an unreadable environment variable is an error, not silence"
```

Changelog entry under `### Fixed`: a set-but-non-UTF-8 `PROEF_KEY`, `PROEF_ENV`, or `PROEF_SECRET_<NAME>` was indistinguishable from unset, so proef decrypted with the wrong key and reported tampering, ran against the wrong environment, or reported a missing secret — all naming the wrong cause. It is now a user error (exit 2) naming the variable.

---

### Task 2: A failed stdout write reaches the exit code

**Files:**
- Modify: `crates/proef-cli/src/render.rs` (the `outln!` macro at `:13-22`)
- Modify: `crates/proef-cli/src/main.rs` (the exit funnel at `:450`)
- Modify: `docs/TROUBLESHOOTING.md` (the exit-code table, row `3`, at `:17`)

**Two paths deliberately bypass the funnel, and that is correct.** `watch.rs:61` and `exec.rs:190` call `std::process::exit(crate::INTERRUPT_EXIT_CODE)` on a second Ctrl-C. That constant is `130` and its doc comment at `main.rs:35-38` says it is *"deliberately outside the typed `ExitCode` taxonomy (ADR-0009 amendment) — not a graceful outcome, so it bypasses the enum entirely."* A stdout failure must **not** override it: the operator interrupted, and `128 + SIGINT` is the shell convention they expect. Leave both call sites alone, and say in the report that you checked them — this is the kind of gap a reviewer should see reasoned about rather than missed.

**Interfaces:**
- Produces: `pub(crate) fn render::note_stdout_failure()` and `pub(crate) fn render::stdout_failed() -> bool`.

**The bug.** `outln!` reports a non-`BrokenPipe` stdout failure on stderr and continues, so `proef … --output json > /full/disk` exits **0** with truncated or zero-byte JSON. A consumer parsing that output has no signal that it is incomplete.

**Why a process-level flag and not a local `bool`.** The spec points at `junit_failed` (`exec.rs:390` → `:405` → `:500`) as the precedent, and the *shape* is right — a flag folded into the final exit. But `junit_failed` is local to `execute()`, whereas `outln!` is used in eight modules (`commands.rs`, `diff.rs`, `explain.rs`, `init.rs`, `secretstore.rs`, `fmt.rs`, `report.rs`, `exec.rs`). A local bool would cover one command. The flag therefore lives beside the macro that sets it, and is read once at `main`'s single exit funnel — one mechanism, one read.

`BrokenPipe` stays swallowed: `proef … | head` must still exit cleanly, and that is deliberate and tested.

- [ ] **Step 1: Write the failing tests — one portable, one Linux-only**

**Forcing a stdout write failure is not portable, and I checked rather than assumed.** On macOS (a gate platform) `/dev/full` does not exist; redirecting stdout to a read-only fd exits `0` with no failure; closing stdout with `1>&-` also exits `0`; a directory as stdout is rejected by the shell before the process starts. So the end-to-end test is Linux-only, and the mechanism gets its own portable test.

Portable — in `crates/proef-cli/src/render.rs`'s test module (create one if absent):

```rust
    #[test]
    fn a_recorded_stdout_failure_is_visible_to_the_exit_funnel() {
        // The latch is process-global and one-way; `main` reads it once at
        // the exit funnel. This pins the mechanism on every platform,
        // including the ones where a real write failure cannot be forced.
        assert!(!stdout_failed(), "the latch starts clear");
        note_stdout_failure();
        assert!(stdout_failed(), "a recorded failure must be visible");
    }
```

That test mutates process-global state, so it must be the only test in this binary that reads the latch — nextest's process-per-test isolation is what keeps it honest. Note that in the report.

End-to-end — in `crates/proef-cli/tests/cli.rs`, alongside the existing closed-pipe tests:

```rust
// /dev/full accepts opens and fails every write with ENOSPC — a full disk
// without needing one. Linux-only: macOS has no such device, and no
// portable substitute forces a write failure (a read-only or closed stdout
// both exit 0). The mechanism itself is pinned portably in render.rs.
#[cfg(target_os = "linux")]
#[test]
fn a_failed_stdout_write_is_a_system_error() {
    let devfull = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("/dev/full is a standard Linux device");
    Command::cargo_bin("proef")
        .unwrap()
        .args(["flows", "tests/features"])
        .stdout(devfull)
        .assert()
        .code(3);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p proef a_recorded_stdout_failure` — expect FAIL (`cannot find function 'note_stdout_failure'`), the correct RED for a symbol that does not exist yet.

The `/dev/full` test does not run on macOS. **If you are on macOS, say so in the report and do not claim it was observed RED** — it will be exercised by `gates (ubuntu-latest)` in CI. If you are on Linux, run it and paste the failure. Either way, after implementing, check the CI run and confirm the Linux job actually executed it rather than skipping it — a `#[cfg]`-skipped test reports as passing and would be no coverage at all.

- [ ] **Step 3: Add the flag and record failures**

In `crates/proef-cli/src/render.rs`, above the `outln!` macro:

```rust
/// Set when a write to stdout failed for any reason other than a closed
/// pipe. Read once, at `main`'s exit funnel: output proef could not deliver
/// must not look like success. A closed pipe is not a failure — `proef … |
/// head` ends the pipeline on purpose.
static STDOUT_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn note_stdout_failure() {
    STDOUT_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn stdout_failed() -> bool {
    STDOUT_FAILED.load(std::sync::atomic::Ordering::Relaxed)
}
```

`Relaxed` is sufficient: the flag is a one-way latch and the read happens after every writer has finished, on the main thread.

In the `outln!` macro body, record the failure beside the existing report:

```rust
        if let Err(err) = writeln!(::std::io::stdout(), $($arg)*)
            && err.kind() != ::std::io::ErrorKind::BrokenPipe
        {
            crate::render::note_stdout_failure();
            crate::render::errln!("error: cannot write to stdout: {err}");
        }
```

Leave `errln!` alone — stderr carries diagnostics, and a failure to print a diagnostic does not make the command's own output untrustworthy.

- [ ] **Step 4: Fold the flag into the exit funnel**

At `crates/proef-cli/src/main.rs:450`, the single funnel:

```rust
    // Output proef could not deliver is an environment failure, whatever the
    // command's own verdict was: a consumer parsing truncated stdout cannot
    // trust the exit code's usual meaning either.
    let code = if render::stdout_failed() {
        proef_core::error::ExitCode::SystemError
    } else {
        code
    };
    std::process::ExitCode::from(code.code())
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p proef` — the new test passes, and the existing closed-pipe tests still pass (a `BrokenPipe` must not set the flag). Name those tests in the report and confirm they were checked, since they are what pins the deliberate exception.

- [ ] **Step 6: Document the new cause of exit 3**

Exit codes are a contract, and `docs/TROUBLESHOOTING.md:17` is the table a user reads to interpret one. Row `3` currently says *"the environment or proef is at fault: unreachable target, native libs, IO"* with the remedy *"check the target, `proef doctor`, disk"*. Add the new cause to that row in its existing voice — output proef could not write (a full disk, a failing device) now lands here rather than looking like success. Keep the table's column formatting; then run `cargo run -p xtask -- docs-check`.

- [ ] **Step 7: Run the full gate and commit**

```bash
git add crates/proef-cli/src/render.rs crates/proef-cli/src/main.rs \
        crates/proef-cli/tests/cli.rs docs/TROUBLESHOOTING.md docs/CHANGELOG.md
git commit -m "fix(cli): a failed stdout write reaches the exit code"
```

Changelog entry under `### Fixed`: writing to a full disk or other failed stdout exited `0` with truncated output; it now exits `3`. A closed pipe still exits cleanly.

---

### Task 3: The tee mirrors only the bytes the console accepted

**Files:**
- Modify: `crates/proef-cli/src/exec.rs` (`impl Write for Tee` at `:946-952`)

**Interfaces:** none — `Tee` is private to `exec.rs`.

**The bug.** `Write::write` may accept fewer bytes than offered; `write_all` loops on the remainder. `Tee::write` hands the **full** slice to the file on every call and only then writes to the console, so a short console write causes the tail to be written to `run.log` twice. `run.log` is the human-readable run record and gains duplicated fragments.

- [ ] **Step 1: Write the failing test**

Add to `exec.rs`'s test module (locate it with `rg -n 'mod tests' crates/proef-cli/src/exec.rs`; if `Tee`'s file field makes a temp file awkward, note it and adapt — the assertion is what matters):

```rust
    #[test]
    fn the_tee_mirrors_only_the_bytes_the_console_accepted() {
        use std::io::Write as _;

        /// A console that accepts three bytes per call, like a pipe under
        /// pressure — `write_all` then loops on the remainder.
        struct ShortWriter(Vec<u8>);
        impl Write for ShortWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let take = buf.len().min(3);
                self.0.extend_from_slice(&buf[..take]);
                Ok(take)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("run.log");
        let file = std::fs::File::create(&path).expect("create");
        let mut tee = Tee(Box::new(ShortWriter(Vec::new())), Some(file));

        tee.write_all(b"abcdefghij").expect("write_all");
        tee.flush().expect("flush");
        drop(tee);

        let mirrored = std::fs::read_to_string(&path).expect("read");
        assert_eq!(
            mirrored, "abcdefghij",
            "the mirror must be a faithful copy of what the console received"
        );
    }
```

Check how the rest of the crate makes temp dirs (`rg -n 'tempdir|tempfile' crates/proef-cli/src`) and match it — do not add a dependency for this.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p proef the_tee_mirrors_only`
Expected: FAIL — the mirrored text repeats the tail (`abcdefghijdefghij…`-shaped), because each `write_all` iteration re-writes the full remaining slice to the file.

- [ ] **Step 3: Mirror the accepted prefix**

Replace the `write` body at `exec.rs:947-951`:

```rust
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Console first: the mirror copies what the console accepted, so a
        // short write cannot duplicate the tail into `run.log`.
        let written = self.0.write(buf)?;
        if let Some(file) = &mut self.1 {
            let _ = file.write_all(&buf[..written]);
        }
        Ok(written)
    }
```

Leave `flush` as it is.

**A tradeoff to state in the report rather than discover in review.** Writing to the console first means that if the console write *errors*, `run.log` receives nothing — where the old order mirrored the full slice first and so kept bytes the console never displayed. That is the intended reading of "mirror": `run.log` is the console record, so bytes the console never accepted do not belong in it, and after Task 2 a failed console write reaches the exit code instead of passing silently. If you think the opposite trade is better — record everything attempted, on the grounds that a run record is most valuable exactly when output is failing — do not silently choose it; say so and let it be adjudicated.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p proef the_tee_mirrors_only`
Expected: PASS.

- [ ] **Step 5: Run the full gate and commit**

```bash
git add crates/proef-cli/src/exec.rs docs/CHANGELOG.md
git commit -m "fix(cli): the run.log tee no longer duplicates bytes on a short write"
```

Changelog entry under `### Fixed`: `run.log` could gain duplicated fragments when the console accepted a short write, because the tee re-wrote the full slice on every retry. It now mirrors only the accepted bytes.

---

### Task 4: `fmt` preserves the file's dominant line ending

**Files:**
- Modify: `crates/proef-cli/src/fmt.rs` (`normalize_pack` at `:77-131`, specifically the join at `:128-130`)

**Interfaces:** none.

**The bug.** `proef fmt` promises to normalize *the raw hurl blocks inside macro packs*. `normalize_pack` splits with `text.lines()` — which strips both `\n` and `\r\n` — and rejoins with `out.join("\n")` plus a trailing `'\n'`. Every CRLF file is rewritten to LF wholesale, so on an `autocrlf` checkout `fmt --check` is permanently red through no fault of the author.

- [ ] **Step 1: Write the failing test**

Add to `fmt.rs`'s test module (existing tests are at `:137` and `:146` — read them first and match their style):

```rust
    #[test]
    fn normalizing_preserves_a_crlf_file_s_line_endings() {
        // `fmt` normalizes hurl blocks, not line endings: rewriting them
        // would make `fmt --check` permanently dirty on an autocrlf
        // checkout, which is a supported way to clone this repo.
        let pack = "macros:\r\n  ping:\r\n    match: I ping\r\n    steps:\r\n      - hurl: |\r\n          GET http://x/a\r\n          HTTP 200\r\n";
        let formatted = normalize_pack(pack);
        assert!(
            formatted.contains("\r\n"),
            "CRLF input must stay CRLF: {formatted:?}"
        );
        assert!(
            !formatted.replace("\r\n", "").contains('\n'),
            "no bare LF may survive in a CRLF file: {formatted:?}"
        );
    }

    #[test]
    fn normalizing_leaves_an_lf_file_on_lf() {
        let pack = "macros:\n  ping:\n    match: I ping\n    steps:\n      - hurl: |\n          GET http://x/a\n          HTTP 200\n";
        let formatted = normalize_pack(pack);
        assert!(!formatted.contains('\r'), "LF input must stay LF: {formatted:?}");
    }
```

- [ ] **Step 2: Run the tests to verify the CRLF one fails**

Run: `cargo nextest run -p proef normalizing_`
Expected: `normalizing_preserves_a_crlf_file_s_line_endings` FAILS (no `\r\n` in the output); `normalizing_leaves_an_lf_file_on_lf` passes already. Both are needed — the second is what catches a fix that flips the default the wrong way.

- [ ] **Step 3: Detect and preserve the dominant ending**

Add above `normalize_pack` in `crates/proef-cli/src/fmt.rs`:

```rust
/// The line ending to write back: whichever the file already uses. `fmt`
/// normalizes hurl blocks, not line endings — rewriting a CRLF checkout
/// wholesale would leave `fmt --check` permanently dirty for an author who
/// changed nothing.
fn dominant_ending(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    if crlf > lf { "\r\n" } else { "\n" }
}
```

Then at the end of `normalize_pack`, replace the join at `:128-130`:

```rust
    let ending = dominant_ending(text);
    let mut result = out.join(ending);
    result.push_str(ending);
    result
}
```

`text.lines()` already strips the `\r`, so the collected lines are ending-free and rejoining is all that is needed. Confirm that by reading the loop before you change it — if any collected line still carries a `\r`, the join would double it, and that is a finding to report rather than paper over.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p proef normalizing_`
Expected: both PASS.

- [ ] **Step 5: Check the corpus did not move**

Run: `cargo nextest run --profile ci`
The repo's own packs are LF, so no snapshot should move. If one does, stop and report — that would mean the dominant-ending detection is choosing CRLF for an LF file.

- [ ] **Step 6: Run the full gate and commit**

```bash
git add crates/proef-cli/src/fmt.rs docs/CHANGELOG.md
git commit -m "fix(cli): proef fmt keeps the file's own line endings"
```

Changelog entry under `### Fixed`: `proef fmt` rewrote every line ending to LF, beyond its hurl-blocks-only promise, which left `fmt --check` permanently failing on an `autocrlf` checkout. It now preserves the file's dominant ending.

---

### Task 5: `report -o` writes an href that resolves from the output file

**Files:**
- Modify: `crates/proef-cli/src/report.rs` (`artifacts_href` at `:64-74`)

**Interfaces:** none — `artifacts_href` is private.

**The bug, as it stands today.** This is **partly fixed already**: `artifacts_href` correctly splits "the report sits in the run dir" (bare `artifacts`) from "the report is elsewhere". The split is right; the else-branch is not. `runs_dir` defaults to `.proef-runs` (`config.rs:215`) — a **relative** path — and nothing canonicalizes it, so `-o /tmp/out/report.html` bakes `.proef-runs/<id>/artifacts/…`, which the browser resolves against `/tmp/out/`. Every artifact link 404s while the command reports success.

**Do not add a second href mechanism.** Keep `artifacts_href` and make its else-branch absolute.

- [ ] **Step 1: Write the failing test**

Add to `report.rs`'s test module (create one if absent, following a neighbouring module's shape):

```rust
    #[test]
    fn an_out_of_run_dir_report_gets_an_absolute_artifacts_href() {
        // `runs-dir` defaults to a relative `.proef-runs`, and a browser
        // resolves a relative href against the HTML file's own directory —
        // so a report written elsewhere needs an absolute path or every
        // artifact link 404s while the command reports success.
        let record_dir = Path::new(".proef-runs/01ABC");
        let out_path = Path::new("/tmp/elsewhere/report.html");
        let href = artifacts_href(record_dir, out_path);
        assert!(
            Path::new(&href).is_absolute(),
            "href must not be relative to the output file: {href}"
        );
        assert!(href.ends_with("artifacts"), "href must point at the artifacts dir: {href}");
    }

    #[test]
    fn a_report_in_the_run_dir_keeps_the_bare_href() {
        let record_dir = Path::new("/runs/01ABC");
        let out_path = Path::new("/runs/01ABC/report.html");
        assert_eq!(artifacts_href(record_dir, out_path), "artifacts");
    }
```

- [ ] **Step 2: Run the tests to verify the first fails**

Run: `cargo nextest run -p proef artifacts_href`
Expected: `an_out_of_run_dir_report_gets_an_absolute_artifacts_href` FAILS (the href is `.proef-runs/01ABC/artifacts`); `a_report_in_the_run_dir_keeps_the_bare_href` passes already and must keep passing.

- [ ] **Step 3: Make the else-branch absolute**

```rust
fn artifacts_href(record_dir: &Path, out_path: &Path) -> String {
    let out_dir = crate::fsutil::parent_dir(out_path);
    if out_dir == record_dir {
        "artifacts".to_owned()
    } else {
        // A browser resolves a relative href against the HTML file's own
        // directory, and `runs-dir` defaults to a relative `.proef-runs`,
        // so the link has to be absolute to survive `-o` pointing anywhere
        // outside the run dir.
        std::path::absolute(record_dir.join("artifacts"))
            .unwrap_or_else(|_| record_dir.join("artifacts"))
            .display()
            .to_string()
    }
}
```

`std::path::absolute` is stable and does not touch the filesystem, so it stays usable when the run dir has already been rotated away.

**A deliberate divergence from the spec.** D5's text also asks to "compare canonicalized paths so a `./` or symlink spelling does not defeat it" in the if-branch. This plan keeps the plain `==`, because the consequence of the comparison being wrong is now benign: a report that *is* in the run dir but spelled `./` takes the else-branch and gets an absolute href — longer than necessary, and still correct. Canonicalizing would add a filesystem call to a pure function and fail on a rotated-away run dir, trading a cosmetic gain for a real failure mode. If the reviewer disagrees, the fix is small and isolated; it is recorded here so the divergence is a decision rather than an oversight.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p proef artifacts_href`
Expected: both PASS.

- [ ] **Step 5: Verify by probe, not by reading**

Generate a real report outside the run dir and confirm the href resolves. The run dir is `.proef-runs/<id>`; use an existing record or make one, then:

```bash
cargo run -q -p proef -- report -o /tmp/probe-report.html
rg -o 'href="[^"]*artifacts[^"]*"' /tmp/probe-report.html | head -3
```

Expected: an absolute path. Paste the actual output into the report.

- [ ] **Step 6: Run the full gate and commit**

```bash
git add crates/proef-cli/src/report.rs docs/CHANGELOG.md
git commit -m "fix(cli): report -o writes artifact links that resolve"
```

Changelog entry under `### Fixed`: `proef report -o` outside the run dir wrote artifact links relative to the run dir, so every link 404'd from the report's own location while the command reported success. The href is now absolute when the report is written elsewhere.

---

### Task 6: `diff` stops inventing flakiness, and the ordinal caveat is written down

**Files:**
- Modify: `crates/proef-cli/src/diff.rs` (`note_flaky` at `:184-196`)
- Modify: `docs/TROUBLESHOOTING.md` (the `proef diff` paragraph at `:95-108`)

**Interfaces:** none.

**Two problems, two treatments.**

**A brand-new retried step flagged "flaky" is a bug.** `note_flaky` reads the base attempt count with `.map_or(1, |step| step.attempts)`, so a step absent from the base run is treated as having had one attempt — and any retry on it reads as new flakiness. A step with no baseline has no flakiness to report.

**The ordinal shift is inherent and stays.** Steps are keyed `(text, ordinal)`, so removing an earlier duplicate shifts later steps' keys and a comparison can attribute one step's timing to another. That is a property of positional keying, which v0.5.2 chose deliberately over text-only keying (which lost duplicates entirely). Fixing it needs stable per-step identity that the record does not carry. **Document it** rather than shipping a subtly wrong number with no warning.

- [ ] **Step 1: Write the failing test**

`diff.rs` has **no test module** — create one at the end of the file, following the shape used elsewhere in the crate (`#[cfg(test)] mod tests { use super::*; … }`).

Drive the assertion through the real entry point rather than reaching for the private method: `note_flaky` is `impl Report` (`:184`), and `Report` is built by `Report::compute(base, new)` (`:146-155`), which takes two `&BTreeMap<Key, ScenarioRun>`. There is no `Report::default()` — an earlier draft of this plan assumed one.

```rust
    #[test]
    fn a_step_absent_from_the_base_run_is_not_flaky() {
        // `map_or(1, …)` treated "absent from base" as "ran once", so any
        // retry on a brand-new step read as new flakiness. A step with no
        // baseline has no flakiness to report.
        let base = run_with(&[("existing step", 1)]);
        let new = run_with(&[("existing step", 1), ("brand new step", 3)]);
        let report = Report::compute(&base, &new);
        assert!(
            report.flaky.is_empty(),
            "a step with no baseline must not be reported as flaky: {:?}",
            report.flaky
        );
    }
```

Write the smallest `run_with` helper that builds a `BTreeMap<Key, ScenarioRun>` with one scenario whose steps carry the given `(text, attempts)` — read the `Key`, `ScenarioRun`, and step types first and construct them literally. Keep both runs' scenario status identical so the only difference the report can find is the step, otherwise a `regressed`/`added` line could mask what you are testing.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p proef a_step_absent_from_the_base_run`
Expected: FAIL — one flaky line reporting `1→3 attempt(s)` for the brand-new step.

- [ ] **Step 3: Skip steps with no baseline**

In `note_flaky`, replace the `map_or` lookup:

```rust
        for ((text, ord), new_step) in &new.steps {
            // A step with no baseline has no flakiness to report — defaulting
            // to "one attempt" turned every retry on a new step into invented
            // flakiness.
            let Some(base_step) = base.steps.get(&(text.clone(), *ord)) else {
                continue;
            };
            let base_attempts = base_step.attempts;
```

Leave the rest of the loop body unchanged.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p proef a_step_absent_from_the_base_run`
Expected: PASS. Then run `cargo nextest run -p proef diff` and confirm the existing diff tests still pass — one of them may assert on a step that only exists in the new run, and if so its expectation was encoding the bug. If that happens, report it rather than editing the assertion to match.

- [ ] **Step 5: Document the ordinal caveat**

`docs/TROUBLESHOOTING.md:95-108` already explains `proef diff` and carries one deliberate caveat (cancelled runs). Add the second, in that paragraph's voice — prose, not a warning box:

```markdown
`proef diff` keys steps by `(text, ordinal)` so that line shifts don't lie and
repeated steps stay distinct. The trade-off is positional: if a scenario loses
an earlier duplicate of a step, every later instance shifts down one ordinal,
and the comparison lines up two different runs' steps. Timing and attempt
counts for the shifted steps can then be attributed to the wrong one. Renaming
or reordering steps between the two runs being compared is worth a second look
at the numbers; adding or removing steps at the end is not affected.
```

Then run `cargo run -p xtask -- docs-check`.

- [ ] **Step 6: Run the full gate and commit**

```bash
git add crates/proef-cli/src/diff.rs docs/TROUBLESHOOTING.md docs/CHANGELOG.md
git commit -m "fix(cli): diff no longer invents flakiness for a step with no baseline"
```

Changelog entry under `### Fixed`: `proef diff` reported a brand-new retried step as newly flaky, because a step absent from the base run was assumed to have run once. Steps with no baseline are now skipped, and the ordinal-shift caveat inherent to positional step keying is documented in TROUBLESHOOTING.

---

## Sequencing

Tasks 1–6 are independent in behaviour and can land in any order. Two notes:

- **Tasks 1 and 2 both edit `main.rs`** (Task 1 adds `mod envvar;` and changes `active_env`; Task 2 changes the exit funnel). They are executed sequentially, so this is not a conflict — but do not run them as parallel implementers.
- **Every task appends to `docs/CHANGELOG.md` `[Unreleased]`.** Same reasoning: sequential is fine, parallel would collide.

Task 2 is the only one whose behaviour is observable from every other command, so run the full suite after it rather than only its own tests.

Tasks 1 and 2 carry the shared mechanisms and the widest blast radius; 3–6 are local. If reviewer time is limited, spend it on 1 and 2.

## Self-review

**Spec coverage:** D1 → Task 1, D2 → Task 2, D3 → Task 3, D4 → Task 4, D5 → Task 5, D6 → Task 6. The spec's "What is NOT in this branch" section is honoured: no `--dry-run` phase validation, no Ctrl-C teardown, no LSP rooting, no Tier 2/3.

**Deviations from the spec, stated deliberately:**
- **D1 covers five sites, not the four the spec lists.** `lsp.rs:45` reads `PROEF_ENV` in the language server's own startup path. The spec's enumeration was incomplete; the rule is uniform, so the fifth site is in scope.
- D2's spec text says to fold a flag in "exactly as `junit_failed` already is". `junit_failed` is local to `execute()`; `outln!` spans eight modules. Task 2 keeps the *shape* (a flag folded into the exit) and moves the *storage* to a process-level latch read once at `main`'s funnel. Same one-mechanism property, correct scope.
- **D2 does not override exit `130`.** Two interrupt paths hard-exit outside the taxonomy by design (ADR-0009 amendment); a stdout failure must not mask the operator's Ctrl-C.
- D5's spec text was written before a partial fix landed. Task 5 completes `artifacts_href` rather than computing a new href, which the original wording would have invited.
- **D5 keeps the plain `==` comparison** rather than the canonicalized one the spec asks for — the failure mode is now a cosmetically-long-but-correct href, and canonicalizing would add IO to a pure function and break on a rotated-away run dir.

**Second review pass (after the first draft was committed).** Re-reading the draft against the tree found five defects in it, all now fixed above and each of a kind worth naming, because they are the kinds a plan reliably hides:
1. **An incomplete enumeration** — Task 1 covered four of five env sites, because the spec listed four and I trusted it instead of sweeping.
2. **A wrong platform assumption** — `/dev/full` does not exist on macOS, a gate platform.
3. **A wrong type** — Task 6's test called `Diff::default()`; the type is `Report` and it has no `Default`.
4. **An undiscovered bypass** — `process::exit(130)` in two places skips the funnel Task 2 relies on; correct here, but it had to be established rather than assumed.
5. **A missing doc obligation** — exit codes are a contract with a user-facing table, and Task 2 adds a cause to it.

**Placeholder scan:** every step carries the code to write or the exact command to run. Three tasks tell the implementer to read a neighbouring test first and match its scaffolding (`exec.rs` temp files, `diff.rs` `ScenarioRun` constructors, `report.rs` test module) — that is a deliberate instruction to match local convention, not a deferred decision; the assertion in each case is fully specified.

**Type consistency:** `envvar::read` returns `Result<Option<String>, String>` in Task 1's definition and at all four call sites. `render::stdout_failed()` returns `bool` and is consumed as one. `artifacts_href` keeps its `(&Path, &Path) -> String` signature.

**Known risk:** Task 1 changes `active_env`'s signature, so the compiler will surface callers the plan has not enumerated. That is intentional — the instruction is to follow the compiler and report any caller that cannot propagate a `Result`, rather than to reintroduce a silent `.ok()`.

**Verified while writing this plan** (so the implementer does not re-derive, and knows what was actually checked rather than assumed):
- `/dev/full` **does not exist on macOS** — an earlier draft of Task 2 used it unconditionally and would have failed on the `gates (macos-latest)` job. Three portable substitutes were tried and none forces a write failure: a read-only fd as stdout exits `0`, a closed stdout (`1>&-`) exits `0`, and a directory as stdout is rejected by the shell before the process starts. Hence the split into a portable mechanism test plus a Linux-gated end-to-end test.
- `exec.rs` has a test module at `:963`; `diff.rs` and `report.rs` do **not** — Tasks 5 and 6 create theirs.
- `tempfile` is already a `proef-cli` dev-dependency (`Cargo.toml:53`), so Task 3 needs no new dependency.
- `runs_dir` defaults to the relative `.proef-runs` at `config.rs:215` — this is what makes Task 5's else-branch wrong.
- `fmt.rs:128-130` rejoins with `out.join("\n")` then pushes `'\n'`, which is the whole of Task 4's bug.
- `main()` has a single exit funnel at `main.rs:450`, which is what makes Task 2's one-read design possible.
