//! Copying `file,…;` assets referenced by an artifact next to it, so the
//! emitted `.hurl` replays under stock `hurl --test` without proef's context
//! root (ADR-0010 hand-off).

use std::path::{Component, Path};

/// Why an asset copy stopped: a reference the user must fix, or a
/// filesystem failure — the split mirrors the exit-code contract (2 vs 3).
#[derive(Debug)]
pub(crate) enum AssetCopyError {
    /// The payload references a path outside the suite.
    Unsafe(String),
    /// The filesystem refused the copy.
    Io(String),
}

impl std::fmt::Display for AssetCopyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsafe(message) | Self::Io(message) => f.write_str(message),
        }
    }
}

/// Copy every `file,…;` asset referenced by `hurl_text` from `source_root`
/// into `dest_root`, preserving the relative layout.
///
/// Asset references are user-controlled text: absolute paths and any `..` or
/// prefix component are rejected before touching the filesystem (they would
/// escape `dest_root`), and a source that already *is* the destination is
/// left alone — `fs::copy` opens the destination with truncate first, so
/// copying a file onto itself destroys it.
pub(crate) fn copy_assets(
    hurl_text: &str,
    source_root: &Path,
    dest_root: &Path,
) -> Result<(), AssetCopyError> {
    for asset in proef_core::emit::file_references(hurl_text) {
        let reference = Path::new(&asset);
        let escapes = reference
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir));
        if escapes {
            return Err(AssetCopyError::Unsafe(format!(
                "asset reference `{asset}` is not a plain relative path — only files inside the suite are copied"
            )));
        }
        let source = source_root.join(reference);
        if !source.is_file() {
            continue;
        }
        let target = dest_root.join(reference);
        if let (Ok(from), Ok(to)) = (source.canonicalize(), target.canonicalize())
            && from == to
        {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                AssetCopyError::Io(format!("cannot create {}: {err}", parent.display()))
            })?;
        }
        std::fs::copy(&source, &target).map_err(|err| {
            AssetCopyError::Io(format!(
                "cannot copy asset `{asset}` to {}: {err}",
                target.display()
            ))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const REFERENCING: &str = "POST http://example.test/upload\nfile,data.bin;\nHTTP 200\n";

    #[test]
    fn same_file_is_never_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("data.bin"), b"332 bytes stand-in").unwrap();
        copy_assets(REFERENCING, root, root).unwrap();
        assert_eq!(
            std::fs::read(root.join("data.bin")).unwrap(),
            b"332 bytes stand-in"
        );
    }

    #[test]
    fn parent_escapes_are_rejected_before_any_copy() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("out")).unwrap();
        std::fs::write(root.join("victim.txt"), b"IMPORTANT-USER-DATA").unwrap();
        let text = "POST http://example.test/upload\nfile,../victim.txt;\nHTTP 200\n";
        let err = copy_assets(text, &root.join("out"), &root.join("out")).unwrap_err();
        assert!(matches!(err, AssetCopyError::Unsafe(_)));
        assert_eq!(
            std::fs::read(root.join("victim.txt")).unwrap(),
            b"IMPORTANT-USER-DATA"
        );
    }

    #[test]
    fn absolute_references_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let text = "POST http://example.test/upload\nfile,/etc/hosts;\nHTTP 200\n";
        let err = copy_assets(text, dir.path(), dir.path()).unwrap_err();
        assert!(matches!(err, AssetCopyError::Unsafe(_)));
    }

    #[test]
    fn plain_relative_assets_copy_with_layout() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("suite/payloads")).unwrap();
        std::fs::create_dir(root.join("out")).unwrap();
        std::fs::write(root.join("suite/payloads/data.bin"), b"bytes").unwrap();
        let text = "POST http://example.test/upload\nfile,payloads/data.bin;\nHTTP 200\n";
        copy_assets(text, &root.join("suite"), &root.join("out")).unwrap();
        assert_eq!(
            std::fs::read(root.join("out/payloads/data.bin")).unwrap(),
            b"bytes"
        );
    }
}
