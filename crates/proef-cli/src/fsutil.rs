//! Small filesystem helpers shared by commands that rewrite user files.

use std::path::{Path, PathBuf};

/// Write `contents` to `path` via a process-unique sibling temp file and an
/// atomic rename: an interrupt mid-write must never leave a user's file
/// truncated or half-written.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let tmp = sibling_tmp(path);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

/// [`write_atomic`] for sensitive files: on unix the temp file is *opened*
/// with mode `0600` so the content is private from the first byte, and the
/// target is re-chmodded after the rename (the mode at open only applies on
/// creation, not to a reused temp file).
pub(crate) fn write_atomic_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let tmp = sibling_tmp(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(&tmp)?.write_all(contents.as_bytes())?;
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// `path` with `suffix` appended to its **file name** — the sibling naming every
/// derived file here uses (`.tmp`, `.lock`, `.corrupt`).
///
/// Appending to the `OsStr` rather than reaching for `with_extension` (which
/// would replace `.json`, not extend it) or `join` (which would make the sidecar
/// a *child* of the store). Both spellings existed separately, and a sidecar
/// that lands in the wrong place is exactly the bug the config-relative path
/// rule was fixing.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// The process-unique temp sibling `write_atomic` renames from. Per-process so
/// two concurrent proefs writing the same file cannot share a scratch path.
fn sibling_tmp(path: &Path) -> PathBuf {
    with_suffix(path, &format!(".{}.tmp", std::process::id()))
}

/// Is this directory name a proef run id (uuid)? Shared by run rotation and
/// `explain`'s latest-run lookup so the two can never diverge on what counts
/// as a run dir.
pub fn is_run_id(name: &str) -> bool {
    // proef only ever writes the hyphenated form. `Uuid::try_parse` also
    // accepts bare 32-hex, `urn:uuid:…` and braced spellings — and rotation
    // deletes the oldest run-shaped directories, so breadth here is a deletion
    // hazard when the runs dir points somewhere shared.
    name.len() == 36 && uuid::Uuid::try_parse(name).is_ok()
}

/// Create the directories `path`'s file needs, so writing it can succeed.
///
/// An output path the user named is a path proef was asked to write, and every
/// comparable runner treats the parents as part of that request: `pytest
/// --junitxml`, `jest-junit`, `cargo-nextest`'s `JUnit` store, and the `hurl`
/// proef embeds all create them. proef created them for `artifacts -o` and the
/// run directory but not for `--junit`, `--sarif` or `report -o` — no rule, just
/// four sites deciding separately, with the two used most in CI on the failing
/// side. Every adopter paid the same `mkdir -p`.
///
/// This does not weaken clig.dev's "side effects should be explicit": that rule
/// is about writing files the user *did not* pass. Here they passed exactly this
/// path.
pub(crate) fn create_parents(path: &Path) -> std::io::Result<()> {
    let parent = parent_dir(path);
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(parent)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn only_hyphenated_uuid_dirs_count_as_run_ids() {
        // Rotation deletes the oldest run-shaped directories beyond the
        // retention limit, and the runs dir can point somewhere shared — so
        // "run-shaped" must mean the hyphenated form proef actually writes,
        // not every spelling the uuid parser accepts.
        assert!(is_run_id("0198f3c1-0000-7000-8000-00000000001a"));
        assert!(!is_run_id("0198f3c100007000800000000000001a"));
        assert!(!is_run_id("urn:uuid:0198f3c1-0000-7000-8000-00000000001a"));
        assert!(!is_run_id("{0198f3c1-0000-7000-8000-00000000001a}"));
        assert!(!is_run_id("cache-abc"));
    }

    #[test]
    fn parent_dir_normalizes_a_bare_filename_to_cwd() {
        // A bare filename has an EMPTY parent (Some("")), which is not a usable
        // directory — it must resolve to "." (cwd), like an explicit "./name".
        assert_eq!(parent_dir(Path::new("setup.feature")), PathBuf::from("."));
        assert_eq!(parent_dir(Path::new("x")), PathBuf::from("."));
        assert_eq!(parent_dir(Path::new("./setup.feature")), PathBuf::from("."));
        // A real directory component is preserved unchanged (cross-platform).
        assert_eq!(
            parent_dir(Path::new("suite/setup.feature")),
            PathBuf::from("suite")
        );
        // Absolute path preserved (POSIX-only assertion — Windows abs paths differ).
        #[cfg(unix)]
        assert_eq!(
            parent_dir(Path::new("/abs/x.feature")),
            PathBuf::from("/abs")
        );
    }
}
