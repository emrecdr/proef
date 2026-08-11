//! Fragment scanning (ADR-0018): read named entries out of a real `.hurl` file.
//!
//! The file stays a working hurl file — the annotation is an ordinary comment,
//! and hurl's grammar already attaches a comment to the entry beneath it
//! (`Request::line_terminators`). So the annotation↔entry binding is exactly as
//! reliable as hurl's parser, and nothing here scans text for structure.
//!
//! This module is the only place a hurl AST is turned into
//! [`ScannedFragment`]; `proef-core` never sees a hurl type.

use hurl_core::ast::visit::{self, Visitor};
use hurl_core::ast::{
    Capture, Entry, ExprKind, OptionKind, Placeholder, Template, TemplateElement,
};
use proef_core::engine::{FragmentScanError, ScannedFragment};

/// The annotation marker. It introduces **a name and nothing else, forever** —
/// keeping it a bare identifier is what stops a comment growing into a second
/// configuration language beside the pack (ADR-0018).
const MARKER: &str = "@proef";

/// Scan one fragment file into its entries, annotated or not.
pub(crate) fn scan(text: &str) -> Result<Vec<ScannedFragment>, FragmentScanError> {
    // Appending to the end cannot move any line, so positions stay valid.
    let mut normalized = text.to_owned();
    if !normalized.ends_with('\n') {
        normalized.push('\n');
    }
    let file =
        hurl_core::parser::parse_hurl_file(&normalized).map_err(|err| FragmentScanError {
            line: err.pos.line,
            column: err.pos.column,
            message: format!("{:?}", err.kind),
        })?;

    // An annotation among the file's *trailing* terminators follows no entry —
    // it names nothing, and silently ignoring it would leave a `ref:` pointing
    // at a fragment the author believes they declared.
    for lt in &file.line_terminators {
        if let Some(comment) = &lt.comment
            && annotation_name(&comment.value).is_some()
        {
            return Err(FragmentScanError {
                line: comment.source_info.start.line,
                column: comment.source_info.start.column,
                message: "`@proef` annotation is followed by no request".to_owned(),
            });
        }
    }

    let lines: Vec<&str> = normalized.lines().collect();
    let starts: Vec<usize> = file.entries.iter().map(start_line).collect();
    let mut out = Vec::with_capacity(file.entries.len());

    for (index, entry) in file.entries.iter().enumerate() {
        let name = annotation(entry)?;
        let start = starts[index];
        // Each entry owns its lines up to where the next one begins, so no byte
        // of the file belongs to two fragments or to none.
        let end = starts
            .get(index + 1)
            .copied()
            .unwrap_or(lines.len() + 1)
            .min(lines.len() + 1);
        let body = lines
            .get(start.saturating_sub(1)..end.saturating_sub(1))
            .unwrap_or_default()
            .join("\n");

        let mut collect = Collect::default();
        visit::walk_entry(&mut collect, entry);

        out.push(ScannedFragment {
            name,
            text: format!("{}\n", body.trim_end()),
            line: start,
            placeholders: collect.placeholders,
            captures: collect.captures,
            declares_retry: entry.request.options().iter().any(|option| {
                matches!(
                    option.kind,
                    OptionKind::Retry(_) | OptionKind::RetryInterval(_)
                )
            }),
        });
    }
    Ok(out)
}

/// The line an entry starts on: its first leading comment when it has one — so
/// the annotation travels with the fragment — otherwise its method line.
fn start_line(entry: &Entry) -> usize {
    entry
        .request
        .line_terminators
        .iter()
        .find_map(|lt| lt.comment.as_ref())
        .map_or_else(
            || entry.request.space0.source_info.start.line,
            |comment| comment.source_info.start.line,
        )
}

/// The `@proef` name an entry declares, if any.
fn annotation(entry: &Entry) -> Result<Option<String>, FragmentScanError> {
    let mut found: Option<String> = None;
    for lt in &entry.request.line_terminators {
        let Some(comment) = &lt.comment else { continue };
        let Some(name) = annotation_name(&comment.value) else {
            continue;
        };
        if found.is_some() {
            return Err(FragmentScanError {
                line: comment.source_info.start.line,
                column: comment.source_info.start.column,
                message: "request carries more than one `@proef` annotation".to_owned(),
            });
        }
        if name.is_empty() {
            return Err(FragmentScanError {
                line: comment.source_info.start.line,
                column: comment.source_info.start.column,
                message: "`@proef` needs a fragment name".to_owned(),
            });
        }
        if name.split_whitespace().count() > 1 {
            return Err(FragmentScanError {
                line: comment.source_info.start.line,
                column: comment.source_info.start.column,
                message: format!(
                    "`@proef` takes a name and nothing else, but found `{name}` — \
                     step settings belong in the pack"
                ),
            });
        }
        found = Some(name);
    }
    Ok(found)
}

/// The text after the marker, when a comment is one. `None` for any other
/// comment — including `@proefX`, where the marker is only a prefix of a longer
/// word and the comment is somebody else's.
fn annotation_name(comment: &str) -> Option<String> {
    let rest = comment.trim_start().strip_prefix(MARKER)?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(rest.trim().to_owned())
}

/// Reads and writes of one entry, gathered from hurl's own AST.
///
/// Templates are *leaves* to the visitor — `visit_template`, `visit_url` and
/// `visit_filename` all default to doing nothing, and none of them forwards to
/// another — so each is overridden here. Missing one would silently under-report
/// an entry's inputs, and a missing input reads as "no binding needed".
#[derive(Default)]
struct Collect {
    placeholders: Vec<String>,
    captures: Vec<String>,
}

impl Collect {
    fn scan_template(&mut self, template: &Template) {
        for element in &template.elements {
            if let TemplateElement::Placeholder(placeholder) = element {
                self.record(placeholder);
            }
        }
    }

    fn record(&mut self, placeholder: &Placeholder) {
        if let ExprKind::Variable(variable) = &placeholder.expr.kind
            && !self.placeholders.contains(&variable.name)
        {
            self.placeholders.push(variable.name.clone());
        }
    }
}

impl Visitor for Collect {
    fn visit_template(&mut self, template: &Template) {
        self.scan_template(template);
    }

    fn visit_url(&mut self, url: &Template) {
        self.scan_template(url);
    }

    fn visit_filename(&mut self, filename: &Template) {
        self.scan_template(filename);
    }

    /// Placeholders standing alone in a typed position (`retry: {{n}}`) never
    /// reach `visit_template` — hurl hands those here instead.
    fn visit_placeholder(&mut self, placeholder: &Placeholder) {
        self.record(placeholder);
    }

    fn visit_capture(&mut self, capture: &Capture) {
        if let Some(name) = literal(&capture.name)
            && !self.captures.contains(&name)
        {
            self.captures.push(name);
        }
        visit::walk_capture(self, capture);
    }
}

/// A template's text when it is entirely literal; `None` when it interpolates
/// (a computed capture name is not statically knowable, so it is not claimed).
fn literal(template: &Template) -> Option<String> {
    let mut out = String::new();
    for element in &template.elements {
        match element {
            TemplateElement::String { value, .. } => out.push_str(value),
            TemplateElement::Placeholder(_) => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const FILE: &str = concat!(
        "# a corpus file proef did not write\n",
        "# @proef admin.search\n",
        "GET {{base}}/api/v1/admin/search/{{index}}\n",
        "Authorization: Bearer {{apiToken}}\n",
        "[Query]\n",
        "q: {{q}}\n",
        "HTTP 200\n",
        "[Captures]\n",
        "recordId: jsonpath \"$[0].id\"\n",
        "\n",
        "DELETE {{base}}/api/v1/admin/records/{{recordId}}\n",
        "HTTP 204\n",
    );

    #[test]
    fn an_annotated_entry_reports_its_name_reads_and_writes() {
        let found = scan(FILE).unwrap();
        assert_eq!(found.len(), 2, "both entries are reported");

        let search = &found[0];
        assert_eq!(search.name.as_deref(), Some("admin.search"));
        // The URL is reached through `visit_url`, the header and query through
        // `visit_template` — a scanner overriding only one would miss the rest.
        assert_eq!(search.placeholders, ["base", "index", "apiToken", "q"]);
        assert_eq!(search.captures, ["recordId"]);
        assert!(!search.declares_retry);
    }

    /// An unannotated entry is reported, not dropped: a corpus proef does not
    /// own is mostly these, and the caller decides what to do about them.
    #[test]
    fn an_unannotated_entry_is_inert_but_present() {
        let found = scan(FILE).unwrap();
        assert_eq!(found[1].name, None);
        assert_eq!(found[1].placeholders, ["base", "recordId"]);
        assert!(found[1].captures.is_empty());
    }

    /// Fragment text is the entry's own lines, annotation included, and the
    /// entries partition the file — no line is claimed twice or lost.
    #[test]
    fn fragment_text_carries_the_entry_and_its_annotation() {
        let found = scan(FILE).unwrap();
        assert!(
            found[0]
                .text
                .starts_with("# a corpus file proef did not write\n")
        );
        assert!(found[0].text.contains("# @proef admin.search\n"));
        assert!(
            found[0]
                .text
                .trim_end()
                .ends_with("recordId: jsonpath \"$[0].id\"")
        );
        assert!(
            !found[0].text.contains("DELETE"),
            "an entry must not swallow the next one"
        );
        assert_eq!(found[0].line, 1);
        assert_eq!(found[1].line, 11);
        // Each fragment parses on its own — that is what makes it referenceable.
        assert!(hurl_core::parser::parse_hurl_file(&found[1].text).is_ok());
    }

    #[test]
    fn a_retry_option_is_reported_for_the_double_declaration_check() {
        let found = scan("# @proef poll\nGET http://x\n[Options]\nretry: 3\nHTTP 200\n").unwrap();
        assert!(found[0].declares_retry);
    }

    #[test]
    fn the_marker_must_be_its_own_word() {
        // `@proefX` is somebody else's comment, not a malformed annotation.
        let found = scan("# @proefX note\nGET http://x\n").unwrap();
        assert_eq!(found[0].name, None);
    }

    #[test]
    fn an_annotation_carrying_more_than_a_name_is_refused() {
        let err = scan("# @proef poll retry=3\nGET http://x\n").unwrap_err();
        assert!(
            err.message.contains("a name and nothing else"),
            "{}",
            err.message
        );
        assert_eq!(err.line, 1);
        assert!(
            scan("# @proef\nGET http://x\n").is_err(),
            "a bare marker names nothing"
        );
    }

    #[test]
    fn two_annotations_on_one_request_are_refused() {
        let err = scan("# @proef one\n# @proef two\nGET http://x\n").unwrap_err();
        assert!(err.message.contains("more than one"), "{}", err.message);
    }

    /// A trailing annotation follows no request. Ignoring it would leave a
    /// `ref:` pointing at a fragment its author believes they declared.
    #[test]
    fn an_annotation_after_the_last_request_is_refused() {
        let err = scan("GET http://x\nHTTP 200\n\n# @proef orphan\n").unwrap_err();
        assert!(err.message.contains("no request"), "{}", err.message);
        assert_eq!(err.line, 4);
    }

    #[test]
    fn an_unparseable_file_reports_its_position() {
        let err = scan("GET http://x\nHTTP notastatus\n").unwrap_err();
        assert_eq!(err.line, 2);
    }
}
