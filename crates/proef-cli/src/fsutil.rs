//! Small filesystem helpers shared by commands that rewrite user files.

use std::path::Path;

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

fn sibling_tmp(path: &Path) -> std::path::PathBuf {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    std::path::PathBuf::from(tmp)
}

/// Is this directory name a proef run id (uuid)? Shared by run rotation and
/// `explain`'s latest-run lookup so the two can never diverge on what counts
/// as a run dir.
pub fn is_run_id(name: &str) -> bool {
    uuid::Uuid::try_parse(name).is_ok()
}
