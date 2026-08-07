//! Best-effort locators mapping semantic pack findings onto pack-file spans.
//!
//! serde gives no spans for *valid* YAML, so these scan the raw text for the
//! well-formed layout (`macros:` at the root, macro names at indent 2).
//! Every locator degrades to `None` — diagnostics then render without a span
//! but still name the macro and step.

use crate::diag::Span;

/// Byte span of a macro's name key (`  <name>:`), when locatable.
pub(crate) fn macro_span(text: &str, name: &str) -> Option<Span> {
    for (offset, line) in lines_with_offsets(text) {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if indent == 2
            && (trimmed.starts_with(&format!("{name}:"))
                || trimmed.starts_with(&format!("\"{name}\":")))
        {
            let start = offset + indent;
            return Some(Span::clamped(start, start + name.len(), text.len()));
        }
    }
    None
}

/// Byte range of a macro's block: from its header line to the next
/// macro header (indent-2 key) or end of file.
fn macro_region(text: &str, name: &str) -> Option<(usize, usize)> {
    let mut start = None;
    for (offset, line) in lines_with_offsets(text) {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        let is_header = indent == 2 && trimmed.ends_with(':') && !trimmed.starts_with('#');
        match start {
            None => {
                if is_header && trimmed.starts_with(&format!("{name}:")) {
                    start = Some(offset);
                }
            }
            Some(begin) => {
                if is_header {
                    return Some((begin, offset));
                }
            }
        }
    }
    start.map(|begin| (begin, text.len()))
}

/// Span of line `rel_line` (1-based) inside the `ordinal`-th (0-based)
/// `<payload_key>:` block of `macro_name`, for mapping engine probe errors
/// (block-relative positions) onto the pack file.
pub(crate) fn payload_line_span(
    text: &str,
    macro_name: &str,
    payload_key: &str,
    ordinal: usize,
    rel_line: usize,
) -> Option<Span> {
    if rel_line == 0 {
        return None; // 1-based by contract — degrade, never underflow
    }
    let (begin, end) = macro_region(text, macro_name)?;
    let region = &text[begin..end];
    let mut seen = 0usize;
    let mut lines = lines_with_offsets(region);
    while let Some((offset, line)) = lines.next() {
        let trimmed = line.trim_start();
        // The key may share the line with the sequence dash (`- hurl: |`).
        let trimmed = trimmed.strip_prefix("- ").unwrap_or(trimmed);
        if trimmed.starts_with(&format!("{payload_key}:")) {
            if seen == ordinal {
                // Content starts on the next line; step rel_line - 1 further.
                let mut remaining = rel_line;
                for (content_offset, content_line) in lines.by_ref() {
                    remaining -= 1;
                    if remaining == 0 {
                        let lead = content_line.len() - content_line.trim_start().len();
                        let start = begin + content_offset + lead;
                        let stop = begin + content_offset + content_line.trim_end().len();
                        return Some(Span::clamped(start, stop.max(start), text.len()));
                    }
                }
                // Block shorter than the reported line — point at the key.
                let start = begin + offset;
                return Some(Span::clamped(start, start + payload_key.len(), text.len()));
            }
            seen += 1;
        }
    }
    None
}

/// Content spans of every `hurl:` line in `macro_name`'s `expect:` items, in
/// textual order — pairs positionally with items whose `hurl` field is
/// `Some(..)` (an assert-only macro has no `steps:`, so every `hurl:` line in
/// its block belongs to an `expect:` item).
pub(crate) fn expect_hurl_line_spans(text: &str, macro_name: &str) -> Vec<Span> {
    key_line_spans(text, macro_name, "hurl")
}

/// Every content span, in textual order, of the lines in `macro_name`'s block
/// whose content (after stripping a leading `- ` sequence dash) begins `<key>:` —
/// each the line's trimmed content, so a cursor anywhere on it resolves. One scan
/// of the block; empty when the macro or key isn't found. The shared primitive
/// behind [`match_span`] and [`use_line_spans`]; best-effort, never panics.
fn key_line_spans(text: &str, macro_name: &str, key: &str) -> Vec<Span> {
    let Some((begin, end)) = macro_region(text, macro_name) else {
        return Vec::new();
    };
    let region = &text[begin..end];
    let prefix = format!("{key}:");
    let mut spans = Vec::new();
    for (offset, line) in lines_with_offsets(region) {
        let trimmed = line.trim_start();
        let lead = line.len() - trimmed.len();
        let after_dash = trimmed.strip_prefix("- ").unwrap_or(trimmed);
        let dash = trimmed.len() - after_dash.len();
        if after_dash.starts_with(&prefix) {
            let start = begin + offset + lead + dash;
            let stop = begin + offset + line.trim_end().len();
            spans.push(Span::clamped(start, stop.max(start), text.len()));
        }
    }
    spans
}

/// Content span of a macro's `match:` line (there is at most one), when
/// locatable — the go-to-definition landing anchor. `None` otherwise.
pub(crate) fn match_span(text: &str, macro_name: &str) -> Option<Span> {
    key_line_spans(text, macro_name, "match").into_iter().next()
}

/// Content spans of every `use:` line in `macro_name`'s block, in textual order
/// (indent and any `- ` sequence dash stripped, so a cursor anywhere on a
/// reference resolves). One scan; empty when the macro has no `use:` steps.
///
/// `analyze::index_use_refs` pairs these positionally with the macro's parsed
/// `MacroStepKind::Use` steps. Because this is the same scan that yields every
/// locatable `use:` span, comparing its length with the parsed step count is a
/// self-consistent guard: a flow-style `- {use: base}` step parses to a `Use` but
/// its line does not begin `use:` after the dash strip, so the counts diverge and
/// the caller skips that macro rather than risk a wrong pairing.
pub(crate) fn use_line_spans(text: &str, macro_name: &str) -> Vec<Span> {
    key_line_spans(text, macro_name, "use")
}

/// `(byte_offset, line_without_newline)` for every line.
fn lines_with_offsets(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    text.split_inclusive('\n').map(move |raw| {
        let start = offset;
        offset += raw.len();
        (start, raw.trim_end_matches(['\n', '\r']))
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    const PACK: &str = "macros:\n  first:\n    match: do it\n    steps:\n      - name: a\n        hurl: |\n          GET http://x/one\n          HTTP 200\n  second:\n    steps:\n      - hurl: |\n          GET http://x/two\n";

    #[test]
    fn macro_names_are_located() {
        let span = macro_span(PACK, "second").expect("span");
        assert_eq!(&PACK[span.start..span.end], "second");
        assert!(macro_span(PACK, "absent").is_none());
    }

    #[test]
    fn payload_lines_map_back_to_the_file() {
        let span = payload_line_span(PACK, "first", "hurl", 0, 2).expect("span");
        assert_eq!(&PACK[span.start..span.end], "HTTP 200");
        // The second template's block is independent.
        let span = payload_line_span(PACK, "second", "hurl", 0, 1).expect("span");
        assert_eq!(&PACK[span.start..span.end], "GET http://x/two");
    }

    const USE_PACK: &str = "macros:\n  base:\n    match: the base\n    steps:\n      - hurl: |\n          GET http://x\n  wrapper:\n    steps:\n      - use: base\n      - use: base#other\n";

    #[test]
    fn match_lines_are_located() {
        let span = match_span(USE_PACK, "base").expect("span");
        assert_eq!(&USE_PACK[span.start..span.end], "match: the base");
        // A macro with no `match:` (use-only) yields None.
        assert!(match_span(USE_PACK, "wrapper").is_none());
        assert!(match_span(USE_PACK, "absent").is_none());
    }

    #[test]
    fn use_lines_are_collected_in_order() {
        let spans = use_line_spans(USE_PACK, "wrapper");
        assert_eq!(spans.len(), 2);
        assert_eq!(&USE_PACK[spans[0].start..spans[0].end], "use: base");
        assert_eq!(&USE_PACK[spans[1].start..spans[1].end], "use: base#other");
        // A macro with no `use:` → empty (never panics).
        assert!(use_line_spans(USE_PACK, "base").is_empty());
        assert!(use_line_spans(USE_PACK, "absent").is_empty());
    }

    const EXPECT_PACK: &str = "macros:\n  checkThing:\n    expect:\n      - status: \"200\"\n      - hurl: |\n          jsonpath \"$.a\" exists\n      - status: \"201\"\n        hurl: |\n          jsonpath \"$.b\" exists\n";

    #[test]
    fn expect_hurl_lines_pair_positionally_with_hurl_bearing_items() {
        // Three items, only the last two carry `hurl:` — the returned spans
        // skip the status-only item rather than leaving a hole.
        let spans = expect_hurl_line_spans(EXPECT_PACK, "checkThing");
        assert_eq!(spans.len(), 2);
        assert_eq!(&EXPECT_PACK[spans[0].start..spans[0].end], "hurl: |");
        assert_eq!(&EXPECT_PACK[spans[1].start..spans[1].end], "hurl: |");
        assert!(expect_hurl_line_spans(EXPECT_PACK, "absent").is_empty());
    }

    const MIXED_USE_PACK: &str = "macros:\n  base:\n    match: the base\n    steps:\n      - hurl: |\n          GET http://x\n  wrapper:\n    steps:\n      - {use: base}\n      - use: base\n";

    #[test]
    fn use_line_spans_see_only_block_style_lines() {
        assert_eq!(use_line_spans(USE_PACK, "wrapper").len(), 2);
        assert!(use_line_spans(USE_PACK, "base").is_empty());
        assert!(use_line_spans(USE_PACK, "absent").is_empty());
        // Flow-style `- {use: base}` is valid YAML and parses to a `Use` step, but
        // its line does not start with `use:` after the dash strip — it is not
        // seen here, so this undercounts relative to the parsed step count. That
        // divergence is exactly what `analyze::index_use_refs` guards on.
        assert_eq!(use_line_spans(MIXED_USE_PACK, "wrapper").len(), 1);
    }
}
