//! Push-model diagnostics: every recompute republishes each file's diagnostics
//! and explicitly clears files that are now clean.

use std::collections::HashSet;
use std::sync::Arc;

use lsp_server::{Connection, Message, Notification};
use lsp_types::{Diagnostic, DiagnosticSeverity, PublishDiagnosticsParams, Uri};
use proef_core::diag::{Diag, Severity};

use crate::analysis::Analysis;
use crate::convert::LineIndex;
use crate::documents::name_to_url;

fn to_lsp(diag: &Diag, index: &LineIndex) -> Diagnostic {
    let range = diag
        .span
        .map(|s| index.span_to_range(s))
        .unwrap_or_default();
    Diagnostic {
        range,
        severity: Some(match diag.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
        }),
        code: Some(lsp_types::NumberOrString::String(diag.code.to_owned())),
        // The published catalogue: editors render this as a clickable link
        // on the code — the one in-band route from an error to its docs.
        code_description: "https://emrecdr.github.io/proef/DIAGNOSTICS.html"
            .parse()
            .ok()
            .map(|href| lsp_types::CodeDescription { href }),
        source: Some("proef".to_owned()),
        message: diag.help.as_ref().map_or_else(
            || diag.message.clone(),
            |h| format!("{}\n\n{h}", diag.message),
        ),
        ..Default::default()
    }
}

/// Publish diagnostics for every analyzed file, clearing files that became
/// clean since the previous publish. `published` tracks the last-published set
/// of source *names* — the same identity `analyze_suite` keys diagnostics by; the
/// `Uri` a name maps to is derived only when a notification is sent.
pub(crate) fn publish(
    connection: &Connection,
    analysis: &Analysis,
    published: &mut HashSet<String>,
) {
    let mut now: HashSet<String> = HashSet::new();

    for (name, diags) in &analysis.suite.diagnostics {
        let Some(url) = name_to_url(name) else {
            continue;
        };
        // An analyzed source with no captured raw text (e.g. a built-in pack)
        // still indexes cleanly against the empty string; its spans clamp to 0.
        let empty: Arc<str> = Arc::from("");
        let raw = analysis.raw.get(name).unwrap_or(&empty);
        let index = LineIndex::new(raw);
        let lsp_diags: Vec<Diagnostic> = diags.iter().map(|d| to_lsp(d, &index)).collect();
        now.insert(name.clone());
        send(connection, url, lsp_diags);
    }

    // Clear files that had diagnostics last time but are clean now.
    for stale in published.difference(&now) {
        if let Some(url) = name_to_url(stale) {
            send(connection, url, Vec::new());
        }
    }
    *published = now;
}

fn send(connection: &Connection, uri: Uri, diagnostics: Vec<Diagnostic>) {
    let params = PublishDiagnosticsParams {
        uri,
        diagnostics,
        version: None,
    };
    // `PublishDiagnosticsParams` is plain data, so serialization cannot fail in
    // practice; if it ever did, dropping this note is safer than panicking a
    // long-lived server — the next recompute republishes anyway.
    let Ok(value) = serde_json::to_value(params) else {
        return;
    };
    let note = Notification {
        method: "textDocument/publishDiagnostics".to_owned(),
        params: value,
    };
    let _ = connection.sender.send(Message::Notification(note));
}
