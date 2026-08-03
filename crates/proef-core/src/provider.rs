//! The injected source-access seam. `proef-core` never reads a file; it asks a
//! `SourceProvider` for the units under a suite and for their bytes. The CLI
//! provides a disk-backed impl; the LSP provides an overlay-then-disk impl.
//! This is the ADR-0012 pattern — IO at the edge, injected into the sans-IO core.

use std::sync::Arc;

/// A source-access failure (missing file, unreadable path, walk error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError(pub String);

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProviderError {}

/// Discovers and reads the feature files and macro packs of one suite.
///
/// Source *names* are filesystem paths rendered as strings — the identity used
/// by `Diag.source_name` and `PackSource.name` throughout the pipeline. `read`
/// returns **raw** bytes; normalization (BOM strip, trailing newline) is the
/// parser's job, so spans stay consistent with the CLI.
pub trait SourceProvider {
    /// Every feature source name under the suite, in the order the provider finds them.
    fn discover_features(&self) -> Result<Vec<String>, ProviderError>;

    /// Every macro pack source name under the suite, in the order the provider finds them.
    fn discover_packs(&self) -> Result<Vec<String>, ProviderError>;

    /// The raw bytes of one source, keyed by a name returned from discovery.
    fn read(&self, name: &str) -> Result<Arc<str>, ProviderError>;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    struct Fake;
    impl SourceProvider for Fake {
        fn discover_features(&self) -> Result<Vec<String>, ProviderError> {
            Ok(vec!["a.feature".to_owned()])
        }
        fn discover_packs(&self) -> Result<Vec<String>, ProviderError> {
            Ok(vec!["packs/p.yaml".to_owned()])
        }
        fn read(&self, name: &str) -> Result<Arc<str>, ProviderError> {
            match name {
                "a.feature" => Ok(Arc::from("Feature: X\n")),
                _ => Err(ProviderError(format!("no such source: {name}"))),
            }
        }
    }

    #[test]
    fn trait_is_object_safe_and_usable_as_dyn() {
        let p: &dyn SourceProvider = &Fake;
        assert_eq!(p.discover_features().unwrap(), vec!["a.feature".to_owned()]);
        assert_eq!(&*p.read("a.feature").unwrap(), "Feature: X\n");
        assert!(p.read("missing").is_err());
    }
}
