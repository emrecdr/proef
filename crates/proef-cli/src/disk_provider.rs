//! The CLI's disk-backed `SourceProvider`. It adds no discovery logic of its
//! own — it delegates to the existing `front` walkers, so there is exactly one
//! discovery implementation in the workspace.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use proef_core::provider::{ProviderError, SourceProvider};

use crate::front;

/// The disk-backed `SourceProvider` the `proef lsp` handler constructs over the
/// workspace root; the overlay-then-disk analysis reads through it.
pub struct DiskSourceProvider {
    root: PathBuf,
    /// The `[run] fragments` root, when the project configures one (ADR-0018).
    fragments: Option<PathBuf>,
}

impl DiskSourceProvider {
    /// Builds a provider rooted at `root`, which must be an **absolute** path —
    /// the LSP passes `current_dir()`, which is already absolute. Kept
    /// uncanonicalized here: canonicalizing would resolve symlinks and desync
    /// source names from the identity the LSP client already knows via document
    /// URIs.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            fragments: None,
        }
    }

    /// Point the provider at a fragment root (`[run] fragments`), so the LSP
    /// resolves `ref:` the same way a run does. Without it every `ref:` reads
    /// as unknown in the editor while the suite runs fine — the drift that
    /// makes editor diagnostics untrustworthy.
    #[must_use]
    pub fn with_fragments(mut self, fragments: Option<PathBuf>) -> Self {
        self.fragments = fragments;
        self
    }
}

/// Source names, spelled the way [`proef_lsp::documents::url_to_name`] spells a
/// document URI — separators normalized to the platform's own.
///
/// A source name is an **identity**, compared as a string: the LSP looks a
/// document up by the name it derives from the client's URI, and any other
/// spelling of the same file is a different key. `Path::join` appends without
/// rewriting what is already there, so a `proef.toml` saying
/// `suite = "tests/features"` — the portable spelling, and the one the docs
/// use — yields `C:\proj\tests/features\packs\api.yaml` on Windows while the URI
/// side yields `C:\proj\tests\features\packs\api.yaml`. The two never match, and
/// every URI-keyed request (go-to-definition, references, completion) answers
/// `null` while the suite itself runs green.
///
/// Re-collecting the components rebuilds the path in native form, which is a
/// no-op on Unix (one separator exists) and the whole fix on Windows.
fn names(paths: Vec<PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .map(|p| {
            p.components()
                .collect::<PathBuf>()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

impl SourceProvider for DiskSourceProvider {
    fn discover_features(&self) -> Result<Vec<String>, ProviderError> {
        front::discover_features(&self.root)
            .map(names)
            .map_err(|e| ProviderError(e.to_string()))
    }

    fn discover_packs(&self) -> Result<Vec<String>, ProviderError> {
        front::pack_files(&self.root)
            .map(names)
            .map_err(|e| ProviderError(e.to_string()))
    }

    fn discover_fragments(&self) -> Result<Vec<String>, ProviderError> {
        let Some(root) = &self.fragments else {
            return Ok(Vec::new());
        };
        let kinds = crate::registry::step_kinds();
        front::fragment_files(root, &kinds)
            .map(names)
            .map_err(|e| ProviderError(e.to_string()))
    }

    fn read(&self, name: &str) -> Result<Arc<str>, ProviderError> {
        std::fs::read_to_string(Path::new(name))
            .map(|s| Arc::from(s.as_str()))
            .map_err(|e| ProviderError(format!("cannot read {name}: {e}")))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use proef_core::provider::SourceProvider;
    use std::path::PathBuf;

    fn workspace_root() -> PathBuf {
        // crates/proef-cli/src -> repo root
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
    }

    /// A discovered name must be spelled the way the LSP spells a document URI,
    /// or the two are different keys for one file and every URI-keyed request
    /// answers `null` while the suite runs green.
    ///
    /// Windows-only because it is the only platform with two separators: a
    /// `proef.toml` saying `suite = "tests/features"` reaches `join` as-is, and
    /// the result must still come back native. This is asserted rather than
    /// assumed because the local gate cannot run it — the bug it pins shipped
    /// green on macOS and Linux.
    #[cfg(windows)]
    #[test]
    fn a_discovered_name_is_spelled_natively_whatever_the_config_said() {
        let mixed = PathBuf::from(r"C:\proj").join("tests/features/packs/api.yaml");
        assert_eq!(
            names(vec![mixed]),
            [r"C:\proj\tests\features\packs\api.yaml"],
        );
    }

    #[test]
    fn discovers_and_reads_the_reference_suite() {
        let root = workspace_root().join("tests/features");
        let provider = DiskSourceProvider::new(root.clone());

        let features = provider.discover_features().unwrap();
        assert!(
            features
                .iter()
                .any(|f| f.ends_with("501_api_event.feature")),
            "expected the reference feature to be discovered, got {features:?}"
        );

        let packs = provider.discover_packs().unwrap();
        assert!(packs.iter().any(|p| p.ends_with("api.yaml")));

        // Reading a discovered feature yields its raw bytes.
        let name = features
            .iter()
            .find(|f| f.ends_with("501_api_event.feature"))
            .unwrap();
        let text = provider.read(name).unwrap();
        assert!(text.contains("Feature:"));

        assert!(provider.read("/nope/missing.feature").is_err());

        // ABSOLUTE-PATH INVARIANT: the LSP keys everything on absolute paths so
        // source names round-trip to client URIs. A provider constructed with an
        // absolute root must yield absolute source names.
        for f in &features {
            assert!(
                std::path::Path::new(f).is_absolute(),
                "feature name not absolute: {f}"
            );
        }
        for p in &packs {
            assert!(
                std::path::Path::new(p).is_absolute(),
                "pack name not absolute: {p}"
            );
        }
    }
}
