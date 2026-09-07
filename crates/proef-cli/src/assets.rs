//! Staging the `file,…;` assets an artifact reads into that scenario's own
//! asset root, which is then the engine's `--file-root` and the root a replay
//! under stock `hurl --test` uses (ADR-0010 hand-off).
//!
//! Two rules make this more than a copy. **Each asset resolves against the
//! directory of the source that referenced it** — beside the feature for an
//! inline `hurl:` block, beside the fragment for a `ref:` (ADR-0018, matching
//! hurl's own per-file `--file-root` default). And **the root is per
//! scenario**, so two features that each keep a `data.json` no longer stage
//! over one another.

use proef_core::emit::AssetRef;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// Why staging stopped: a reference the user must fix, or a filesystem
/// failure — the split mirrors the exit-code contract (2 vs 3).
#[derive(Debug)]
pub(crate) enum AssetCopyError {
    /// The payload references a path proef will not follow, or two sources
    /// claim one name.
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

/// Where each kind of source keeps its assets.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AssetRoots<'a> {
    /// Directory the feature file was **read from** — the root for inline
    /// `hurl:` blocks. An IO path (`LoadedFeature::read_from`'s parent),
    /// never the portable display name: a name's anchor (project root, or
    /// the caller's own typed spelling) is not in the string, so resolving
    /// one against the working directory breaks from any subdirectory.
    pub feature: &'a Path,
    /// The project root (the directory holding `proef.toml`), which is what
    /// a fragment's recorded name is relative to. `None` when no config is in
    /// scope — the state the config-independent reference corpus runs in,
    /// where names arrive already usable from the working directory.
    pub project: Option<&'a Path>,
}

impl AssetRoots<'_> {
    /// The directory `asset` resolves against, from the source that wrote it.
    ///
    /// A fragment is qualified `file.hurl#name`, and the file half carries the
    /// name the *naming boundary* gave it: project-root relative, or left
    /// absolute when the corpus lies outside the project. Both are handled by
    /// one join — `Path::join` lets an absolute right-hand side win — so the
    /// out-of-project corpus needs no branch of its own.
    fn source_dir(&self, asset: &AssetRef) -> PathBuf {
        let Some(fragment) = &asset.fragment else {
            return self.feature.to_path_buf();
        };
        // The reader that lives beside the writer (`Fragment::qualified`), so
        // the separator stays one decision rather than three.
        let (file, _) = proef_core::pack::split_qualified(fragment);
        let file = file.unwrap_or(fragment);
        let anchored = self
            .project
            .map_or_else(|| PathBuf::from(file), |root| root.join(file));
        crate::fsutil::parent_dir(&anchored)
    }
}

/// Stage every asset into `dest_root`, each from the directory its own source
/// referenced it from.
///
/// Asset references are user-controlled text: absolute paths and any `..` or
/// prefix component are rejected before touching the filesystem. A missing
/// source is an **error**, not a skip — this root is what the engine reads
/// during the run, so a quietly absent file becomes a failing request whose
/// message blames the author for a path that was correct.
///
/// A *source* that is a symlink is followed, deliberately: stock `hurl`
/// follows it too, and refusing would make the same bytes pass under one
/// runner and fail under the other — the divergence class ADR-0018's
/// dual-runner rule exists to prevent. The *destination* is proef's own
/// directory, where a symlink has no legitimate author: writing through one
/// would land bytes outside the asset root, so an existing link is removed
/// before the copy.
pub(crate) fn stage_assets(
    assets: &[AssetRef],
    roots: AssetRoots<'_>,
    dest_root: &Path,
) -> Result<(), AssetCopyError> {
    // One staged name may come from only one place. Two sources claiming it
    // would resolve to whichever was copied last — the silent-wrong-bytes
    // failure the per-scenario root exists to end, so it is refused rather
    // than narrowed.
    let mut claimed: BTreeMap<&str, PathBuf> = BTreeMap::new();
    // The same refusal for names the *filesystem* will not keep apart:
    // `Data.json` and `data.json` are two keys above but one file on a
    // case-insensitive volume (macOS, Windows), and NFC/NFD spellings of one
    // name likewise converge — so the check is on the canonical path the
    // copy actually landed on, which is exact on every platform without
    // guessing the volume's rules.
    let mut landed: BTreeMap<PathBuf, String> = BTreeMap::new();
    for asset in assets {
        let name = asset.name.as_str();
        let reference = Path::new(name);
        let escapes = reference
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir));
        if escapes {
            return Err(AssetCopyError::Unsafe(format!(
                "asset reference `{name}` is not a plain relative path — proef stages only files beside the feature or the fragment that names them"
            )));
        }
        let source = roots.source_dir(asset).join(reference);
        if let Some(previous) = claimed.get(name)
            && previous != &source
        {
            return Err(AssetCopyError::Unsafe(format!(
                "two sources in this scenario both supply `{name}` ({} and {}) — rename one, or the staged file would be whichever was copied last",
                previous.display(),
                source.display()
            )));
        }
        claimed.insert(name, source.clone());
        if !source.is_file() {
            return Err(AssetCopyError::Unsafe(format!(
                "asset `{name}` is not readable at {} — a `file,…;` body resolves beside the {} that names it",
                source.display(),
                if asset.fragment.is_some() {
                    "fragment"
                } else {
                    "feature"
                }
            )));
        }
        let target = dest_root.join(reference);
        // `fs::copy` truncates the destination first, so copying a file onto
        // itself destroys it.
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
        // Never write *through* a pre-existing symlink in the staging root:
        // `fs::copy` follows it and the bytes land wherever it points —
        // outside the very root that exists to contain them. Nothing
        // legitimate puts links here (the root is proef-owned and staged
        // fresh), so one found is removed, not honoured.
        if target
            .symlink_metadata()
            .is_ok_and(|meta| meta.file_type().is_symlink())
        {
            std::fs::remove_file(&target).map_err(|err| {
                AssetCopyError::Io(format!(
                    "cannot replace symlink at {}: {err}",
                    target.display()
                ))
            })?;
        }
        std::fs::copy(&source, &target).map_err(|err| {
            AssetCopyError::Io(format!(
                "cannot stage asset `{name}` to {}: {err}",
                target.display()
            ))
        })?;
        // Post-copy, the canonical destination says which file the platform
        // actually used; two reference spellings converging on it is the
        // last-writer-wins overwrite the raw-string map above cannot see.
        if let Ok(canonical) = target.canonicalize() {
            if let Some(previous) = landed.get(&canonical)
                && previous != name
            {
                return Err(AssetCopyError::Unsafe(format!(
                    "asset references `{previous}` and `{name}` are one file on this filesystem ({}) — rename one, or the staged bytes would be whichever was copied last",
                    canonical.display()
                )));
            }
            landed.insert(canonical, name.to_owned());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// An inline `hurl:` block's asset — no fragment, so it resolves beside
    /// the feature.
    fn inline(name: &str) -> AssetRef {
        AssetRef {
            name: name.to_owned(),
            fragment: None,
        }
    }

    /// A `ref:` step's asset, qualified as the record spells it.
    fn from_fragment(name: &str, fragment: &str) -> AssetRef {
        AssetRef {
            name: name.to_owned(),
            fragment: Some(fragment.to_owned()),
        }
    }

    fn roots<'a>(feature: &'a Path, project: Option<&'a Path>) -> AssetRoots<'a> {
        AssetRoots { feature, project }
    }

    #[test]
    fn same_file_is_never_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("data.bin"), b"332 bytes stand-in").unwrap();
        stage_assets(&[inline("data.bin")], roots(root, None), root).unwrap();
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
        let out = root.join("out");
        let err = stage_assets(&[inline("../victim.txt")], roots(&out, None), &out).unwrap_err();
        assert!(matches!(err, AssetCopyError::Unsafe(_)));
        assert_eq!(
            std::fs::read(root.join("victim.txt")).unwrap(),
            b"IMPORTANT-USER-DATA"
        );
    }

    #[test]
    fn absolute_references_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let err =
            stage_assets(&[inline("/etc/hosts")], roots(dir.path(), None), dir.path()).unwrap_err();
        assert!(matches!(err, AssetCopyError::Unsafe(_)));
    }

    #[test]
    fn plain_relative_assets_copy_with_layout() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("suite/payloads")).unwrap();
        std::fs::create_dir(root.join("out")).unwrap();
        std::fs::write(root.join("suite/payloads/data.bin"), b"bytes").unwrap();
        stage_assets(
            &[inline("payloads/data.bin")],
            roots(&root.join("suite"), None),
            &root.join("out"),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(root.join("out/payloads/data.bin")).unwrap(),
            b"bytes"
        );
    }

    /// The rule ADR-0018 needs: a fragment's asset comes from beside the
    /// *fragment*, which is the directory stock `hurl` would read it from —
    /// not from beside the feature, which is a different tree entirely.
    #[test]
    fn a_fragment_asset_is_staged_from_beside_the_fragment() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("suite")).unwrap();
        std::fs::create_dir_all(root.join("hurl/admin")).unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        // The same name exists beside the feature too: if the resolution were
        // feature-first this test would pass with the wrong bytes.
        std::fs::write(root.join("suite/payload.json"), b"FEATURE-COPY").unwrap();
        std::fs::write(root.join("hurl/admin/payload.json"), b"FRAGMENT-COPY").unwrap();
        stage_assets(
            &[from_fragment(
                "payload.json",
                "hurl/admin/upload.hurl#upload",
            )],
            roots(&root.join("suite"), Some(root)),
            &root.join("out"),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(root.join("out/payload.json")).unwrap(),
            b"FRAGMENT-COPY"
        );
    }

    /// A corpus outside the project keeps an absolute recorded name, and the
    /// same single join has to handle it — `Path::join` letting the absolute
    /// side win is what makes the branch unnecessary.
    #[test]
    fn a_corpus_outside_the_project_resolves_from_its_absolute_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("elsewhere/payload.json"), b"OUTSIDE").unwrap();
        let absolute = root.join("elsewhere/shared.hurl");
        stage_assets(
            &[from_fragment(
                "payload.json",
                &format!("{}#upload", absolute.display()),
            )],
            roots(&root.join("project"), Some(&root.join("project"))),
            &root.join("out"),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(root.join("out/payload.json")).unwrap(),
            b"OUTSIDE"
        );
    }

    /// A missing asset used to be skipped in silence. The staged root is what
    /// the engine reads during the run, so the skip surfaced later as hurl
    /// blaming the author for a path that was right all along.
    #[test]
    fn a_missing_asset_is_reported_rather_than_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let err = stage_assets(
            &[inline("absent.json")],
            roots(dir.path(), None),
            dir.path(),
        )
        .unwrap_err();
        let AssetCopyError::Unsafe(message) = err else {
            panic!("a missing source is the author's to fix, not an IO fault");
        };
        assert!(
            message.contains("absent.json") && message.contains("feature"),
            "the message must name the file and where it was looked for: {message}"
        );
    }

    /// The staging root is proef-owned: an existing symlink there must be
    /// replaced, never written *through* — `fs::copy` follows links, so the
    /// bytes would land wherever the link points, outside the root built to
    /// contain them.
    #[cfg(unix)]
    #[test]
    fn a_destination_symlink_is_replaced_not_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("suite")).unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("suite/data.json"), b"STAGED-BYTES").unwrap();
        std::fs::write(root.join("victim.json"), b"UNTOUCHED").unwrap();
        std::os::unix::fs::symlink(root.join("victim.json"), root.join("out/data.json")).unwrap();
        stage_assets(
            &[inline("data.json")],
            roots(&root.join("suite"), None),
            &root.join("out"),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(root.join("victim.json")).unwrap(),
            b"UNTOUCHED",
            "the link's target must never receive the copy"
        );
        let staged = root.join("out/data.json");
        assert!(
            !staged.symlink_metadata().unwrap().file_type().is_symlink(),
            "the destination must be a regular file after staging"
        );
        assert_eq!(std::fs::read(&staged).unwrap(), b"STAGED-BYTES");
    }

    /// Two reference spellings the raw-string map keeps apart can be one
    /// file to the filesystem (case-insensitive volumes; NFC/NFD). The
    /// canonical-destination check refuses the overwrite where it actually
    /// happens, so it is exact per platform — on a case-sensitive volume
    /// both files legitimately coexist and nothing fires, which is why this
    /// test only runs where the collision exists.
    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn two_spellings_that_are_one_file_here_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("suite")).unwrap();
        std::fs::create_dir_all(root.join("hurl")).unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("suite/Data.json"), b"FEATURE").unwrap();
        std::fs::write(root.join("hurl/data.json"), b"FRAGMENT").unwrap();
        let err = stage_assets(
            &[
                inline("Data.json"),
                from_fragment("data.json", "hurl/up.hurl#up"),
            ],
            roots(&root.join("suite"), Some(root)),
            &root.join("out"),
        )
        .unwrap_err();
        let AssetCopyError::Unsafe(message) = err else {
            panic!("a filesystem-level collision is the author's to resolve");
        };
        assert!(
            message.contains("one file"),
            "the message must say the two spellings converge: {message}"
        );
    }

    /// Two sources, one staged name: whichever landed last would win, which
    /// is the silent-wrong-bytes failure this module exists to end.
    #[test]
    fn two_sources_claiming_one_name_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("suite")).unwrap();
        std::fs::create_dir_all(root.join("hurl")).unwrap();
        std::fs::write(root.join("suite/data.json"), b"FEATURE").unwrap();
        std::fs::write(root.join("hurl/data.json"), b"FRAGMENT").unwrap();
        let err = stage_assets(
            &[
                inline("data.json"),
                from_fragment("data.json", "hurl/up.hurl#up"),
            ],
            roots(&root.join("suite"), Some(root)),
            root,
        )
        .unwrap_err();
        assert!(matches!(err, AssetCopyError::Unsafe(_)), "{err}");
    }
}
