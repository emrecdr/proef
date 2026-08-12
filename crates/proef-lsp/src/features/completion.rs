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

/// Is the cursor inside a `bind:` table — either flow (`bind: { a: 1 }`) or
/// block style?
///
/// Block style is decided by YAML's own rule rather than by a regex: the
/// nearest non-blank line above with a *smaller* indent is this line's parent,
/// and we are in a bind table exactly when that parent is `bind:`. Which also
/// means the `bind:` line itself answers no — its own parent is the step.
fn cursor_is_in_bind(raw: &str, position: Position) -> bool {
    let row = position.line as usize;
    // `nth`, as `complete_fragment_ref` does: this runs per completion request,
    // so the flow-style answer below should not first materialise every line of
    // the document.
    let Some(line) = raw.lines().nth(row) else {
        return false;
    };
    // Anchored like `complete_fragment_ref`'s `ref:` check, not a substring
    // search: `rebind: {…}` is not a bind table, and a value that merely
    // contains `bind:` is not one either.
    let trimmed = line.trim_start();
    if trimmed.starts_with("bind:")
        && let Some(brace) = line.find('{')
        && let Some(close) = line.rfind('}')
    {
        // `position.character` counts UTF-16 code units; `find`/`rfind` return
        // byte offsets. Comparing them directly is correct only while the line
        // is all-ASCII, so the cursor goes through the bridge that owns this
        // conversion (`convert.rs`) rather than around it: the difference of two
        // document offsets is the cursor's byte column within the line.
        let index = LineIndex::new(raw);
        let line_start = index.position_to_offset(Position {
            line: position.line,
            character: 0,
        });
        let at = index
            .position_to_offset(position)
            .saturating_sub(line_start);
        return at > brace && at <= close;
    }
    let indent = |s: &str| s.len() - s.trim_start().len();
    let here = indent(line);
    // The parent is the *nearest* line above with a smaller indent. Taken as
    // the last such line scanning forward rather than the first scanning back,
    // which is the same line without needing a reversible (i.e. collected)
    // iterator.
    raw.lines()
        .take(row)
        .filter(|l| !l.trim().is_empty() && indent(l) < here)
        .last()
        .is_some_and(|parent| parent.trim() == "bind:")
}

/// Placeholder completions when the cursor sits inside a `bind:` table.
/// `None` when it does not, so the caller falls through to step completion.
///
/// What a fragment reads is taken off the engine's own AST at scan time, so
/// this offers the file's real interface rather than a second description that
/// could disagree with it. Without it, the only route to a foreign corpus's
/// variable names is to run the suite and read
/// `proef::lower::unbound_placeholder` — a *lower-time* error, so today the
/// names arrive only after a failure.
///
/// Every fragment this pack refs contributes, ranked by how near its `ref:`
/// line is to the cursor. `bind:` exists at three scopes (pack, macro, step)
/// and only the step one names a single fragment unambiguously, so narrowing
/// to one would be wrong in two cases out of three; the owning fragment rides
/// in `detail` instead, which keeps a union readable.
fn complete_bind_key(
    analysis: &Analysis,
    pack: &str,
    raw: &str,
    position: Position,
) -> Option<Vec<CompletionItem>> {
    if !cursor_is_in_bind(raw, position) {
        return None;
    }
    let index = LineIndex::new(raw);
    let cursor = position.line as usize;
    let mut refs: Vec<(usize, &str)> = analysis
        .suite
        .fragment_refs
        .iter()
        .filter(|r| r.pack == pack)
        .map(|r| {
            let line = index.span_to_range(r.span).start.line as usize;
            (line.abs_diff(cursor), r.target_fragment.as_str())
        })
        .collect();
    // Distance first, then name: two `ref:`s equidistant from the cursor must
    // not order by whichever the index happened to visit first.
    refs.sort_unstable();

    let defs: std::collections::BTreeMap<&str, &proef_core::analyze::FragmentDef> = analysis
        .suite
        .fragments
        .iter()
        .map(|f| (f.name.as_str(), f))
        .collect();

    let mut seen = std::collections::BTreeSet::new();
    let mut items: Vec<CompletionItem> = Vec::new();
    for (_, fragment) in refs {
        let Some(def) = defs.get(fragment) else {
            continue;
        };
        for placeholder in &def.placeholders {
            // A variable the fragment supplies itself needs no `bind:` and may
            // not have one, so offering it would propose an edit the next
            // `option_declared_twice` rejects. The list is what still needs a
            // value, not everything the file mentions.
            if def.supplied_variables.contains(placeholder) {
                continue;
            }
            if !seen.insert(placeholder.as_str()) {
                continue;
            }
            let rank = items.len();
            items.push(CompletionItem {
                label: placeholder.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: Some(format!("read by {fragment}")),
                insert_text: Some(format!("{placeholder}: ")),
                // Nearest `ref:` first, so the step-scope case reads as exact
                // even though the list is a union.
                sort_text: Some(format!("{rank:04}")),
                ..CompletionItem::default()
            });
        }
    }
    Some(items)
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
    // A `bind:` table completes against what the fragments this pack refs
    // actually read — again a pack vocabulary, never step prose.
    if let Some(items) = complete_bind_key(analysis, &name, raw, position) {
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

#[cfg(test)]
mod bind_scope_tests {
    use super::cursor_is_in_bind;
    use lsp_types::Position;

    fn at(raw: &str, line: u32, character: u32) -> bool {
        cursor_is_in_bind(raw, Position { line, character })
    }

    /// Whether the cursor is in a `bind:` table decides whether an author is
    /// offered a fragment's variables or a list of step patterns. Both styles
    /// occur in real packs, and the near-misses (the `bind:` line itself, a
    /// sibling key) must answer no or the wrong vocabulary appears.
    #[test]
    fn a_bind_table_is_recognised_in_both_yaml_styles() {
        let block = "macros:\n  m:\n    steps:\n      - ref: f\n        bind:\n          q: 1\n";
        assert!(at(block, 5, 10), "a child of `bind:` is inside it");
        assert!(
            !at(block, 4, 13),
            "the `bind:` line itself is not inside its own table"
        );
        assert!(!at(block, 3, 20), "a `ref:` line is not a bind table");

        let flow = "macros:\n  m:\n    bind: { q: 1 }\n";
        assert!(at(flow, 2, 13), "past the brace is inside the flow table");
        // The flow branch compares the cursor against byte offsets from
        // `find`/`rfind`, but LSP columns are UTF-16 code units. On an all-ASCII
        // line the two agree and the difference is invisible; a multi-byte value
        // is where they diverge, so the non-ASCII case is the one that pins it.
        let wide = "macros:\n  m:\n    bind: { q: \"café ☕\" }\n";
        assert!(
            at(wide, 2, 20),
            "a cursor after multi-byte text is still inside the table"
        );
        // `}` is UTF-16 column 24 but byte 27, so the old byte-vs-column
        // comparison called every column up to 27 "inside" and kept offering
        // bind keys well past the end of the table.
        assert!(
            !at(wide, 2, 25),
            "past the closing brace is outside, counting UTF-16 not bytes"
        );
        assert!(
            !at(flow, 2, 6),
            "on the key itself is not yet inside the table"
        );
        assert!(!at(flow, 2, 18), "past the closing brace is outside again");

        // The key is matched anchored, not as a substring: another key that
        // merely ends in `bind:` is a different setting entirely.
        let lookalike = "macros:\n  m:\n    rebind: { q: 1 }\n";
        assert!(!at(lookalike, 2, 15), "`rebind:` is not `bind:`");

        // A sibling key nested just as deeply, under a different parent.
        let sibling = "macros:\n  m:\n    defaults:\n      index: records\n";
        assert!(!at(sibling, 3, 12), "`defaults:` is not `bind:`");

        // Out of range never panics and never claims to be in a table.
        assert!(!at(block, 99, 0));
    }
}
