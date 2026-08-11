//! Completion: in a feature step, offer macro `match:` patterns as snippets,
//! ranked by relevance to the partially-typed step prose via
//! `matcher::prefix_rank`; nothing is filtered server-side (the client
//! narrows via filterText).

use std::fmt::Write as _;

use lsp_types::{CompletionItem, CompletionItemKind, InsertTextFormat, Position, Uri};
use proef_core::matcher;

use crate::analysis::Analysis;
use crate::convert::{LineIndex, normalize};
use crate::documents::url_to_name;

/// Fragment-name completions when the cursor sits on a pack's `ref:` line.
/// `None` when it does not, so the caller falls through to step completion.
///
/// Keyed on the line's own text rather than on an index of `ref:` spans: an
/// author mid-type has written `ref: ` and nothing after it, which parses to no
/// step at all and so appears in no index.
fn complete_fragment_ref(
    analysis: &Analysis,
    raw: &str,
    position: Position,
) -> Option<Vec<CompletionItem>> {
    let line = raw.lines().nth(position.line as usize)?;
    let trimmed = line.trim_start();
    let after_dash = trimmed.strip_prefix("- ").unwrap_or(trimmed);
    let typed = after_dash.strip_prefix("ref:")?.trim();

    // No `sort_text`: fragments are indexed from a name-keyed `BTreeMap`, so this
    // list is already in label order and the labels are unique. Pinning a rank
    // here would only restate the client's own default ordering — and the
    // ranked completions elsewhere in this file need `sort_text` precisely
    // because *their* order is not the label's.
    Some(
        analysis
            .suite
            .fragments
            .iter()
            .filter(|f| typed.is_empty() || f.name.starts_with(typed))
            .map(|f| CompletionItem {
                label: f.name.clone(),
                kind: Some(CompletionItemKind::REFERENCE),
                detail: Some(format!("fragment in {}", f.file)),
                insert_text: Some(f.name.clone()),
                ..CompletionItem::default()
            })
            .collect(),
    )
}

/// Escape one literal character for LSP snippet syntax.
///
/// `$`, `}` and `\` are the syntax's own metacharacters, so a pattern that
/// merely *contains* one changes the snippet's shape: prose like
/// `the price is $5` makes the client read `$5` as tabstop 5 and drop the text.
/// Ordinary sentences carry `$`, which is exactly what these patterns are.
fn push_escaped(out: &mut String, c: char) {
    if matches!(c, '$' | '}' | '\\') {
        out.push('\\');
    }
    out.push(c);
}

/// Turn a `match:` pattern into an LSP snippet: each `{capture}` becomes a
/// numbered tabstop `${n:capture}`. Everything else is literal text and is
/// escaped as such — only the tabstops this function writes are syntax.
fn pattern_to_snippet(pattern: &str) -> String {
    let mut out = String::new();
    let mut tab = 1;
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            for c2 in chars.by_ref() {
                if c2 == '}' {
                    break;
                }
                push_escaped(&mut name, c2);
            }
            // `write!` to a `String` is infallible; nothing to propagate.
            let _ = write!(out, "${{{tab}:{name}}}");
            tab += 1;
        } else {
            push_escaped(&mut out, c);
        }
    }
    out
}

/// Offers every macro `match:` pattern in the suite as a ranked snippet
/// completion at `position` in the document at `url`. Every pattern is
/// returned — nothing is hidden — but each carries a `sort_text` from its
/// rank by `proef_core::matcher::prefix_rank` (the closer to the typed prose
/// prefix, the earlier it sorts; ties keep suite order) and a `filter_text`
/// set to the pattern's literal skeleton, so the client narrows on prose
/// rather than on the raw snippet text.
pub fn complete(analysis: &Analysis, url: &Uri, position: Position) -> Vec<CompletionItem> {
    let name = url_to_name(url);
    let Some(raw) = analysis.raw.get(&name) else {
        return Vec::new();
    };
    // A pack's `ref:` line completes against fragment names, not step prose —
    // a different vocabulary in a different file, so it answers on its own and
    // never mixes with the macro patterns below.
    if let Some(items) = complete_fragment_ref(analysis, raw, position) {
        return items;
    }

    // The prose typed so far on this line, after the Gherkin keyword.
    let prefix = current_step_prefix(raw, position);

    // Rank patterns by relevance to the typed prose prefix (best first);
    // equal ranks keep suite order (stable sort). Patterns with no `match:`
    // (use-only macros) are skipped.
    let mut patterns: Vec<(&str, &str)> = analysis
        .suite
        .macros
        .iter()
        .filter_map(|m| m.pattern.as_deref().map(|p| (m.name.as_str(), p)))
        .collect();
    patterns.sort_by_key(|(_, p)| matcher::prefix_rank(&prefix, p));

    // Zero-pad the rank index so the client's lexical sortText comparison
    // matches our numeric order (e.g. "00", "01", … for double-digit item counts).
    let width = patterns.len().to_string().len();
    patterns
        .into_iter()
        .enumerate()
        .map(|(i, (macro_name, pattern))| CompletionItem {
            label: pattern.to_owned(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(format!("macro {macro_name}")),
            insert_text: Some(pattern_to_snippet(pattern)),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            sort_text: Some(format!("{i:0width$}")),
            filter_text: Some(matcher::literal_skeleton(pattern)),
            ..Default::default()
        })
        .collect()
}

/// The text between the Gherkin keyword and the cursor on the current line.
fn current_step_prefix(raw: &str, position: Position) -> String {
    let idx = LineIndex::new(raw);
    let end = idx.position_to_offset(position);
    // Walk back to the start of the line in normalized coordinates — `end` is
    // a normalized-text offset, so the line search must run over normalized
    // text too (the one-implementation `normalize`, not a second copy of it).
    let normalized = normalize(raw);
    let end = end.min(normalized.len());
    let line_start = normalized[..end].rfind('\n').map_or(0, |n| n + 1);
    let line = normalized[line_start..end].trim_start();
    // Strip a leading Gherkin keyword if present.
    for kw in ["Given ", "When ", "Then ", "And ", "But ", "* "] {
        if let Some(rest) = line.strip_prefix(kw) {
            return rest.to_owned();
        }
    }
    line.to_owned()
}

#[cfg(test)]
mod snippet_tests {
    use super::pattern_to_snippet;

    /// A pattern is prose, and prose carries `$`. Unescaped, the client reads
    /// `$5` as tabstop 5 and drops the text — the completion silently inserts
    /// something the author never wrote.
    #[test]
    fn snippet_metacharacters_in_prose_are_escaped() {
        assert_eq!(
            pattern_to_snippet("the price is $5"),
            "the price is \\$5",
            "a literal `$` must not open a tabstop"
        );
        assert_eq!(pattern_to_snippet(r"a back\slash"), r"a back\\slash");

        // The tabstops this function writes are syntax and stay unescaped.
        assert_eq!(
            pattern_to_snippet("the operator searches for {term}"),
            "the operator searches for ${1:term}"
        );
        // Both at once. `${amount}` is a literal `$` followed by the capture
        // `{amount}`, so the `$` escapes and the tabstop it precedes does not.
        assert_eq!(
            pattern_to_snippet("pay ${amount} now"),
            "pay \\$${1:amount} now"
        );
    }
}
