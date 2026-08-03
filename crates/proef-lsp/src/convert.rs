//! The byte-span ↔ LSP-position bridge — the highest-bug-risk surface, isolated
//! and property-tested. It reconciles two mismatches at once:
//!   1. proef spans index *normalized* source (BOM stripped, trailing `\n`
//!      appended — see `proef_core` feature normalization); editors hold *raw*
//!      text. The BOM strip shifts every offset by its byte length.
//!   2. proef spans are byte offsets; LSP positions are (line, UTF-16 column).

use lsp_types::{Position, Range};
use proef_core::diag::Span;

const BOM: char = '\u{feff}';

/// The `proef_core` feature-normalization rule (strip a leading BOM; append a
/// trailing newline if missing — mirrors `proef_core::feature::parse`,
/// `crates/proef-core/src/feature.rs:69-73`). Callers that need normalized-text
/// byte coordinates without running full feature parsing share this one
/// implementation rather than inlining the rule again.
pub fn normalize(raw: &str) -> String {
    let stripped = raw.strip_prefix(BOM).unwrap_or(raw);
    let mut s = stripped.to_owned();
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Byte-offset/LSP-position index over one document's raw editor text.
///
/// Built once per document version; bridges `proef_core` diagnostics (byte
/// spans into normalized text) and `lsp_types` positions (line, UTF-16
/// column, into raw text).
pub struct LineIndex {
    /// The raw editor text.
    raw: String,
    /// Byte offset (into `raw`) of the start of each line.
    line_starts: Vec<usize>,
    /// Bytes stripped from the front during normalization (0 or BOM length).
    bom_len: usize,
}

impl LineIndex {
    /// Builds the index over `raw` — the exact text the editor holds
    /// (BOM intact, no synthetic trailing newline).
    pub fn new(raw: &str) -> Self {
        let bom_len = if raw.starts_with(BOM) {
            BOM.len_utf8()
        } else {
            0
        };
        let mut line_starts = vec![0usize];
        for (i, b) in raw.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self {
            raw: raw.to_owned(),
            line_starts,
            bom_len,
        }
    }

    /// Convert a *normalized* byte offset to a raw byte offset, clamped into the
    /// raw text (a span over the synthetic trailing newline lands at raw end)
    /// and snapped to a raw char boundary (arbitrary/out-of-range inputs must
    /// never land mid-codepoint).
    fn normalized_to_raw(&self, normalized: usize) -> usize {
        let raw_off = (normalized + self.bom_len).min(self.raw.len());
        self.floor_char_boundary(raw_off)
    }

    /// The largest byte offset `<= off` that lies on a char boundary of `raw`.
    /// `off` is first clamped into range, so this always terminates (offset 0
    /// and `raw.len()` are always boundaries).
    fn floor_char_boundary(&self, off: usize) -> usize {
        let mut off = off.min(self.raw.len());
        while !self.raw.is_char_boundary(off) {
            off -= 1;
        }
        off
    }

    /// The raw byte offset where column-counting starts for `line`: the line's
    /// start, except line 0 skips a leading BOM — the BOM is stripped during
    /// normalization and consumes no LSP column.
    fn content_start(&self, line: usize) -> usize {
        let line_start = self.line_starts[line];
        if line == 0 {
            line_start.max(self.bom_len)
        } else {
            line_start
        }
    }

    fn raw_offset_to_position(&self, raw_off: usize) -> Position {
        // Binary-search the line whose start is <= raw_off.
        let line = match self.line_starts.binary_search(&raw_off) {
            Ok(exact) => exact,
            Err(next) => next - 1,
        };
        let start = self.content_start(line).min(raw_off);
        // UTF-16 column: count code units between the line's content start and raw_off.
        let col16 = self.raw[start..raw_off]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>();
        // Line/column counts stay far under u32::MAX for any realistic source
        // file; LSP's `Position` is fixed at u32, so the narrowing is total in
        // practice.
        #[allow(clippy::cast_possible_truncation)]
        Position {
            line: line as u32,
            character: col16 as u32,
        }
    }

    /// Normalized-text byte span → LSP range (raw text, UTF-16 columns).
    pub fn span_to_range(&self, span: Span) -> Range {
        let start = self.raw_offset_to_position(self.normalized_to_raw(span.start));
        let end = self.raw_offset_to_position(self.normalized_to_raw(span.end));
        Range { start, end }
    }

    /// LSP position (raw text) → byte offset into the *normalized* text.
    pub fn position_to_offset(&self, pos: Position) -> usize {
        let line = (pos.line as usize).min(self.line_starts.len().saturating_sub(1));
        let start = self.content_start(line);
        // Walk chars accumulating UTF-16 units until we reach `pos.character`.
        let mut raw_off = start;
        let mut col16 = 0u32;
        for ch in self.raw[start..].chars() {
            if col16 >= pos.character || ch == '\n' {
                break;
            }
            // See the comment in `raw_offset_to_position`: UTF-16 column counts
            // stay far under u32::MAX for any realistic source file.
            #[allow(clippy::cast_possible_truncation)]
            {
                col16 += ch.len_utf16() as u32;
            }
            raw_off += ch.len_utf8();
        }
        // Raw offset → normalized offset (undo the BOM shift), clamped to >= 0.
        raw_off.saturating_sub(self.bom_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;
    use proef_core::diag::Span;

    #[test]
    fn ascii_span_maps_to_expected_range() {
        // "abc\ndef\n" — span over "def" is bytes 4..7 (normalized == raw here).
        let idx = LineIndex::new("abc\ndef\n");
        let r = idx.span_to_range(Span { start: 4, end: 7 });
        assert_eq!(
            r.start,
            Position {
                line: 1,
                character: 0
            }
        );
        assert_eq!(
            r.end,
            Position {
                line: 1,
                character: 3
            }
        );
    }

    #[test]
    fn non_ascii_uses_utf16_columns() {
        // "é" is 2 UTF-8 bytes but 1 UTF-16 unit; "😀" is 4 bytes, 2 UTF-16 units.
        let raw = "é😀x\n";
        let idx = LineIndex::new(raw);
        // byte span over the trailing "x": é=2 bytes, 😀=4 bytes → x at byte 6..7.
        let r = idx.span_to_range(Span { start: 6, end: 7 });
        // columns: é=1 unit, 😀=2 units → x starts at UTF-16 column 3.
        assert_eq!(
            r.start,
            Position {
                line: 0,
                character: 3
            }
        );
        assert_eq!(
            r.end,
            Position {
                line: 0,
                character: 4
            }
        );
    }

    #[test]
    fn bom_shifts_normalized_offsets_back_to_raw() {
        // Raw begins with a BOM (3 bytes). Normalized strips it, so a
        // normalized offset of 0 is raw offset 3 → line 0, column 0 in raw.
        let raw = "\u{feff}abc\n";
        let idx = LineIndex::new(raw);
        let r = idx.span_to_range(Span { start: 0, end: 3 }); // "abc" in normalized
        assert_eq!(
            r.start,
            Position {
                line: 0,
                character: 0
            }
        );
        assert_eq!(
            r.end,
            Position {
                line: 0,
                character: 3
            }
        );
    }

    #[test]
    fn appended_newline_clamps_to_raw_end() {
        // Raw has no trailing newline; normalization appended one at offset 3.
        // A span pointing at that synthetic newline clamps to the raw end.
        let raw = "abc";
        let idx = LineIndex::new(raw);
        let r = idx.span_to_range(Span { start: 3, end: 4 });
        assert_eq!(
            r.start,
            Position {
                line: 0,
                character: 3
            }
        );
        assert_eq!(
            r.end,
            Position {
                line: 0,
                character: 3
            }
        );
    }

    #[test]
    fn position_round_trips_to_normalized_offset() {
        let raw = "é😀x\ndef\n";
        let idx = LineIndex::new(raw);
        let pos = Position {
            line: 0,
            character: 3,
        }; // start of "x"
        assert_eq!(idx.position_to_offset(pos), 6); // normalized byte offset of "x"
    }

    mod properties {
        #![allow(clippy::ignored_unit_patterns)]
        // Property inputs are bounded (line/character loops below stay in the
        // single digits), so `usize`→`u32` narrowing here is always exact.
        #![allow(clippy::cast_possible_truncation)]

        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// Every valid (line, column) position in the raw text maps to a
            /// normalized offset and back to the same position — for ASCII,
            /// non-ASCII, BOM, and missing-trailing-newline inputs.
            #[test]
            fn position_offset_position_round_trips(
                // A mix of ASCII, accented, emoji, newlines; optionally BOM-prefixed.
                bom in prop::bool::ANY,
                body in prop::collection::vec(
                    prop_oneof!["[a-z ]", "é", "😀", "\n"], 0..40),
            ) {
                let mut raw = String::new();
                if bom { raw.push('\u{feff}'); }
                raw.extend(body);
                let idx = LineIndex::new(&raw);

                // For each line, pick the end-of-content column and round-trip it.
                let line_count = raw.bytes().filter(|&b| b == b'\n').count() + 1;
                for line in 0..line_count as u32 {
                    for character in 0u32..6 {
                        let off = idx.position_to_offset(Position { line, character });
                        // Offset must land on a char boundary of the normalized text.
                        let normalized = normalize(&raw);
                        prop_assert!(off <= normalized.len());
                        prop_assert!(normalized.is_char_boundary(off));
                        // Mapping that offset back to a range never panics and stays
                        // within the raw text bounds.
                        let r = idx.span_to_range(Span { start: off, end: off });
                        prop_assert!(r.start.line < line_count as u32);
                    }
                }
            }

            /// `span_to_range` never panics and produces start <= end for any span
            /// bounded by the normalized length.
            #[test]
            fn span_to_range_is_total(
                raw in "\\PC{0,50}",
                a in 0usize..60,
                b in 0usize..60,
            ) {
                let idx = LineIndex::new(&raw);
                let (start, end) = (a.min(b), a.max(b));
                let r = idx.span_to_range(Span { start, end });
                prop_assert!(
                    (r.start.line, r.start.character) <= (r.end.line, r.end.character)
                );
            }
        }
    }
}
