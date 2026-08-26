//! Hover: what the thing under the cursor resolves to, without going there.
//!
//! The same three targets go-to-definition resolves — a bound step, a `use:`,
//! a `ref:` — answered in place. Go-to-definition costs a jump and a jump back;
//! hover is the cheaper question ("what does this bind to?") that an author
//! asks far more often than "take me there".
//!
//! Every fact rendered here already exists in the analysis. Nothing is
//! re-derived, so a hover cannot disagree with the diagnostic on the same line.

use std::fmt::Write as _;

use lsp_types::{Contents, Hover, MarkupContent, MarkupKind, Position, Uri};

use crate::analysis::Analysis;
use crate::convert::LineIndex;
use crate::documents::url_to_name;

/// What `position` in the document at `url` resolves to, or `None` when the
/// cursor is not on something the suite knows.
pub fn hover(analysis: &Analysis, url: &Uri, position: Position) -> Option<Hover> {
    let name = url_to_name(url);
    let raw = analysis.raw.get(&name)?;
    let index = LineIndex::new(raw);
    let offset = index.position_to_offset(position);

    // A `ref:` first, for the reason `definition::goto` checks it first: it is
    // the only target that lives in another file, so it cannot share the macro
    // lookup below.
    let (markdown, span) =
        fragment_at(analysis, &name, offset).or_else(|| step_or_use_at(analysis, &name, offset))?;

    Some(Hover {
        contents: Contents::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        // Naming the range is what makes an editor highlight the token rather
        // than guess at a word boundary — and proef's tokens (`task.search`)
        // are not what a word-boundary guess would pick.
        range: Some(index.span_to_range(span)),
    })
}

/// The fragment a `ref:` under the cursor resolves to: where it lives and what
/// it still needs bound.
fn fragment_at(
    analysis: &Analysis,
    pack: &str,
    offset: usize,
) -> Option<(String, proef_core::diag::Span)> {
    let reference = analysis
        .suite
        .fragment_refs
        .iter()
        .find(|r| r.pack == pack && r.span.start <= offset && offset < r.span.end)?;
    let fragment = analysis
        .suite
        .fragments
        .iter()
        .find(|f| f.name == reference.target_fragment)?;

    let mut out = format!("**fragment** `{}`\n\nin `{}`", fragment.name, fragment.file);
    // What still needs a `bind:` — the same subtraction the `bind:` completion
    // makes, and the question an author on a `ref:` line is actually asking. A
    // variable the fragment supplies itself is not one of them.
    let needs: Vec<&str> = fragment
        .placeholders
        .iter()
        .filter(|p| !fragment.supplied_variables.contains(p))
        .map(String::as_str)
        .collect();
    if needs.is_empty() {
        out.push_str("\n\nneeds no `bind:`");
    } else {
        let _ = write!(out, "\n\nbinds: `{}`", needs.join("`, `"));
    }
    Some((out, reference.span))
}

/// The macro a feature step binds, or the macro a `use:` targets — the two
/// paths that end at the same rendering.
fn step_or_use_at(
    analysis: &Analysis,
    name: &str,
    offset: usize,
) -> Option<(String, proef_core::diag::Span)> {
    let (macro_name, span) = analysis
        .suite
        .bindings
        .iter()
        .find(|b| b.feature == name && b.step_span.start <= offset && offset < b.step_span.end)
        .map(|b| (b.macro_name.as_str(), b.step_span))
        .or_else(|| {
            analysis
                .suite
                .use_refs
                .iter()
                .find(|u| u.pack == name && u.span.start <= offset && offset < u.span.end)
                .map(|u| (u.target_macro.as_str(), u.span))
        })?;
    let m = analysis
        .suite
        .macros
        .iter()
        .find(|m| m.name == macro_name)?;

    let mut out = format!("**macro** `{}`\n\nin `{}`", m.name, m.pack);
    if let Some(pattern) = &m.pattern {
        let _ = write!(out, "\n\n```gherkin\n{pattern}\n```");
    }
    if !m.params.is_empty() {
        let _ = write!(out, "\n\nparams: `{}`", m.params.join("`, `"));
    }
    Some((out, span))
}
