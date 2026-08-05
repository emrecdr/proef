# proef — bare-filename path resolution fix (design)

**Goal:** Fix a confirmed pre-existing bug where a bare-filename path (no directory
component) — e.g. `[run] setup = "setup.feature"` at project root, or
`proef test somefile.feature` from that file's own directory — fails with a
confusing "cannot read directory " error (or silently passes an empty context
directory downstream), because `Path::parent()` returns `Some("")` for a bare
filename and several sites treat that empty path as a usable directory.

**Architecture:** One-bug fix in `proef-cli`. A single shared helper normalizes the
empty/None parent to `"."` (current directory), applied at the four sites that
derive a directory from a file path. `proef-core` untouched.

**Tech stack:** Rust 2024; existing `assert_cmd`/`tempfile` test harnesses.

**Branch:** `fix/setup-bare-filename-path` off `main` (5c36a5e = v0.5.0). Independent
of PRs #4/#5/#6 (front.rs is untouched by all three; the exec.rs/commands.rs call
sites are on different lines than PR #6's edits). Its own small patch PR.

---

## Verified facts (confirmed against current source)

- **Root cause.** `Path::new("setup.feature").parent()` returns `Some("")` (an empty
  path), NOT `None`. Confirmed by direct execution: `setup.feature` → `Some("")`,
  `x` → `Some("")`; while `./setup.feature` → `Some(".")`, `suite/x.feature` →
  `Some("suite")`, `/abs/x.feature` → `Some("/abs")` are all non-empty.
- **The hard-failing site.** `front.rs` `project_packs` (front.rs:300-306) derives the
  pack-search base as `path.parent().map_or_else(|| PathBuf::from("."), …)` — the
  `"."` fallback only fires on `None`, so a bare filename yields `base = ""`, and
  `pack_files("")` → `walk_dir("")` → `read_dir("")` fails "cannot read directory ".
  This fails *early* in `front::run`, so today a bare-filename path never reaches the
  sibling sites below.
- **Three sibling sites, same `file_path.parent()` pattern**, reachable once
  `project_packs` is fixed:
  - `exec.rs:749` — `file_root: Path::new(feature.file.path).parent().map(to_path_buf)`
    → `Some("")` passed to the engine as the context dir.
  - `exec.rs:728` — `if let Some(root) = …parent()` → `copy_assets(…, root="", …)`;
    failure here is a *warning* (incomplete run record).
  - `commands.rs:413` — `if let Some(root) = …parent()` → `copy_assets(…, root="", …)`
    in the `artifacts` command; failure here is a *hard error*.
- **Home for the helper.** `fsutil.rs` ("Small filesystem helpers shared by
  commands") — already imported by `commands.rs` and `exec.rs` via `crate::fsutil::`.

## Design

Add to `crates/proef-cli/src/fsutil.rs`:

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

Apply it at the four sites:

- **front.rs `project_packs`**: the `else` branch becomes `fsutil::parent_dir(path)`
  (the `if path.is_dir()` branch is unchanged).
- **exec.rs:749** (`file_root`): `Some(fsutil::parent_dir(Path::new(feature.file.path.as_str())))`
  — note `file_root` is an `Option<PathBuf>`; it becomes `Some(parent_dir(...))` (a bare
  filename now yields `Some(".")` instead of `Some("")`; it is never `None` today because
  `parent()` on a non-empty path string is always `Some`, so the type/behavior for real
  paths is preserved).
- **exec.rs:728** (asset copy, run record): replace `if let Some(root) = …parent()` with
  `let root = fsutil::parent_dir(...);` then the `copy_assets` call unconditionally (the
  `Some` guard was only skipping the `None` case, which no longer applies — `parent_dir`
  always yields a dir).
- **commands.rs:413** (artifacts asset copy): same shape as exec.rs:728 —
  `let root = fsutil::parent_dir(...); if let Err(err) = copy_assets(…, &root, …) { … }`.

Behavior is unchanged for every currently-working path (directory paths → the
`is_dir()` branch; paths with a directory component or absolute paths → non-empty
parent, untouched). Only bare filenames change: from `""` (broken/empty) to `"."`
(cwd) — the correct base for a file addressed by bare name in the current directory.

## Testing

- **Unit test (fsutil.rs)** — `parent_dir` over the cases pinned by the rustc check:
  `"setup.feature"` → `"."`, `"x"` → `"."`, `"./setup.feature"` → `"."`,
  `"suite/setup.feature"` → `"suite"`, `"/abs/x.feature"` → `"/abs"`. Genuinely fails
  against the old `map_or_else(|| ".", …)` (which would give `""` for the bare cases).
- **Integration test (tests/execute.rs)** — assert_cmd: a project-root `proef.toml` with
  a bare-filename `[run] setup = "setup.feature"` (the setup file at the project root,
  NOT under `suite/`) runs without the "cannot read directory" failure. This is the exact
  case the v0.5.2 §3.4 test had to route around by placing the setup under `suite/`; it
  fails without this fix. Needs the fixture server (real setup runs) — mirror an existing
  `execute.rs` setup test's `Fixture::start()` pattern.

## Constraints

- `proef-core` untouched (`proef-cli` only). No new dependencies.
- No behavior change for any currently-working path (directory, with-directory-component,
  or absolute) — the helper only rescues the empty-parent (bare-filename) case.
- No task ids / plan numbers in code comments; no AI-attribution commit trailers.
- The regression tests must genuinely fail without the fix.
- Ships as a patch (rides the next release cut; own PR now).

## Task shape (for the plan)

A single logical change, testable as one unit: (1) add `fsutil::parent_dir` + its unit
test; (2) swap the four call sites; (3) the bare-filename integration test; (4) gate. One
task, TDD (unit test RED → helper → integration test RED → call-site swaps → GREEN), one
commit.
