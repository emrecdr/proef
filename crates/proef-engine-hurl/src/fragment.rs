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
use proef_core::engine::{FragmentScanError, ScannedFile, ScannedFragment};

/// The annotation marker. It introduces **a name and nothing else, forever** —
/// keeping it a bare identifier is what stops a comment growing into a second
/// configuration language beside the pack (ADR-0018).
const MARKER: &str = "@proef";

/// Scan one fragment file into the entries it annotates, plus the lines of the
/// ones it does not.
pub(crate) fn scan(text: &str) -> Result<ScannedFile, FragmentScanError> {
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
    let mut unannotated = Vec::new();

    for (index, entry) in file.entries.iter().enumerate() {
        // An unannotated entry is not a fragment and nothing downstream can use
        // one, so only its line is kept — never a built-then-discarded
        // `ScannedFragment`. The `starts` table above still covers it, which is
        // what keeps an annotated entry's text ending where the *next* entry
        // begins, named or not. A corpus proef did not write is mostly
        // unannotated and the LSP re-scans it on every recompute, so this stays
        // a push rather than the majority of the work.
        let Some(name) = annotation(entry)? else {
            unannotated.push(starts[index]);
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
    Ok(ScannedFile {
        fragments: out,
        unannotated,
    })
}

/// The option families an entry sets for itself, named as the pack spells them
/// so the core can match them against a step's own keys without knowing hurl.
/// `retry-interval` folds into `retry`: they are one policy, and a step's
/// `retry:` sets both.
fn declared_options(entry: &Entry) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for option in entry.request.options() {
        // Through the same table the inline path uses, keyed by hurl's own
        // `identifier()` — the raw text left of the `:`. Written twice (once
        // over `OptionKind` variants, once over strings) these were one rule at
        // two spellings, and the halves are consulted by *different* checks:
        // the AST table feeds the double-declaration refusal, the string table
        // feeds the ADR-0007 value caps. A family added to one and not the
        // other would be capped but never clash-checked.
        let Some(family) = crate::recognise_option(option.kind.identifier()).and_then(|o| o.family)
        else {
            continue;
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

/// The variable names one template value reads — [`FragmentSupport::template_reads`]
/// for the hurl kind ([`proef_core::engine::FragmentSupport`]).
///
/// The value is parsed exactly as it will be evaluated: it reaches hurl as the
/// right-hand side of an injected `[Options] variable:` line, whose value
/// grammar is a template. Rather than re-implement that grammar, the value is
/// placed in a probe entry's header — the same `Template` production — and the
/// file handed to hurl's parser; the walk is the same [`Collect`] the fragment
/// scanner uses, so "what does this text read" has exactly one answer in the
/// tree, and `ExprKind::Function` (`{{newUuid}}`, `{{newDate}}`) never
/// registers as a variable (R17-2.2 — a text-level scan reported functions as
/// unbound variables, refusing input the engine itself runs). Text the parser
/// refuses reports no reads: the emitted artifact is parse-validated anyway,
/// so a malformed template still fails loudly there, under hurl's own error.
pub(crate) fn template_reads(value: &str) -> Vec<String> {
    let probe = format!("GET http://probe.invalid\nx-proef-probe: {value}\nHTTP 200\n");
    let Ok(file) = hurl_core::parser::parse_hurl_file(&probe) else {
        return Vec::new();
    };
    let mut collect = Collect::default();
    for entry in &file.entries {
        visit::walk_entry(&mut collect, entry);
    }
    collect.placeholders
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

    /// One answer to "what does this read", and it is the parser's: functions
    /// are not variables, filters reduce to their subject, junk reads nothing.
    #[test]
    fn template_reads_reports_variables_and_never_functions() {
        assert_eq!(template_reads("{{q}}"), ["q"]);
        assert_eq!(template_reads("idx-{{base}}-{{q}}"), ["base", "q"]);
        assert_eq!(template_reads("{{newUuid}}"), Vec::<String>::new());
        assert_eq!(template_reads("{{newDate}}-{{q}}"), ["q"]);
        assert_eq!(template_reads("literal only"), Vec::<String>::new());
        assert_eq!(template_reads("broken {{"), Vec::<String>::new());
    }

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
        let scanned = scan(FILE).unwrap();
        assert_eq!(
            scanned.fragments.len(),
            1,
            "only the annotated entry is reported"
        );

        let search = &scanned.fragments[0];
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
        let scanned = scan(FILE).unwrap();
        assert_eq!(scanned.fragments.len(), 1);
        assert!(
            !scanned.fragments[0].text.contains("DELETE"),
            "the dropped entry still marks where the annotated one stops: {}",
            scanned.fragments[0].text
        );
        assert!(
            !scanned.fragments[0]
                .placeholders
                .contains(&"recordId".to_owned()),
            "nor may its reads be attributed to the fragment: {:?}",
            scanned.fragments[0].placeholders
        );
    }

    /// Fragment text runs from the annotation to just before the next entry:
    /// the name travels with the request, the file's own header does not, and
    /// no line is claimed twice or lost.
    #[test]
    fn fragment_text_starts_at_the_annotation_not_the_file_header() {
        let scanned = scan(FILE).unwrap();
        assert!(
            scanned.fragments[0]
                .text
                .starts_with("# @proef admin.search\n")
        );
        assert!(
            !scanned.fragments[0]
                .text
                .contains("a corpus file proef did not write"),
            "hurl attaches a file's header to its first entry; a fragment is not the header"
        );
        assert!(
            scanned.fragments[0]
                .text
                .trim_end()
                .ends_with("recordId: jsonpath \"$[0].id\"")
        );
        assert!(
            !scanned.fragments[0].text.contains("DELETE"),
            "an entry must not swallow the next one"
        );
        assert_eq!(
            scanned.fragments[0].line, 2,
            "the annotation line, not the header above it"
        );
        // Each fragment parses on its own — that is what makes it referenceable.
        assert!(hurl_core::parser::parse_hurl_file(&scanned.fragments[0].text).is_ok());
    }

    #[test]
    fn a_retry_option_is_reported_for_the_double_declaration_check() {
        let scanned = scan("# @proef poll\nGET http://x\n[Options]\nretry: 3\nHTTP 200\n").unwrap();
        assert_eq!(scanned.fragments[0].declared_options, ["retry"]);
    }

    /// A variable an entry sets for itself is read back by name, and is *not*
    /// mistaken for one the entry reads. Both halves matter: the name answers
    /// that entry's own `{{token}}` (so the file still runs under stock `hurl`
    /// with no variables file) and collides with a `bind:` of the same name
    /// (so the pair is refused rather than resolved last-wins).
    #[test]
    fn a_supplied_variable_is_reported_by_name() {
        let scanned = scan(concat!(
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
        assert_eq!(
            scanned.fragments[0].supplied_variables,
            ["token"],
            "deduped by name"
        );
        assert_eq!(
            scanned.fragments[0].placeholders,
            ["token", "other"],
            "supplying a variable does not stop the entry from reading it"
        );
        assert!(
            scanned.fragments[0].declared_options.is_empty(),
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
        let scanned = scan(concat!(
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
            scanned.fragments[0].declared_options,
            ["retry", "delay"],
            "`retry-interval` folds into `retry`; both families are reported once"
        );
        for family in &scanned.fragments[0].declared_options {
            assert!(
                proef_core::engine::OPTION_FAMILIES.contains(&family.as_str()),
                "`{family}` is not a family the pack can declare, so the clash check cannot see it"
            );
        }
    }

    #[test]
    fn the_marker_must_be_its_own_word() {
        // `@proefX` is somebody else's comment, not a malformed annotation — so
        // the entry is unannotated: nothing can `ref:` it, and it is reported
        // only as a line a listing can point at.
        let scanned = scan("# @proefX note\nGET http://x\n").unwrap();
        assert!(scanned.fragments.is_empty());
        // Line 2, the request itself: an annotated entry starts at its
        // annotation, an unannotated one has none to start at, so the method
        // line is both the truth and the thing a listing wants to point at.
        assert_eq!(scanned.unannotated, [2]);
    }

    /// A BOM is an encoding artifact, not content. Left in place it reaches
    /// hurl's parser as the first character of the first request and fails the
    /// whole file at 1:1 — blaming the request for the editor that saved it.
    #[test]
    fn a_leading_byte_order_mark_is_stripped_before_parsing() {
        // Unwraps deliberately: an `Err` here *is* the regression — the scan
        // failing on the encoding rather than reading the request.
        let scanned = scan("\u{feff}# @proef ping\nGET http://x\nHTTP 200\n").unwrap();
        assert_eq!(scanned.fragments.len(), 1);
        assert_eq!(scanned.fragments[0].name, "ping");
        assert!(
            !scanned.fragments[0].text.starts_with('\u{feff}'),
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

/// Properties over **generated hurl files** (R9-2).
///
/// Arbitrary bytes would be the wrong generator here and it is worth saying why:
/// this module's own risk is the entry-boundary arithmetic — `starts[index]` to
/// `starts[index + 1]`, the slice that decides which lines belong to which
/// fragment — and random text dies in hurl's parser before reaching it. The
/// same lesson the fragment fuzz target learned by measurement. So the strategy
/// below *builds* files that parse, and spends its budget on the shapes that
/// move a boundary: entries with and without annotations, blank lines and stray
/// comments between them, and options/captures that lengthen an entry.
#[cfg(test)]
mod properties {
    #![allow(clippy::unwrap_used)]

    use super::scan;
    use proef_core::engine::OPTION_FAMILIES;
    use proptest::prelude::*;

    /// One entry, as source text plus whether it carries an annotation.
    fn entry() -> impl Strategy<Value = (String, bool)> {
        (
            prop::option::of("[a-z][a-z0-9.]{0,8}"),
            any::<bool>(),
            any::<bool>(),
            0u8..4,
        )
            .prop_map(|(name, with_options, with_captures, blanks)| {
                let mut text = String::new();
                for _ in 0..blanks {
                    // Blank and comment-only lines between entries are exactly
                    // what shifts a start line without adding an entry.
                    text.push_str("\n# filler\n");
                }
                let annotated = name.is_some();
                if let Some(name) = &name {
                    use std::fmt::Write as _;
                    let _ = writeln!(text, "# @proef {name}");
                }
                text.push_str("GET http://example.invalid/p\n");
                if with_options {
                    text.push_str("[Options]\nretry: 2\n");
                }
                text.push_str("HTTP 200\n");
                if with_captures {
                    text.push_str("[Captures]\nid: jsonpath \"$.id\"\n");
                }
                (text, annotated)
            })
    }

    proptest! {
        /// Every reported line points into the file, every entry is accounted
        /// for exactly once, and the lines come back in order.
        ///
        /// Ordering is the load-bearing one: a fragment's text runs to where the
        /// *next* entry starts, so a start table that was out of order or that
        /// skipped an unannotated entry would hand one fragment another's body —
        /// silently, and into a durable artifact.
        #[test]
        fn a_scan_partitions_the_file_in_order_and_within_its_bounds(
            entries in prop::collection::vec(entry(), 0..6)
        ) {
            let text: String = entries.iter().map(|(t, _)| t.as_str()).collect();
            let annotated = entries.iter().filter(|(_, a)| *a).count();
            let total = entries.len();
            let line_count = text.lines().count().max(1);

            let scanned = scan(&text).unwrap();

            prop_assert_eq!(scanned.fragments.len(), annotated);
            prop_assert_eq!(scanned.fragments.len() + scanned.unannotated.len(), total);

            let mut lines: Vec<usize> = scanned.fragments.iter().map(|f| f.line).collect();
            lines.extend(scanned.unannotated.iter().copied());
            lines.sort_unstable();
            for line in &lines {
                prop_assert!(*line >= 1 && *line <= line_count, "line {} outside 1..={}", line, line_count);
            }
            let mut deduped = lines.clone();
            deduped.dedup();
            prop_assert_eq!(deduped.len(), lines.len(), "two entries claimed one start line");

            for fragment in &scanned.fragments {
                prop_assert!(fragment.text.ends_with('\n'), "entry text must end with a newline");
                prop_assert!(!fragment.text.is_empty());
                // The partition of *text*, not just of start lines. The line
                // accounting above passes even when a fragment's body runs on
                // into the entry after it — mutation-checked, and it did — so
                // the boundary is pinned by what each fragment ends up holding.
                // The generator emits exactly one request line per entry, so
                // "one request line per fragment" is that partition, stated.
                prop_assert_eq!(
                    fragment.text.matches("GET http://example.invalid").count(),
                    1,
                    "fragment {} swallowed a neighbouring entry: {:?}",
                    fragment.name, fragment.text
                );
                // And it begins where its annotation does, never at the file
                // header or at the filler before it.
                prop_assert!(
                    fragment.text.starts_with(&format!("# @proef {}", fragment.name)),
                    "fragment text must start at its annotation: {:?}", fragment.text
                );
                for family in &fragment.declared_options {
                    prop_assert!(
                        OPTION_FAMILIES.contains(&family.as_str()),
                        "scanner emitted an option family the pack cannot match: {}", family
                    );
                }
            }
        }

        /// Totality over free text: a corpus is foreign by design, so anything
        /// may be in it. Err is a fine answer; a panic is not.
        #[test]
        fn scanning_arbitrary_text_never_panics(text in ".{0,400}") {
            let _ = scan(&text);
        }
    }
}
