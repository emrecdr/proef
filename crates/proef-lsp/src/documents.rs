//! The in-memory document overlay and the overlay-then-disk source provider.
//!
//! This is the LSP's IO edge: open buffers live here, disk is the fallback.
//! `proef-core` never touches either — it sees only the `SourceProvider` trait.
//!
//! `lsp-types` 0.97 dropped the `url`-crate-backed `Url` alias in favor of its
//! own RFC-3986 `Uri` (a `fluent_uri::Uri<String>` newtype, spec-correct about
//! case and percent-encoding where `url::Url` normalized too eagerly). There is
//! no `Uri::from_file_path`/`to_file_path` helper, so [`url_to_name`] and
//! [`name_to_url`] hand-roll that bridge for the native filesystem paths a
//! workspace produces — plain `/`-rooted paths on Unix, drive-prefixed
//! `C:\…` paths on Windows — so the names round-trip and match what the disk
//! provider renders from a real `PathBuf`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Component, Path, Prefix};
use std::sync::Arc;

use lsp_types::Uri;
use proef_core::provider::{ProviderError, SourceProvider};

/// A source name is a file's path rendered as a string — the same identity
/// `Diag.source_name` and `PackSource.name` already use across the pipeline.
pub fn url_to_name(url: &Uri) -> String {
    if !url.scheme().is_some_and(|s| s.eq_lowercase("file")) {
        return url.as_str().to_owned();
    }
    let segments: Vec<String> = url
        .path()
        .segments()
        .map(|segment| segment.decode().into_string_lossy().into_owned())
        .collect();
    join_native(&segments)
}

/// Joins decoded `file://` path segments into a Unix-native source name: each
/// segment prefixed with `/`, an empty path collapsing to the root `/`. This is
/// the byte-for-byte behavior the bridge has always had on Unix.
#[cfg(not(windows))]
fn join_native(segments: &[String]) -> String {
    let mut name = String::new();
    for segment in segments {
        name.push('/');
        name.push_str(segment);
    }
    if name.is_empty() {
        name.push('/');
    }
    name
}

/// Joins decoded `file://` path segments into a Windows-native source name. A
/// leading drive segment (`C:`) rebuilds the native `C:\seg\seg` form with no
/// separator before the drive; anything else (UNC, drive-less) falls back to a
/// best-effort `\`-joined path so the string is at least stable and comparable.
#[cfg(windows)]
fn join_native(segments: &[String]) -> String {
    match segments.split_first() {
        Some((drive, rest)) if is_drive(drive) => {
            let mut name = drive.clone();
            for segment in rest {
                name.push('\\');
                name.push_str(segment);
            }
            name
        }
        _ => {
            let mut name = String::new();
            for segment in segments {
                name.push('\\');
                name.push_str(segment);
            }
            name
        }
    }
}

/// True for a bare drive segment like `C:` — an ascii letter followed by a colon.
#[cfg(windows)]
fn is_drive(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Inverse of [`url_to_name`] for a filesystem path; `None` if it is not an
/// absolute path we can form a `file://` URI from.
pub fn name_to_url(name: &str) -> Option<Uri> {
    let path = Path::new(name);
    if !path.is_absolute() {
        return None;
    }
    let mut raw = String::from("file://");
    for component in path.components() {
        match component {
            Component::Normal(seg) => {
                raw.push('/');
                percent_encode_segment(&seg.to_string_lossy(), &mut raw);
            }
            Component::Prefix(prefix) => match prefix.kind() {
                // A real drive renders with a bare colon (`file:///C:/…`) — the
                // form LSP clients emit and the only one that round-trips back
                // through `Path::is_absolute` on Windows. Other prefix kinds
                // (UNC, device namespaces) have no such convention, so keep the
                // safe percent-encoded form.
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                    raw.push('/');
                    raw.push(letter as char);
                    raw.push(':');
                }
                _ => {
                    raw.push('/');
                    percent_encode_segment(&prefix.as_os_str().to_string_lossy(), &mut raw);
                }
            },
            Component::RootDir | Component::CurDir | Component::ParentDir => {}
        }
    }
    raw.parse().ok()
}

/// Percent-encodes everything but the RFC 3986 "unreserved" characters.
///
/// Over-encoding (e.g. escaping bytes a path segment could legally carry
/// bare) is always safe here: [`url_to_name`] decodes with `EStr::decode`,
/// which is the exact inverse regardless of how conservatively we encoded.
fn percent_encode_segment(segment: &str, out: &mut String) {
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            // `write!` to a `String` is infallible; nothing to propagate.
            _ => drop(write!(out, "%{byte:02X}")),
        }
    }
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

    // A path segment may legally contain sub-delims like `(`; lsp_types::Uri Eq
    // compares raw strings, so a raw-Uri-keyed overlay missed when the client left
    // `(` bare but name_to_url percent-encoded it. Keying by source name (which is
    // decoded) makes the encoding irrelevant — the open buffer is always found.
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
        // A client (e.g. Neovim) opens the doc with the sub-delim left BARE — legal
        // and common. name_to_url percent-encodes it, so pre-fix the raw-Uri-keyed
        // overlay (keyed by the client's bare form) missed read()'s re-derived
        // encoded form.
        let u = name_to_url(&native_abs("s/a(b.feature"))
            .unwrap()
            .as_str()
            .replace("%28", "(")
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
