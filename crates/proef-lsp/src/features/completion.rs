//! Completion: in a feature step, offer macro `match:` patterns as snippets,
//! ranked by the same "did you mean" substrate that powers unbound-step help.

use std::fmt::Write as _;

use lsp_types::{CompletionItem, CompletionItemKind, InsertTextFormat, Position, Uri};
use proef_core::matcher;

use crate::analysis::Analysis;
use crate::convert::{LineIndex, normalize};
use crate::documents::url_to_name;

/// Turn a `match:` pattern into an LSP snippet: each `{capture}` becomes a
/// numbered tabstop `${n:capture}`.
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
                name.push(c2);
            }
            // `write!` to a `String` is infallible; nothing to propagate.
            let _ = write!(out, "${{{tab}:{name}}}");
            tab += 1;
        } else {
            out.push(c);
        }
    }
    out
}

/// Offers every macro `match:` pattern in the suite as a ranked snippet
/// completion at `position` in the document at `url`. Ranking puts the
/// pattern closest to the already-typed prose first (the same substrate
/// `proef_core::matcher` uses for "did you mean" unbound-step diagnostics);
/// the rest keep their suite order.
pub fn complete(analysis: &Analysis, url: &Uri, position: Position) -> Vec<CompletionItem> {
    let name = url_to_name(url);
    let Some(raw) = analysis.raw.get(&name) else {
        return Vec::new();
    };
    // The prose typed so far on this line, after the Gherkin keyword.
    let prefix = current_step_prefix(raw, position);

    // Rank patterns by closeness to the prefix (best first), like unbound-step
    // suggestions. Patterns with no `match:` (use-only macros) are skipped.
    let mut patterns: Vec<(&str, &str)> = analysis
        .suite
        .macros
        .iter()
        .filter_map(|m| m.pattern.as_deref().map(|p| (m.name.as_str(), p)))
        .collect();
    if let Some(best) = matcher::closest(&prefix, patterns.iter().map(|(_, p)| *p)) {
        patterns.sort_by_key(|(_, p)| i32::from(*p != best));
    }

    patterns
        .into_iter()
        .map(|(macro_name, pattern)| CompletionItem {
            label: pattern.to_owned(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(format!("macro {macro_name}")),
            insert_text: Some(pattern_to_snippet(pattern)),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
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
