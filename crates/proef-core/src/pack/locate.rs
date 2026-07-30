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
}
