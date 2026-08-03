//! The in-memory document overlay and the overlay-then-disk source provider.
//!
//! This is the LSP's IO edge: open buffers live here, disk is the fallback.
//! `proef-core` never touches either — it sees only the `SourceProvider` trait.
//!
//! `lsp-types` 0.97 dropped the `url`-crate-backed `Url` alias in favor of its
//! own RFC-3986 `Uri` (a `fluent_uri::Uri<String>` newtype, spec-correct about
//! case and percent-encoding where `url::Url` normalized too eagerly). There is
//! no `Uri::from_file_path`/`to_file_path` helper, so [`url_to_name`] and
//! [`name_to_url`] hand-roll that bridge for the plain absolute paths a
//! filesystem-backed workspace produces.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Component, Path};
use std::sync::Arc;

use lsp_types::Uri;

/// A source name is a file's path rendered as a string — the same identity
/// `Diag.source_name` and `PackSource.name` already use across the pipeline.
pub fn url_to_name(url: &Uri) -> String {
    if !url.scheme().is_some_and(|s| s.eq_lowercase("file")) {
        return url.as_str().to_owned();
    }
    let mut name = String::new();
    for segment in url.path().segments() {
        name.push('/');
        name.push_str(&segment.decode().into_string_lossy());
    }
    if name.is_empty() {
        name.push('/');
    }
    name
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
            Component::Prefix(prefix) => {
                raw.push('/');
                percent_encode_segment(&prefix.as_os_str().to_string_lossy(), &mut raw);
            }
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

/// Open-buffer text keyed by document URI. Absent key ⇒ read from disk.
#[derive(Debug, Default)]
pub struct Documents {
    open: HashMap<Uri, Arc<str>>,
}

impl Documents {
    /// Records (or replaces) the client-owned text for `url` on `textDocument/didOpen`.
    pub fn open(&mut self, url: Uri, text: String) {
        self.open.insert(url, Arc::from(text));
    }

    /// Replaces the client-owned text for `url` on `textDocument/didChange`.
    pub fn change(&mut self, url: Uri, text: String) {
        self.open.insert(url, Arc::from(text));
    }

    /// Forgets the client-owned text for `url` on `textDocument/didClose`; the
    /// overlay falls back to disk for that URI afterward.
    pub fn close(&mut self, url: &Uri) {
        self.open.remove(url);
    }

    /// The open-buffer text for `url`, if the client has it open.
    pub fn get(&self, url: &Uri) -> Option<&str> {
        self.open.get(url).map(|t| &**t)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn overlay_prefers_open_text_and_forgets_on_close() {
        let mut docs = Documents::default();
        let u = name_to_url("/suite/a.feature").unwrap();
        docs.open(u.clone(), "first".to_owned());
        assert_eq!(docs.get(&u), Some("first"));
        docs.change(u.clone(), "second".to_owned());
        assert_eq!(docs.get(&u), Some("second"));
        docs.close(&u);
        assert_eq!(docs.get(&u), None);
    }

    #[test]
    fn name_url_round_trips() {
        let u = name_to_url("/suite/packs/api.yaml").unwrap();
        let name = url_to_name(&u);
        assert_eq!(name_to_url(&name), Some(u));
    }
}
