//! Quick fixes: the structured half of a "did you mean" as an applicable edit.
//!
//! The analysis already decided what the fix is — `proef_core::diag::Fix`
//! carries a span and a replacement, attached only where the edit is certain
//! (`Diag::with_fix_replacing`). This module does no matching of its own; it
//! converts spans to ranges and hands the client an edit.
//!
//! Fixes are read back off the analysis rather than out of the request's
//! `context.diagnostics`. The client echoes the diagnostics it currently holds,
//! which is a snapshot of whatever the last publish told it — and it carries no
//! fix, since LSP's `Diagnostic` has nowhere to put one. The analysis is the
//! authority on both, and it is cached, so reading it is what the request costs.

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionResponse, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::analysis::Analysis;
use crate::convert::LineIndex;
use crate::documents::url_to_name;

/// Every quick fix reachable from `range` in the document at `url` — reachable
/// meaning the cursor is on the diagnostic *or* on the text the fix rewrites.
///
/// Both, because the two are regularly not the same place: a `use:` error
/// carets the macro's name key while the misspelled target sits several lines
/// below. An author who has just typed the typo is at the token; an author
/// following the squiggle is at the diagnostic. Offering at only one of them
/// hides the fix from half the ways of arriving at it.
///
/// Overlap, not containment: a client asks with the cursor's own (often empty)
/// range, and an author fixing a typo has the caret *inside* the offending
/// token, not selecting it.
pub fn actions(analysis: &Analysis, url: &Uri, range: Range) -> Vec<CodeActionResponse> {
    let name = url_to_name(url);
    let Some(raw) = analysis.raw.get(&name) else {
        return Vec::new();
    };
    let Some(diags) = analysis.suite.diagnostics.get(&name) else {
        return Vec::new();
    };
    let index = LineIndex::new(raw);

    diags
        .iter()
        .filter_map(|diag| {
            let fix = diag.fix.as_ref()?;
            let edit = index.span_to_range(fix.span);
            let caret = diag.span.map(|s| index.span_to_range(s));
            let reachable =
                overlaps(edit, range) || caret.is_some_and(|caret| overlaps(caret, range));
            reachable.then(|| {
                CodeActionResponse::CodeAction(CodeAction {
                    title: fix.title.clone(),
                    kind: Some(CodeActionKind::QuickFix),
                    // Naming the diagnostic is what lets an editor strike it
                    // through, group the action under it, and clear it on apply.
                    diagnostics: Some(vec![super::diagnostics::to_lsp(diag, &index)]),
                    // A did-you-mean is the only fix proef offers for its own
                    // diagnostic, so it is the one an editor should reach for
                    // under "fix all" or a single-key apply.
                    is_preferred: Some(true),
                    edit: Some(WorkspaceEdit {
                        changes: Some(
                            [(
                                url.clone(),
                                vec![TextEdit {
                                    range: edit,
                                    new_text: fix.replacement.clone(),
                                }],
                            )]
                            .into_iter()
                            .collect(),
                        ),
                        ..WorkspaceEdit::default()
                    }),
                    ..CodeAction::default()
                })
            })
        })
        .collect()
}

/// Do two ranges share any position? Touching at a point counts: an empty
/// cursor range sitting exactly at a token's start is inside that token as far
/// as an author is concerned.
fn overlaps(a: Range, b: Range) -> bool {
    let key = |p: lsp_types::Position| (p.line, p.character);
    key(a.start) <= key(b.end) && key(b.start) <= key(a.end)
}
