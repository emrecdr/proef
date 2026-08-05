# proef — bare-filename path resolution fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix a bare-filename path (no directory component) failing with "cannot read directory " (and passing an empty context directory downstream), because `Path::parent()` returns `Some("")` for a bare filename and several sites treat that empty path as a usable directory.

**Architecture:** One shared `fsutil::parent_dir` helper normalizes the empty/None parent to `"."` (cwd), applied at the four `proef-cli` sites that derive a directory from a file path. proef-core untouched. Full re-verified mechanisms in `docs/superpowers/specs/2026-08-05-setup-bare-filename-path-design.md` "Verified facts".

**Tech Stack:** Rust 2024; existing `assert_cmd`/`tempfile`/`Fixture` test harnesses.

**Branch:** `fix/setup-bare-filename-path` off `main` (5c36a5e = v0.5.0). Its own small patch PR, independent of PRs #4/#5/#6.

## Global Constraints

- proef-core untouched (this is proef-cli only). No new dependencies.
- **NO behavior change for any currently-working path**: directory paths (the `is_dir()` branch), paths with a directory component (`suite/x` → `suite`), and absolute paths (`/abs/x` → `/abs`) are all unchanged — only bare filenames change (`""` → `"."` = cwd), matching how `./x.feature` already resolves.
- No task ids / plan numbers in code comments. No AI-attribution commit trailers.
- Both regression tests must genuinely FAIL without the fix (hold the bar — earlier passes caught vacuous tests).
- The workspace package/binary is `proef` (use `cargo … -p proef`, `assert_cmd::cargo::cargo_bin("proef")`).
- Ships as a patch (rides the next release cut).
- **Gate** (all pass before commit): `cargo fmt --all --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo nextest run --profile ci`; `cargo test --doc`; `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace`; `cargo run -p xtask -- docs-check`.

## Verified facts (from the spec — cite, don't re-derive)

- `Path::new("setup.feature").parent()` → `Some("")` (empty, not None); `"x"` → `Some("")`; while `"./setup.feature"` → `Some(".")`, `"suite/setup.feature"` → `Some("suite")`, `"/abs/x.feature"` → `Some("/abs")` (all non-empty). Verified by direct `rustc` execution.
- Four sites derive a directory from `file_path.parent()`: `front.rs project_packs` (hard-fails `read_dir("")` early in `front::run`); `exec.rs:749` (`file_root: Option<PathBuf>`, the engine context dir); `exec.rs:728` (asset copy, run record — warning on failure); `commands.rs:413` (`artifacts` asset copy — hard error on failure).
- `copy_assets(hurl_text: &str, source_root: &Path, dest_root: &Path)` (assets.rs:33) — takes `&Path`, so `&root` (a `&PathBuf`) works.
- `file_root: Option<std::path::PathBuf>` (engine.rs:209 / runner.rs:39), cloned into the runner as the context dir (runner.rs:503) — never `None` for a real feature path today, so always-`Some` is behavior-preserving.
- `fsutil.rs` imports only `std::path::Path` (needs `PathBuf` added); it's the shared FS-helper home, already imported by `commands.rs`/`exec.rs` via `crate::fsutil::`.

---

### Task 1: `fsutil::parent_dir` + four call sites + regression tests

**Files:**
- Modify: `crates/proef-cli/src/fsutil.rs` (add `parent_dir` + unit test; add `PathBuf` import)
- Modify: `crates/proef-cli/src/front.rs:300-306` (`project_packs` else branch)
- Modify: `crates/proef-cli/src/exec.rs:749` (`file_root`) and `crates/proef-cli/src/exec.rs:728` (asset copy)
- Modify: `crates/proef-cli/src/commands.rs:413` (`artifacts` asset copy)
- Test: `crates/proef-cli/tests/execute.rs` (bare-filename setup integration test)

**Interfaces:**
- Produces: `pub(crate) fn parent_dir(path: &std::path::Path) -> std::path::PathBuf` in `crate::fsutil`.
- Consumes: `crate::assets::copy_assets(&str, &Path, &Path) -> Result<(), AssetCopyError>`; `ScenarioSpec.file_root: Option<PathBuf>`.

- [ ] **Step 1: Write the helper unit test (RED).** In `fsutil.rs`, add (or extend) a test module at the end:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn parent_dir_normalizes_a_bare_filename_to_cwd() {
        // A bare filename has an EMPTY parent (Some("")), which is not a usable
        // directory — it must resolve to "." (cwd), like an explicit "./name".
        assert_eq!(parent_dir(Path::new("setup.feature")), PathBuf::from("."));
        assert_eq!(parent_dir(Path::new("x")), PathBuf::from("."));
        assert_eq!(parent_dir(Path::new("./setup.feature")), PathBuf::from("."));
        // A real directory component is preserved unchanged (cross-platform).
        assert_eq!(parent_dir(Path::new("suite/setup.feature")), PathBuf::from("suite"));
        // Absolute path preserved (POSIX-only assertion — Windows abs paths differ).
        #[cfg(unix)]
        assert_eq!(parent_dir(Path::new("/abs/x.feature")), PathBuf::from("/abs"));
    }
}
```

- [ ] **Step 2: Run it (verify RED).** `cargo nextest run -p proef parent_dir_normalizes_a_bare_filename_to_cwd`. Expected: FAIL to compile — `parent_dir` does not exist yet (the type-change RED).

- [ ] **Step 3: Add the helper.** In `fsutil.rs`, change the import line `use std::path::Path;` to `use std::path::{Path, PathBuf};`, and add:

```rust
/// The directory a file path lives in, for deriving a search/context base.
/// `Path::parent()` returns `Some("")` for a bare filename (no directory
/// component) — an empty path that is not a usable directory — so normalize
/// both the empty and `None` cases to `.` (the current directory), matching
/// how an explicit `./name` already resolves.
pub(crate) fn parent_dir(path: &Path) -> PathBuf {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}
```

- [ ] **Step 4: Run the unit test (verify GREEN).** `cargo nextest run -p proef parent_dir_normalizes_a_bare_filename_to_cwd`. Expected: PASS.

- [ ] **Step 5: Write the integration test (RED).** In `crates/proef-cli/tests/execute.rs`, add a test that a bare-filename `[run] setup` at the project root works. **First read an existing setup test in that file** (e.g. `single_file_setup_still_runs_once` from the v0.5.2 pass, or `setup_shares_globals_teardown…`) to copy the exact `Fixture::start()` + `proef_in` + fixture pack/feature layout that binds cleanly. Then:

```rust
#[test]
fn bare_filename_setup_at_project_root_resolves_packs() {
    let fixture = Fixture::start(); // real `test` runs the setup feature (HTTP)
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Suite with one ordinary feature + a pack so its step binds.
    std::fs::create_dir_all(root.join("suite/packs")).unwrap();
    std::fs::write(root.join("suite/main.feature"),
        "Feature: M\n  Scenario: S\n    When I noop\n").unwrap();
    std::fs::write(root.join("suite/packs/p.yaml"),
        "macros:\n  noop:\n    match: \"I noop\"\n    steps:\n      - hurl: |\n          GET {{BASE}}/health\n").unwrap();
    // Setup feature at the PROJECT ROOT — a BARE filename, no directory prefix.
    std::fs::write(root.join("setup.feature"),
        "Feature: Setup\n  Scenario: SU\n    When I noop\n").unwrap();
    std::fs::write(root.join("proef.toml"),
        "[run]\nsuite = \"suite\"\nsetup = \"setup.feature\"\n").unwrap();

    // Pre-fix: project_packs derives base = "" from the bare filename's empty
    // parent → read_dir("") → "cannot read directory ". Post-fix: base = "." (cwd).
    assert_cmd::Command::cargo_bin("proef").unwrap()
        .current_dir(root)
        .env("BASE", fixture.base_url())      // match the fixture-url env the existing tests use
        .args(["test"])
        .assert()
        .stderr(predicates::str::contains("cannot read directory").not());
}
```

**Adapt to the real fixture harness:** the `Fixture`/`proef_in`/base-url env name (`BASE` above is a placeholder — use whatever the existing `execute.rs` setup tests use, e.g. `PROEF_BASE_URL` or the pack's `${url:…}` convention) and the pack's request line must match a working existing test so the setup + suite actually bind and run. The load-bearing assertion is `stderr does NOT contain "cannot read directory"`; strengthen to `.code(0)` if the fixture reliably makes the run pass (mirror the existing test's final assertion). Import `predicates::prelude::*` (or `use predicates::str::PredicateStrExt` for `.not()`) as the file already does for other tests.

- [ ] **Step 6: Run it (verify RED).** `cargo nextest run -p proef bare_filename_setup_at_project_root_resolves_packs`. Expected: FAIL — the run aborts with "cannot read directory " (project_packs' `read_dir("")`), so the `.not()` stderr assertion fails.

- [ ] **Step 7: Swap the four call sites.**

**7a — `front.rs` `project_packs` (lines 300-306):** change the `else` branch:

```rust
    let base = if path.is_dir() {
        path.to_path_buf()
    } else {
        crate::fsutil::parent_dir(path)
    };
```

**7b — `exec.rs:749` (`file_root` in `build_specs`):**

```rust
                file_root: Some(crate::fsutil::parent_dir(Path::new(feature.file.path.as_str()))),
```

(`file_root` is `Option<PathBuf>`; a real feature path's parent is always `Some`, so `Some(parent_dir(...))` preserves behavior for real inputs — a bare-filename feature now yields `Some(".")` not `Some("")`.)

**7c — `exec.rs:728` (asset copy, run record):** replace the `if let Some(root) = …parent() && …` with:

```rust
                    let root = crate::fsutil::parent_dir(Path::new(feature_file.path.as_str()));
                    if let Err(err) =
                        crate::assets::copy_assets(&artifact.hurl_text, &root, &artifacts_dir)
                    {
                        eprintln!("warning: run record for {}.hurl: {err}", artifact.slug);
                    }
```

**7d — `commands.rs:413` (`artifacts` asset copy):** replace the `if let Some(root) = …parent() && …` with (keeping the existing error-return arms):

```rust
            let root = crate::fsutil::parent_dir(Path::new(feature.file.path.as_str()));
            if let Err(err) = crate::assets::copy_assets(&artifact.hurl_text, &root, out_dir) {
                eprintln!("error: {}.hurl: {err}", artifact.slug);
                return match err {
                    crate::assets::AssetCopyError::Unsafe(_) => ExitCode::UserError,
                    crate::assets::AssetCopyError::Io(_) => ExitCode::SystemError,
                };
            }
```

- [ ] **Step 8: Run the integration test + full suite (verify GREEN).** `cargo nextest run -p proef bare_filename_setup_at_project_root_resolves_packs` → PASS (no "cannot read directory"). Then `cargo nextest run --profile ci` → all pass; the existing `project_packs`/artifacts/setup tests are unchanged (paths with directory components are untouched by `parent_dir`).

- [ ] **Step 9: Full gate + commit.** Run every gate command. Then:

```bash
git add crates/proef-cli/src/fsutil.rs crates/proef-cli/src/front.rs crates/proef-cli/src/exec.rs crates/proef-cli/src/commands.rs crates/proef-cli/tests/execute.rs
git commit -m "fix(cli): resolve packs/assets from cwd for a bare-filename path

Path::parent() returns Some(\"\") for a bare filename (no directory
component), so deriving a base directory from it produced an empty path
that read_dir rejected (\"cannot read directory \") — and passed \"\" as an
asset/context root downstream. Normalize the empty (and None) parent to
\".\" via a shared fsutil::parent_dir helper, applied at the four
base-derivation sites."
```

---

## Self-review

**Spec coverage:** the helper (spec "Design") → Step 3; the four call sites (spec's four verified sites) → Steps 7a-7d; the unit test → Steps 1-4; the integration test → Steps 5-6, 8. Full coverage.

**Placeholder scan:** the integration test's fixture wiring (base-url env name, pack request line) is deliberately marked "adapt to the real harness — mirror an existing execute.rs setup test", because the exact `Fixture`/env convention must match a working sibling test verbatim; the load-bearing assertion (`stderr !contains "cannot read directory"`) and the bare-filename `proef.toml` are concrete. That's a "match the real fixture", not missing content. No TBD/TODO.

**Type consistency:** `parent_dir(&Path) -> PathBuf` (Step 3) is called with `&root` into `copy_assets(_, &Path, _)` (Steps 7c/7d — `&PathBuf` derefs to `&Path`) and wrapped `Some(parent_dir(...))` into `file_root: Option<PathBuf>` (Step 7b). Consistent.
