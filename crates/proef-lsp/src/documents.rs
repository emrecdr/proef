//! The in-memory document overlay and the overlay-then-disk source provider.
//!
//! This is the LSP's IO edge: open buffers live here, disk is the fallback.
//! `proef-core` never touches either — it sees only the `SourceProvider` trait.
//!
//! `gen-lsp-types` with its `url` feature aliases `Uri` to `url::Url`, whose
//! `from_file_path`/`to_file_path` are the native-path bridge — drive letters,
//! percent-decoding and all. [`url_to_name`] and [`name_to_url`] wrap them
//! only to keep the pipeline's identity rule in one place: a source name is
//! the file's native path as a string, and a non-`file:` URI falls back to its
//! own text so it stays stable and comparable.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use lsp_types::Uri;
use proef_core::provider::{ProviderError, SourceProvider};

/// A source name is a file's path rendered as a string — the same identity
/// `Diag.source_name` and `PackSource.name` already use across the pipeline.
pub fn url_to_name(url: &Uri) -> String {
    if url.scheme() != "file" {
        return url.as_str().to_owned();
    }
    url.to_file_path().map_or_else(
        |()| url.as_str().to_owned(),
        |path| path.to_string_lossy().into_owned(),
    )
}

/// Inverse of [`url_to_name`] for a filesystem path; `None` if it is not an
/// absolute path we can form a `file://` URI from.
pub fn name_to_url(name: &str) -> Option<Uri> {
    Uri::from_file_path(Path::new(name)).ok()
}

/// Open-buffer text keyed by *source name* (`url_to_name` of the document URI) —
/// the same identity the rest of the pipeline uses. Keying by the decoded name
/// (not the raw `Uri`) makes the client's percent-encoding choice irrelevant, so
/// an open buffer is found regardless of how sub-delims were encoded. Absent key
/// ⇒ read from disk.
#[derive(Debug, Default)]
pub struct Documents {
    open: HashMap<String, Arc<str>>,
}

impl Documents {
    /// Records (or replaces) the client-owned text for `url` on `textDocument/didOpen`.
    // `url` is only borrowed for `url_to_name`, but callers already own a fresh
    // `Uri` deserialized from the notification and have no further use for it —
    // taking it by value keeps the call site a plain move, not a needless borrow.
    #[allow(clippy::needless_pass_by_value)]
    pub fn open(&mut self, url: Uri, text: String) {
        self.open.insert(url_to_name(&url), Arc::from(text));
    }

    /// Replaces the client-owned text for `url` on `textDocument/didChange`.
    #[allow(clippy::needless_pass_by_value)]
    pub fn change(&mut self, url: Uri, text: String) {
        self.open.insert(url_to_name(&url), Arc::from(text));
    }

    /// Forgets the client-owned text for `url` on `textDocument/didClose`; the
    /// overlay falls back to disk for that source afterward.
    pub fn close(&mut self, url: &Uri) {
        self.open.remove(&url_to_name(url));
    }

    /// The open-buffer text for source `name`, if the client has it open.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.open.get(name).map(|t| &**t)
    }
}

/// A [`SourceProvider`] that reads open buffers from the overlay and falls back
/// to the injected disk provider. Discovery is disk's job (a suite is what is
/// on disk under the root); an unsaved open buffer only overrides the *bytes* of
/// a source disk already knows about. This is the LSP's whole source seam.
pub struct OverlaySourceProvider<'a> {
    overlay: &'a Documents,
    disk: &'a dyn SourceProvider,
}

impl<'a> OverlaySourceProvider<'a> {
    /// Wraps the open-buffer overlay over a disk fallback for one recompute.
    pub fn new(overlay: &'a Documents, disk: &'a dyn SourceProvider) -> Self {
        Self { overlay, disk }
    }
}

impl SourceProvider for OverlaySourceProvider<'_> {
    fn discover_features(&self) -> Result<Vec<String>, ProviderError> {
        self.disk.discover_features()
    }
    fn discover_packs(&self) -> Result<Vec<String>, ProviderError> {
        self.disk.discover_packs()
    }
    fn discover_fragments(&self) -> Result<Vec<String>, ProviderError> {
        self.disk.discover_fragments()
    }
    fn read(&self, name: &str) -> Result<Arc<str>, ProviderError> {
        if let Some(text) = self.overlay.get(name) {
            return Ok(Arc::from(text));
        }
        self.disk.read(name)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// An absolute path valid on the current OS, built from a `/`-relative tail.
    fn native_abs(rel: &str) -> String {
        #[cfg(windows)]
        {
            format!("C:\\{}", rel.replace('/', "\\"))
        }
        #[cfg(not(windows))]
        {
            format!("/{rel}")
        }
    }

    #[test]
    fn overlay_prefers_open_text_and_forgets_on_close() {
        let mut docs = Documents::default();
        let name = native_abs("suite/a.feature");
        let u = name_to_url(&name).unwrap();
        docs.open(u.clone(), "first".to_owned());
        assert_eq!(docs.get(&name), Some("first"));
        docs.change(u.clone(), "second".to_owned());
        assert_eq!(docs.get(&name), Some("second"));
        docs.close(&u);
        assert_eq!(docs.get(&name), None);
    }

    #[test]
    fn name_url_round_trips() {
        let u = name_to_url(&native_abs("suite/packs/api.yaml")).unwrap();
        let name = url_to_name(&u);
        assert_eq!(name_to_url(&name), Some(u));
    }

    /// Windows-only: the bridge accepts a drive-absolute native path, renders it
    /// back in native form, and round-trips through the `file:///C:/…` URI. This
    /// pins the cross-platform fix; it exercises only on Windows CI.
    #[cfg(windows)]
    #[test]
    fn windows_drive_path_round_trips() {
        let name = "C:\\suite\\a.feature";
        let u = name_to_url(name).expect("a drive-absolute path forms a file URI");
        assert_eq!(url_to_name(&u), name);
        assert_eq!(name_to_url(&url_to_name(&u)), Some(u));
    }

    #[test]
    fn overlay_provider_prefers_open_buffer_over_disk() {
        use proef_core::provider::{ProviderError, SourceProvider};
        use std::sync::Arc;

        struct DiskStub;
        impl SourceProvider for DiskStub {
            fn discover_features(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec![native_abs("s/a.feature")])
            }
            fn discover_packs(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec![])
            }
            fn discover_fragments(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec![])
            }
            fn read(&self, name: &str) -> Result<Arc<str>, ProviderError> {
                if name == native_abs("s/a.feature") {
                    Ok(Arc::from("on disk"))
                } else {
                    Err(ProviderError("missing".to_owned()))
                }
            }
        }

        let mut docs = Documents::default();
        let u = name_to_url(&native_abs("s/a.feature")).unwrap();
        docs.open(u, "in editor".to_owned());
        let disk = DiskStub;
        let overlay = OverlaySourceProvider::new(&docs, &disk);

        // discovery is disk's job
        assert_eq!(
            overlay.discover_features().unwrap(),
            vec![native_abs("s/a.feature")]
        );
        // reading prefers the open buffer
        assert_eq!(
            &*overlay.read(&native_abs("s/a.feature")).unwrap(),
            "in editor"
        );
    }

    // A path segment may legally contain sub-delims like `(`, bare or
    // percent-encoded — the client's choice. Uri equality compares serialized
    // text, so a raw-Uri-keyed overlay missed whenever the client's encoding
    // differed from name_to_url's. Keying by source name (which is decoded)
    // makes the encoding irrelevant — the open buffer is always found.
    #[test]
    fn overlay_finds_open_buffer_for_paths_with_sub_delims() {
        struct DiskStub;
        impl SourceProvider for DiskStub {
            fn discover_features(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec![native_abs("s/a(b.feature")])
            }
            fn discover_packs(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec![])
            }
            fn discover_fragments(&self) -> Result<Vec<String>, ProviderError> {
                Ok(vec![])
            }
            fn read(&self, _name: &str) -> Result<Arc<str>, ProviderError> {
                Ok(Arc::from("on disk"))
            }
        }

        let mut docs = Documents::default();
        // The client opens the doc with the sub-delim percent-ENCODED — legal,
        // and the opposite of `name_to_url`'s spelling (url::Url leaves `(`
        // bare), so a raw-Uri-keyed overlay would miss read()'s re-derived form.
        let u = name_to_url(&native_abs("s/a(b.feature"))
            .unwrap()
            .as_str()
            .replace('(', "%28")
            .parse::<Uri>()
            .unwrap();
        docs.open(u, "in editor".to_owned());
        let disk = DiskStub;
        let overlay = OverlaySourceProvider::new(&docs, &disk);

        // The unsaved buffer must win over disk despite the `(` in the path.
        assert_eq!(
            &*overlay.read(&native_abs("s/a(b.feature")).unwrap(),
            "in editor"
        );
    }
}
