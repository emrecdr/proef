//! Small filesystem helpers shared by commands that rewrite user files.

use std::path::Path;

/// Write `contents` to `path` via a process-unique sibling temp file and an
/// atomic rename: an interrupt mid-write must never leave a user's file
/// truncated or half-written.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}
