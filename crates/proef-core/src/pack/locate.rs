//! Best-effort locators mapping semantic pack findings onto pack-file spans.
//!
//! serde gives no spans for *valid* YAML, so these scan the raw text for the
//! well-formed layout (`macros:` at the root, macro names at indent 2).
//! Every locator degrades to `None` — diagnostics then render without a span
//! but still name the macro and step.
//!
//! Scanning is done **once per file**, not once per lookup. Each locator used
//! to walk the whole text to find its macro's block, so validating a pack of
//! N macros scanned it N times: measured at 4× the time for each doubling of
//! the macro count (0.03 s at 400 macros, 1.96 s at 3200). [`MacroIndex`]
//! records every macro's name span and block region in one pass, and the
//! locators became lookups into it.

use std::collections::BTreeMap;

use crate::diag::Span;

/// Where one macro sits in its pack file.
struct Anchor {
    /// Byte span of the macro's name key (`  <name>:`).
    name: Span,
    /// Byte range of the macro's block: its header line to the next indent-2
    /// header, or end of file.
    region: (usize, usize),
}

/// Every macro header in one pack file, found in a single pass.
///
/// Borrows the text it indexed: the spans are offsets into *that* string, and
/// letting the two separate would be a way to produce spans into the wrong
/// file.
pub(crate) struct MacroIndex<'a> {
    text: &'a str,
    macros: BTreeMap<&'a str, Anchor>,
}

impl<'a> MacroIndex<'a> {
    /// Index `text` in one pass.
    pub(crate) fn new(text: &'a str) -> Self {
        let mut macros: BTreeMap<&'a str, Anchor> = BTreeMap::new();
        let mut open: Option<(&'a str, usize)> = None;
        for (offset, line) in lines_with_offsets(text) {
            let trimmed = line.trim_start();
            let indent = line.len() - trimmed.len();
            // A *header* ends at its colon. A pack root holds `bind:` at the
            // same indent and its entries are `name: value` lines too, so a
            // bare prefix test anchored macro diagnostics on config lines.
            if indent != 2 || !trimmed.ends_with(':') || trimmed.starts_with('#') {
                continue;
            }
            // Close the previous macro at this header.
            if let Some((name, start)) = open.take() {
                Self::insert(&mut macros, text, name, start, offset);
            }
            let key = &trimmed[..trimmed.len() - 1];
            // A quoted header names the same macro as a bare one. `macro_span`
            // always accepted both; the region scan behind every *other*
            // locator accepted only the bare form, so a quoted macro silently
            // lost its `match:`/`use:`/`ref:`/payload spans. One reader, one
            // answer.
            let name = key
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                .unwrap_or(key);
            open = Some((name, offset));
        }
        if let Some((name, start)) = open {
            Self::insert(&mut macros, text, name, start, text.len());
        }
        Self { text, macros }
    }

    /// Record one macro, keeping the **first** header of a duplicated name —
    /// the same one the old first-match-wins scan returned. (A duplicate is
    /// already a load error; this only decides where its caret sits.)
    fn insert(
        macros: &mut BTreeMap<&'a str, Anchor>,
        text: &str,
        name: &'a str,
        start: usize,
        end: usize,
    ) {
        let indent = 2;
        let quoted = text[start..].trim_start().starts_with('"');
        let name_start = start + indent + usize::from(quoted);
        macros.entry(name).or_insert(Anchor {
            name: Span::clamped(name_start, name_start + name.len(), text.len()),
            region: (start, end),
        });
    }

    /// Byte span of a macro's name key, when locatable.
    pub(crate) fn macro_span(&self, name: &str) -> Option<Span> {
        self.macros.get(name).map(|anchor| anchor.name)
    }

    /// Content span of a macro's `match:` line (there is at most one), when
    /// locatable — the go-to-definition landing anchor.
    ///
    /// [`KeyLines::sole`] rather than a pairing: a macro has at most one
    /// `match:`, so there is no sequence to line up and nothing to miscount. A
    /// flow-style macro yields no span, which is the right answer — no anchor —
    /// rather than a wrong one.
    pub(crate) fn match_span(&self, name: &str) -> Option<Span> {
        self.key_lines(name, "match").sole()
    }

    /// Content lines of every `hurl:` key in `name`'s `expect:` items, in
    /// textual order, for positional pairing with the items whose `hurl` field
    /// is `Some(..)` (an assert-only macro has no `steps:`, so every `hurl:`
    /// line in its block belongs to an `expect:` item).
    pub(crate) fn expect_hurl_lines(&self, name: &str) -> KeyLines {
        self.key_lines(name, "hurl")
    }

    /// Located `<key>:` lines in `name`'s block, as a value that does not hand
    /// them over until the caller states how many it expects.
    pub(crate) fn key_lines(&self, name: &str, key: &str) -> KeyLines {
        KeyLines(self.key_line_spans(name, key))
    }

    /// Every content span, in textual order, of the lines in `name`'s block
    /// whose content (after stripping a leading `- ` sequence dash) begins
    /// `<key>:` — each the line's trimmed content, so a cursor anywhere on it
    /// resolves. Empty when the macro or key isn't found.
    ///
    /// Private: every caller goes through [`Self::key_lines`], which returns a
    /// [`KeyLines`] rather than a bare `Vec<Span>`. This scan recognises only
    /// block-style `key:` lines, so a flow-style `- {use: base}` step parses to
    /// a `Use` and contributes no line — the list is a *lower bound* on the
    /// parsed items, and handing it out as a plain vector makes an undercount
    /// indistinguishable from a complete answer.
    fn key_line_spans(&self, name: &str, key: &str) -> Vec<Span> {
        let Some((begin, end)) = self.region(name) else {
            return Vec::new();
        };
        let region = &self.text[begin..end];
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
                spans.push(Span::clamped(start, stop.max(start), self.text.len()));
            }
        }
        spans
    }

    /// Span of line `rel_line` (1-based) inside the `ordinal`-th (0-based)
    /// `<payload_key>:` block of `name`, for mapping engine probe errors
    /// (block-relative positions) onto the pack file.
    pub(crate) fn payload_line_span(
        &self,
        name: &str,
        payload_key: &str,
        ordinal: usize,
        rel_line: usize,
    ) -> Option<Span> {
        if rel_line == 0 {
            return None; // 1-based by contract — degrade, never underflow
        }
        let (begin, end) = self.region(name)?;
        let region = &self.text[begin..end];
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
                            return Some(Span::clamped(start, stop.max(start), self.text.len()));
                        }
                    }
                    // Block shorter than the reported line — point at the key.
                    let start = begin + offset;
                    return Some(Span::clamped(
                        start,
                        start + payload_key.len(),
                        self.text.len(),
                    ));
                }
                seen += 1;
            }
        }
        None
    }

    fn region(&self, name: &str) -> Option<(usize, usize)> {
        self.macros.get(name).map(|anchor| anchor.region)
    }
}

/// Byte span of one 1-based line's trimmed content — the anchor for a finding
/// an engine reported by position rather than by offset (a fragment scan, a
/// payload probe). Degrades to `None` past the end, like every locator here.
pub(crate) fn line_span(text: &str, line: usize) -> Option<Span> {
    let (offset, raw) = lines_with_offsets(text).nth(line.checked_sub(1)?)?;
    let lead = raw.len() - raw.trim_start().len();
    Some(Span::clamped(
        offset + lead,
        offset + raw.trim_end().len(),
        text.len(),
    ))
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

/// Located `<key>:` lines in one macro's block, in textual order.
///
/// **Why this is a type and not a `Vec<Span>`.** The scan behind it sees only
/// block-style `key:` lines: a flow-style `- {use: base}` item is valid YAML,
/// parses to a real step, and contributes no line. So the list can be shorter
/// than the parsed items it is meant to describe, and pairing them positionally
/// then attributes every span after the gap to the wrong item — a
/// go-to-definition that lands on the neighbouring line, or a diagnostic
/// pointing at it.
///
/// Both callers already knew this and each wrote its own count comparison, in
/// its own words, from a prose warning. That is a convention, and a convention
/// only holds while everyone who arrives has read it; a third caller would have
/// had to rediscover both the hazard and the remedy. Here the remedy is the
/// only way through: there is no accessor that yields the spans without being
/// told how many the caller expects.
pub(crate) struct KeyLines(Vec<Span>);

impl KeyLines {
    /// The spans, in textual order — **only** when there are exactly `parsed`
    /// of them.
    ///
    /// `None` means the scan and the parser disagree about how many items this
    /// macro has, so no positional pairing between them is trustworthy. The
    /// caller falls back to a coarser anchor (the macro's own span) rather than
    /// risking an ordinal-shifted wrong one.
    pub(crate) fn paired_with(self, parsed: usize) -> Option<Vec<Span>> {
        (self.0.len() == parsed).then_some(self.0)
    }

    /// The first span, for a key that can occur at most once in a block
    /// (`match:`), where there is no sequence to pair against.
    pub(crate) fn sole(self) -> Option<Span> {
        self.0.into_iter().next()
    }

    /// How many lines were located. For tests and diagnostics only — a caller
    /// that pairs must go through [`Self::paired_with`].
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    const PACK: &str = "macros:\n  first:\n    match: do it\n    steps:\n      - name: a\n        hurl: |\n          GET http://x/one\n          HTTP 200\n  second:\n    steps:\n      - hurl: |\n          GET http://x/two\n";

    #[test]
    fn macro_names_are_located() {
        let index = MacroIndex::new(PACK);
        let span = index.macro_span("second").expect("span");
        assert_eq!(&PACK[span.start..span.end], "second");
        assert!(index.macro_span("absent").is_none());
    }

    #[test]
    fn payload_lines_map_back_to_the_file() {
        let index = MacroIndex::new(PACK);
        let span = index
            .payload_line_span("first", "hurl", 0, 2)
            .expect("span");
        assert_eq!(&PACK[span.start..span.end], "HTTP 200");
        // The second template's block is independent.
        let span = index
            .payload_line_span("second", "hurl", 0, 1)
            .expect("span");
        assert_eq!(&PACK[span.start..span.end], "GET http://x/two");
    }

    const USE_PACK: &str = "macros:\n  base:\n    match: the base\n    steps:\n      - hurl: |\n          GET http://x\n  wrapper:\n    steps:\n      - use: base\n      - use: base#other\n";

    #[test]
    fn match_lines_are_located() {
        let index = MacroIndex::new(USE_PACK);
        let span = index.match_span("base").expect("span");
        assert_eq!(&USE_PACK[span.start..span.end], "match: the base");
        // A macro with no `match:` (use-only) yields None.
        assert!(index.match_span("wrapper").is_none());
        assert!(index.match_span("absent").is_none());
    }

    #[test]
    fn use_lines_are_collected_in_order() {
        let index = MacroIndex::new(USE_PACK);
        let spans = index.key_line_spans("wrapper", "use");
        assert_eq!(spans.len(), 2);
        assert_eq!(&USE_PACK[spans[0].start..spans[0].end], "use: base");
        assert_eq!(&USE_PACK[spans[1].start..spans[1].end], "use: base#other");
        // A macro with no `use:` → empty (never panics).
        assert!(index.key_line_spans("base", "use").is_empty());
        assert!(index.key_line_spans("absent", "use").is_empty());
    }

    const EXPECT_PACK: &str = "macros:\n  checkThing:\n    expect:\n      - status: \"200\"\n      - hurl: |\n          jsonpath \"$.a\" exists\n      - status: \"201\"\n        hurl: |\n          jsonpath \"$.b\" exists\n";

    #[test]
    fn expect_hurl_lines_pair_positionally_with_hurl_bearing_items() {
        // Three items, only the last two carry `hurl:` — the returned spans
        // skip the status-only item rather than leaving a hole.
        let index = MacroIndex::new(EXPECT_PACK);
        let spans = index
            .expect_hurl_lines("checkThing")
            .paired_with(2)
            .expect("two hurl-bearing items, two located lines");
        assert_eq!(&EXPECT_PACK[spans[0].start..spans[0].end], "hurl: |");
        assert_eq!(&EXPECT_PACK[spans[1].start..spans[1].end], "hurl: |");
        assert_eq!(index.expect_hurl_lines("absent").len(), 0);
    }

    const MIXED_USE_PACK: &str = "macros:\n  base:\n    match: the base\n    steps:\n      - hurl: |\n          GET http://x\n  wrapper:\n    steps:\n      - {use: base}\n      - use: base\n";

    #[test]
    fn use_line_spans_see_only_block_style_lines() {
        let index = MacroIndex::new(USE_PACK);
        assert_eq!(index.key_line_spans("wrapper", "use").len(), 2);
        assert!(index.key_line_spans("base", "use").is_empty());
        assert!(index.key_line_spans("absent", "use").is_empty());
        // Flow-style `- {use: base}` is valid YAML and parses to a `Use` step, but
        // its line does not start with `use:` after the dash strip — it is not
        // seen here, so this undercounts relative to the parsed step count.
        assert_eq!(
            MacroIndex::new(MIXED_USE_PACK)
                .key_line_spans("wrapper", "use")
                .len(),
            1
        );
    }

    /// **The undercount cannot reach a caller as if it were complete.**
    ///
    /// `MIXED_USE_PACK`'s `wrapper` has two parsed `use:` steps and one located
    /// line, because the flow-style item contributes none. Asking for two gets
    /// `None` — no spans at all — rather than one span that would be paired
    /// with the wrong step. Asking for the count that is actually there still
    /// works, which is what keeps this a guard rather than a blanket refusal.
    ///
    /// Before `KeyLines`, the primitive returned a bare `Vec<Span>` and each of
    /// the two callers wrote its own length comparison from a prose warning.
    /// Both were correct; neither was enforced.
    #[test]
    fn located_lines_refuse_to_pair_with_a_count_they_do_not_match() {
        let index = MacroIndex::new(MIXED_USE_PACK);
        assert!(
            index.key_lines("wrapper", "use").paired_with(2).is_none(),
            "an undercount must not be handed over as a complete pairing"
        );
        assert_eq!(
            index
                .key_lines("wrapper", "use")
                .paired_with(1)
                .expect("one located line pairs with one")
                .len(),
            1
        );
    }

    /// `match:` occurs at most once, so it has no sequence to pair against and
    /// takes `sole()`. A flow-style macro yields no span — the right answer (no
    /// anchor), not a wrong one.
    #[test]
    fn a_sole_key_needs_no_pairing() {
        assert!(
            MacroIndex::new(USE_PACK)
                .key_lines("base", "match")
                .sole()
                .is_some()
        );
        assert!(
            MacroIndex::new("macros:\n  flow: {match: the thing}\n")
                .key_lines("flow", "match")
                .sole()
                .is_none()
        );
    }

    /// A pack root's `bind:` sits at the same indent as a macro name and its
    /// entries look like `name: value` lines. Only a *header* opens a macro.
    #[test]
    fn a_root_bind_table_is_not_a_macro() {
        let pack = "bind:\n  token: abc\nmacros:\n  token:\n    match: use the token\n    steps:\n      - hurl: |\n          GET http://x\n";
        let index = MacroIndex::new(pack);
        let span = index
            .macro_span("token")
            .expect("the macro, not the bind key");
        // The macro `token:` is the indent-2 header under `macros:`; the bind
        // entry `  token: abc` is not a header (it carries a value).
        assert_eq!(&pack[span.start..span.end], "token");
        assert!(span.start > pack.find("macros:").expect("macros"));
        assert_eq!(
            &pack[index.match_span("token").expect("match").start..],
            "match: use the token\n    steps:\n      - hurl: |\n          GET http://x\n"
        );
    }

    /// A quoted macro header names the same macro as a bare one.
    ///
    /// `macro_span` always accepted both spellings while the region scan behind
    /// every other locator accepted only the bare form, so a quoted macro got a
    /// caret on its name and silently nothing for its `match:`, `use:`, `ref:`
    /// or payload lines. One reader now gives one answer.
    #[test]
    fn a_quoted_macro_header_locates_like_a_bare_one() {
        let pack = "macros:\n  \"needs quotes\":\n    match: do the thing\n    steps:\n      - use: other\n";
        let index = MacroIndex::new(pack);
        let span = index.macro_span("needs quotes").expect("the name span");
        assert_eq!(&pack[span.start..span.end], "needs quotes");
        let match_span = index.match_span("needs quotes").expect("its match line");
        assert_eq!(
            &pack[match_span.start..match_span.end],
            "match: do the thing"
        );
        assert_eq!(index.key_line_spans("needs quotes", "use").len(), 1);
    }
}
