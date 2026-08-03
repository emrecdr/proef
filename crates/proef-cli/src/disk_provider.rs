//! The CLI's disk-backed `SourceProvider`. It adds no discovery logic of its
//! own — it delegates to the existing `front` walkers, so there is exactly one
//! discovery implementation in the workspace.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use proef_core::provider::{ProviderError, SourceProvider};

use crate::front;

/// Not yet wired into a command; the LSP handler will construct one over the
/// workspace root once the collect-all analysis lands.
#[allow(dead_code)]
pub struct DiskSourceProvider {
    root: PathBuf,
}

impl DiskSourceProvider {
    /// Builds a provider rooted at `root`, which must be an **absolute** path —
    /// the CLI passes the suite directory as given on the command line, and the
    /// LSP passes a canonicalized `current_dir()`. Kept uncanonicalized here:
    /// canonicalizing would resolve symlinks and desync source names from the
    /// identity the LSP client already knows via document URIs.
    #[allow(dead_code)]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

// Dead in a non-test build until something constructs `DiskSourceProvider`;
// the trait impl below stays live because it satisfies a public trait, but
// this helper it calls does not.
#[allow(dead_code)]
fn names(paths: Vec<PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
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
