//! Semantic tokens: telling proef's two variable tiers apart on screen.
//!
//! A macro pack's `hurl: |` block is the centre of the authoring experience and,
//! to every editor, a plain YAML scalar. Inside it two kinds of placeholder sit
//! side by side and look identical:
//!
//! - `${…}` resolves at **lower time**. proef substitutes it while building the
//!   artifact, so by the time a request is made the text is gone.
//! - `{{…}}` resolves at **run time**. proef passes it through untouched and
//!   hurl resolves it against the variable set.
//!
//! That distinction is ADR-0005's whole model, it is the thing authors most
//! often get wrong, and no generic grammar can see it — a YAML highlighter sees
//! a string and a hurl highlighter never runs, because the block is not a file.
//! proef is the only party that knows, which is what makes this worth serving
//! over the protocol rather than leaving to a static syntax grammar.
//!
//! The mapping is chosen to survive an editor's default theme rather than to be
//! clever: `${…}` is a **macro** (a substitution performed before execution —
//! which is what a macro *is*) and `{{…}}` is a **variable** (resolved during
//! it). Every mainstream theme colours those two differently already, so the
//! feature works without anyone configuring anything.

use lsp_types::{SemanticToken, SemanticTokens};
use proef_core::diag::Span;

use crate::analysis::Analysis;
use crate::convert::LineIndex;
use crate::documents::url_to_name;

/// The legend, in the order the indices below refer to. Announced in
/// `ServerCapabilities`; a client reads it once and then sees only integers, so
/// the order here *is* the wire format — appending is safe, reordering is not.
pub const TOKEN_TYPES: [&str; 2] = ["macro", "variable"];

/// Index into [`TOKEN_TYPES`] for a lower-time `${…}` reference.
const LOWER_TIME: u32 = 0;
/// Index into [`TOKEN_TYPES`] for a run-time `{{…}}` placeholder.
const RUN_TIME: u32 = 1;

/// Tokens for the document at `url`, or `None` when the suite has no source for
/// it (an untracked file, or one the provider could not read).
pub fn tokens(analysis: &Analysis, url: &lsp_types::Uri) -> Option<SemanticTokens> {
    let name = url_to_name(url);
    let raw = analysis.raw.get(&name)?;
    Some(SemanticTokens {
        result_id: None,
        data: encode(raw),
    })
}

/// Every placeholder in `text`, as the protocol's delta-encoded token stream.
fn encode(text: &str) -> Vec<SemanticToken> {
    let mut spans: Vec<(Span, u32)> = proef_core::resolve::reference_spans(text)
        .into_iter()
        .map(|span| (span, LOWER_TIME))
        .collect();
    spans.extend(
        run_time_spans(text)
            .into_iter()
            .map(|span| (span, RUN_TIME)),
    );
    // The two scans are independent, and the protocol requires source order.
    spans.sort_by_key(|(span, _)| span.start);

    let index = LineIndex::new(text);
    let mut out = Vec::with_capacity(spans.len());
    let (mut last_line, mut last_start) = (0, 0);
    for (span, token_type) in spans {
        let range = index.span_to_range(span);
        // A token the protocol cannot express: its encoding carries a single
        // `length`, so a span crossing a line break has no representation.
        // Placeholders do not span lines in practice, and dropping one is
        // better than emitting a length that would paint over the wrong text.
        if range.start.line != range.end.line {
            continue;
        }
        let delta_line = range.start.line - last_line;
        let delta_start = if delta_line == 0 {
            range.start.character - last_start
        } else {
            range.start.character
        };
        out.push(SemanticToken {
            delta_line,
            delta_start,
            length: range.end.character - range.start.character,
            token_type,
            token_modifiers_bitset: 0,
        });
        last_line = range.start.line;
        last_start = range.start.character;
    }
    out
}

/// Every `{{…}}` placeholder, as byte spans in source order.
///
/// This scan lives here rather than in `proef-core` deliberately. `{{…}}` is the
/// *engine's* spelling — core's rule is only that it passes through untouched
/// (ADR-0005) — and ADR-0002's amendment is that engine syntax does not
/// accumulate in the core. An editor feature is a consumer, so it is the right
/// place for a consumer's reading of it.
///
/// Nesting is not a case: `{{` opens and the first `}}` closes. A single `{` or
/// `}` is ordinary text — hurl's own templating requires the doubled form — and
/// an unclosed `{{` is left unlit rather than guessed at, on the same reasoning
/// `first_reference` uses for an unclosed `${`.
fn run_time_spans(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut rest = 0;
    while let Some(open) = text[rest..].find("{{") {
        let start = rest + open;
        let Some(close) = text[start + 2..].find("}}") else {
            break; // unclosed — literal text, not a placeholder
        };
        let end = start + 2 + close + 2;
        spans.push(Span { start, end });
        rest = end;
    }
    spans
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{LOWER_TIME, RUN_TIME, TOKEN_TYPES, encode, run_time_spans};

    /// The indices are only meaningful against the announced legend, so every
    /// other test here is self-referential without this one: swapping the two
    /// constants leaves them all green, because they compare a constant to
    /// itself. This is the assertion that pins what the numbers *mean*.
    #[test]
    fn each_index_names_its_type_in_the_announced_legend() {
        assert_eq!(
            TOKEN_TYPES[LOWER_TIME as usize], "macro",
            "`${{…}}` is substituted before execution — that is what a macro is"
        );
        assert_eq!(
            TOKEN_TYPES[RUN_TIME as usize], "variable",
            "`{{{{…}}}}` is resolved during execution"
        );
    }

    /// Decode the delta stream back to `(line, character, length, type)` so the
    /// assertions read as positions rather than as arithmetic.
    fn decoded(text: &str) -> Vec<(u32, u32, u32, u32)> {
        let (mut line, mut start) = (0, 0);
        encode(text)
            .into_iter()
            .map(|t| {
                line += t.delta_line;
                start = if t.delta_line == 0 {
                    start + t.delta_start
                } else {
                    t.delta_start
                };
                (line, start, t.length, t.token_type)
            })
            .collect()
    }

    /// The claim the feature exists for: on one line, the two tiers get
    /// different types.
    #[test]
    fn the_two_tiers_get_different_token_types() {
        let tokens = decoded("GET ${url:base}/r/{{recordId}}\n");
        assert_eq!(
            tokens,
            vec![
                (0, 4, 11, LOWER_TIME), // ${url:base}
                (0, 18, 12, RUN_TIME),  // {{recordId}}
            ],
            "each placeholder lit as its own tier"
        );
    }

    /// `$${` is a literal `${` — the escape ADR-0005 defines. Lighting it would
    /// tell the author proef will substitute text it will in fact leave alone,
    /// which is worse than no highlighting at all.
    #[test]
    fn an_escaped_reference_is_not_a_token() {
        assert!(
            decoded("literal $${notavar} stays\n").is_empty(),
            "an escaped `${{` must stay dark"
        );
    }

    /// Ordering is a protocol requirement, and the two scans are independent —
    /// so a run-time placeholder before a lower-time one is the case that
    /// catches a missing sort.
    #[test]
    fn tokens_are_emitted_in_source_order() {
        let tokens = decoded("{{first}} then ${second}\n");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].3, RUN_TIME, "the earlier one comes first");
        assert_eq!(tokens[1].3, LOWER_TIME);
        assert!(tokens[0].1 < tokens[1].1);
    }

    /// Deltas are relative to the previous token, and the relative-to-what
    /// changes at a line break. Getting this wrong paints the right colours in
    /// the wrong places, which no type checker can see.
    #[test]
    fn deltas_reset_across_lines() {
        let text = "GET ${a}\nHeader: {{b}}\nBody ${c}\n";
        assert_eq!(
            decoded(text),
            vec![
                (0, 4, 4, LOWER_TIME),
                (1, 8, 5, RUN_TIME),
                (2, 5, 4, LOWER_TIME)
            ]
        );
    }

    #[test]
    fn an_unclosed_placeholder_is_literal_text() {
        assert!(run_time_spans("a {{ b\n").is_empty(), "no closing `}}`");
        assert!(decoded("GET ${unclosed\n").is_empty(), "no closing brace");
        // A lone brace is ordinary text: hurl's templating needs the doubled
        // form, so `{x}` in a pack is a capture pattern, not a placeholder.
        assert!(run_time_spans("a {b} c\n").is_empty());
    }

    /// Multi-byte text ahead of a token: the protocol counts UTF-16 code units,
    /// and `LineIndex` owns that conversion. Without this the tokens drift right
    /// by one column per non-ASCII character before them on the line.
    #[test]
    fn columns_follow_the_protocols_units_not_bytes() {
        let tokens = decoded("# café ${x}\n");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].1, 7, "seven code units precede the `${{`");
    }
}
