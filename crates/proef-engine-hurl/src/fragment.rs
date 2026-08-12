//! Fragment scanning (ADR-0018): read named entries out of a real `.hurl` file.
//!
//! The file stays a working hurl file — the annotation is an ordinary comment,
//! and hurl's grammar already attaches a comment to the entry beneath it
//! (`Request::line_terminators`). So the annotation↔entry binding is exactly as
//! reliable as hurl's parser, and nothing here scans text for structure.
//!
//! This module is the only place a hurl AST is turned into
//! [`ScannedFragment`]; `proef-core` never sees a hurl type.

use std::borrow::Cow;

use hurl_core::ast::visit::{self, Visitor};
use hurl_core::ast::{
    Comment, Entry, ExprKind, OptionKind, Placeholder, Template, TemplateElement,
};
use proef_core::engine::{FragmentScanError, ScannedFragment};

/// The annotation marker. It introduces **a name and nothing else, forever** —
/// keeping it a bare identifier is what stops a comment growing into a second
/// configuration language beside the pack (ADR-0018).
const MARKER: &str = "@proef";

/// Scan one fragment file into the entries it *annotates*.
pub(crate) fn scan(text: &str) -> Result<Vec<ScannedFragment>, FragmentScanError> {
    // Same normalization rule as every other text entry point (`feature::parse`,
    // `validate_payload`): drop a leading BOM, guarantee a trailing newline.
    // A BOM would otherwise reach hurl's parser as content and fail the whole
    // file at line 1 column 1, blaming the request rather than the encoding.
    // Neither edit moves a line, so reported positions stay valid.
    //
    // Unlike those two this keeps a `Cow`: it runs over an entire corpus, and
    // the LSP re-scans that corpus on every recompute, so not copying the
    // already-well-formed majority is worth the branch.
    let stripped = text.strip_prefix('\u{feff}').unwrap_or(text);
    let normalized = if stripped.ends_with('\n') {
        Cow::Borrowed(stripped)
    } else {
        Cow::Owned(format!("{stripped}\n"))
    };
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
            return Err(at_comment(
                comment,
                "`@proef` annotation is followed by no request".to_owned(),
            ));
        }
    }

    let lines: Vec<&str> = normalized.lines().collect();
    let starts: Vec<usize> = file.entries.iter().map(start_line).collect();
    let mut out = Vec::with_capacity(file.entries.len());

    for (index, entry) in file.entries.iter().enumerate() {
        // Unannotated entries are not fragments and nothing downstream can use
        // one, so they are dropped here rather than built and discarded. The
        // `starts` table above still covers them, which is what keeps an
        // annotated entry's text ending where the *next* entry begins — named
        // or not. A corpus proef did not write is mostly unannotated, and the
        // LSP re-scans it on every recompute, so this is the majority of the
        // work in the common case.
        let Some(name) = annotation(entry)? else {
            continue;
        };
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
            declared_options: declared_options(entry),
            supplied_variables: supplied_variables(entry),
        });
    }
    Ok(out)
}

/// The option families an entry sets for itself, named as the pack spells them
/// so the core can match them against a step's own keys without knowing hurl.
/// `retry-interval` folds into `retry`: they are one policy, and a step's
/// `retry:` sets both.
fn declared_options(entry: &Entry) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for option in entry.request.options() {
        let family = match option.kind {
            OptionKind::Retry(_) | OptionKind::RetryInterval(_) => "retry",
            OptionKind::Delay(_) => "delay",
            _ => continue,
        };
        if !out.iter().any(|seen| seen == family) {
            out.push(family.to_owned());
        }
    }
    out
}

/// The variables an entry supplies to itself via `[Options] variable:`.
///
/// This is how a fragment stays runnable under stock `hurl` with no
/// `--variables-file`, so it must count as an answer to its own
/// `{{placeholder}}` — and it must clash with a `bind:` of the same name, since
/// hurl resolves that pair last-wins rather than refusing it.
fn supplied_variables(entry: &Entry) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for option in entry.request.options() {
        if let OptionKind::Variable(definition) = &option.kind
            && !out.contains(&definition.name)
        {
            out.push(definition.name.clone());
        }
    }
    out
}

/// The line a fragment starts on: its `@proef` annotation when it has one, so
/// the name travels with the text; otherwise the entry's method line.
///
/// Deliberately *not* the first leading comment. hurl attaches a file's whole
/// header block to its first entry, so that rule pulled unrelated prose into
/// every artifact referencing it — the annotation is where the fragment begins.
fn start_line(entry: &Entry) -> usize {
    entry
        .request
        .line_terminators
        .iter()
        .filter_map(|lt| lt.comment.as_ref())
        .find(|comment| annotation_name(&comment.value).is_some())
        .map_or_else(
            || entry.request.space0.source_info.start.line,
            |comment| comment.source_info.start.line,
        )
}

/// A scan error positioned at `comment` — the four sites below differ only in
/// what they say, so the position is written once.
fn at_comment(comment: &Comment, message: impl Into<String>) -> FragmentScanError {
    FragmentScanError {
        line: comment.source_info.start.line,
        column: comment.source_info.start.column,
        message: message.into(),
    }
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
            return Err(at_comment(
                comment,
                "request carries more than one `@proef` annotation".to_owned(),
            ));
        }
        if name.is_empty() {
            return Err(at_comment(
                comment,
                "`@proef` needs a fragment name".to_owned(),
            ));
        }
        if name.split_whitespace().count() > 1 {
            return Err(at_comment(
                comment,
                format!(
                    "`@proef` takes a name and nothing else, but found `{name}` — \
                     step settings belong in the pack"
                ),
            ));
        }
        // `#` is the file qualifier in `ref: file.hurl#name`, so a name
        // containing one can be declared but never referenced: the lookup
        // splits on it and searches for a fragment that does not exist. The
        // failure it produced was a dead end — "names no loaded fragment — did
        // you mean `a#b`?", suggesting the exact spelling that just failed.
        if name.contains('#') {
            return Err(at_comment(
                comment,
                format!(
                    "`@proef` name `{name}` contains `#`, which separates a file from a \
                     fragment in `ref: file.hurl#name` — such a name could never be \
                     referenced"
                ),
            ));
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
        assert_eq!(found.len(), 1, "only the annotated entry is reported");

        let search = &found[0];
        assert_eq!(search.name, "admin.search");
        // The URL is reached through `visit_url`, the header and query through
        // `visit_template` — a scanner overriding only one would miss the rest.
        assert_eq!(search.placeholders, ["base", "index", "apiToken", "q"]);
        assert!(search.declared_options.is_empty());
    }

    /// An unannotated entry is not a fragment, so it is not reported — but it
    /// must still *bound* the one before it. Dropping it from the output while
    /// keeping it in the entry table is what stops `admin.search` swallowing
    /// the `DELETE` that follows it.
    #[test]
    fn an_unannotated_entry_is_dropped_but_still_ends_the_one_before_it() {
        let found = scan(FILE).unwrap();
        assert_eq!(found.len(), 1);
        assert!(
            !found[0].text.contains("DELETE"),
            "the dropped entry still marks where the annotated one stops: {}",
            found[0].text
        );
        assert!(
            !found[0].placeholders.contains(&"recordId".to_owned()),
            "nor may its reads be attributed to the fragment: {:?}",
            found[0].placeholders
        );
    }

    /// Fragment text runs from the annotation to just before the next entry:
    /// the name travels with the request, the file's own header does not, and
    /// no line is claimed twice or lost.
    #[test]
    fn fragment_text_starts_at_the_annotation_not_the_file_header() {
        let found = scan(FILE).unwrap();
        assert!(found[0].text.starts_with("# @proef admin.search\n"));
        assert!(
            !found[0].text.contains("a corpus file proef did not write"),
            "hurl attaches a file's header to its first entry; a fragment is not the header"
        );
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
        assert_eq!(
            found[0].line, 2,
            "the annotation line, not the header above it"
        );
        // Each fragment parses on its own — that is what makes it referenceable.
        assert!(hurl_core::parser::parse_hurl_file(&found[0].text).is_ok());
    }

    #[test]
    fn a_retry_option_is_reported_for_the_double_declaration_check() {
        let found = scan("# @proef poll\nGET http://x\n[Options]\nretry: 3\nHTTP 200\n").unwrap();
        assert_eq!(found[0].declared_options, ["retry"]);
    }

    /// A variable an entry sets for itself is read back by name, and is *not*
    /// mistaken for one the entry reads. Both halves matter: the name answers
    /// that entry's own `{{token}}` (so the file still runs under stock `hurl`
    /// with no variables file) and collides with a `bind:` of the same name
    /// (so the pair is refused rather than resolved last-wins).
    #[test]
    fn a_supplied_variable_is_reported_by_name() {
        let found = scan(concat!(
            "# @proef auth\n",
            "GET http://x\n",
            "[Options]\n",
            "variable: token=local-default\n",
            "variable: token=again\n",
            "[Query]\n",
            "t: {{token}}\n",
            "u: {{other}}\n",
            "HTTP 200\n",
        ))
        .unwrap();
        assert_eq!(found[0].supplied_variables, ["token"], "deduped by name");
        assert_eq!(
            found[0].placeholders,
            ["token", "other"],
            "supplying a variable does not stop the entry from reading it"
        );
        assert!(
            found[0].declared_options.is_empty(),
            "`variable:` is not an option *family* — it clashes name-to-name"
        );
    }

    /// The seam's contract, checked across the crate boundary rather than
    /// assumed on each side of it.
    ///
    /// `declared_options` is compared by **string equality** against the pack's
    /// own option keys, so a spelling only this engine knows would match
    /// nothing and silently disable `option_declared_twice` — reinstating
    /// hurl's last-wins, which is the bug that rule exists to refuse. Both of
    /// hurl's retry spellings must therefore arrive folded into one family.
    #[test]
    fn every_option_family_the_scanner_emits_is_one_the_pack_knows() {
        let found = scan(concat!(
            "# @proef every\n",
            "GET http://x\n",
            "[Options]\n",
            "retry: 3\n",
            "retry-interval: 500\n",
            "delay: 100\n",
            "HTTP 200\n",
        ))
        .unwrap();
        assert_eq!(
            found[0].declared_options,
            ["retry", "delay"],
            "`retry-interval` folds into `retry`; both families are reported once"
        );
        for family in &found[0].declared_options {
            assert!(
                proef_core::engine::OPTION_FAMILIES.contains(&family.as_str()),
                "`{family}` is not a family the pack can declare, so the clash check cannot see it"
            );
        }
    }

    #[test]
    fn the_marker_must_be_its_own_word() {
        // `@proefX` is somebody else's comment, not a malformed annotation — so
        // the entry is unannotated and reported by nobody.
        let found = scan("# @proefX note\nGET http://x\n").unwrap();
        assert!(found.is_empty());
    }

    /// A BOM is an encoding artifact, not content. Left in place it reaches
    /// hurl's parser as the first character of the first request and fails the
    /// whole file at 1:1 — blaming the request for the editor that saved it.
    #[test]
    fn a_leading_byte_order_mark_is_stripped_before_parsing() {
        // Unwraps deliberately: an `Err` here *is* the regression — the scan
        // failing on the encoding rather than reading the request.
        let found = scan("\u{feff}# @proef ping\nGET http://x\nHTTP 200\n").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "ping");
        assert!(
            !found[0].text.starts_with('\u{feff}'),
            "and it must not travel into the artifact, which has to be valid hurl"
        );
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
