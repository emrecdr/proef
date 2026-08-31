//! Document symbols: the outline an editor shows for one file.
//!
//! Two vocabularies, one per file kind — scenarios in a feature, macros in a
//! pack. Which one applies is decided by what the analysis *found* in the file,
//! not by its extension: a name is a feature or a pack because discovery said
//! so, and the LSP has no business re-deciding that from a suffix.

use lsp_types::{DocumentSymbol, DocumentSymbolResponse, Position, Range, SymbolKind, Uri};

use crate::analysis::Analysis;
use crate::convert::LineIndex;
use crate::documents::url_to_name;

/// The outline for the document at `url`, or `None` when it is neither a
/// discovered feature nor a loaded pack.
pub fn outline(analysis: &Analysis, url: &Uri) -> Option<DocumentSymbolResponse> {
    let name = url_to_name(url);
    let raw = analysis.raw.get(&name)?;
    let index = LineIndex::new(raw);

    let scenarios: Vec<DocumentSymbol> = analysis
        .suite
        .scenarios
        .iter()
        .filter(|s| s.feature == name)
        .map(|s| {
            // Core reports the header's 1-based line for display; the whole
            // line is the symbol, which is as precise as the parse makes
            // available and exactly what an outline needs.
            let range = whole_line(raw, s.line);
            symbol(
                s.name.clone(),
                tag_detail(&s.tags),
                // A scenario is the unit a run executes and a report names, so
                // `Method` — the kind editors render as a runnable-looking
                // entry — reads truer than `Object`.
                SymbolKind::Method,
                range,
            )
        })
        .collect();

    let macros: Vec<DocumentSymbol> = analysis
        .suite
        .macros
        .iter()
        .filter(|m| m.pack == name)
        .filter_map(|m| {
            let anchor = m.def_span?;
            let range = index.span_to_range(anchor);
            Some(symbol(
                m.name.clone(),
                m.pattern.clone(),
                SymbolKind::Function,
                range,
            ))
        })
        .collect();

    // A file is one or the other. Returning the empty list for a known file
    // with nothing in it is still an answer — it tells the editor the outline
    // is empty rather than unavailable.
    if scenarios.is_empty() && macros.is_empty() && !analysis.suite.diagnostics.contains_key(&name)
    {
        return None;
    }
    let mut symbols = scenarios;
    symbols.extend(macros);
    Some(DocumentSymbolResponse::DocumentSymbolList(symbols))
}

/// One symbol whose selection range is its whole extent — proef's names are
/// their own lines, so there is no narrower thing to reveal.
fn symbol(name: String, detail: Option<String>, kind: SymbolKind, range: Range) -> DocumentSymbol {
    #[allow(deprecated)] // `deprecated` is superseded by `tags`; both must be set.
    DocumentSymbol {
        name,
        detail,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children: None,
    }
}

/// The tags a scenario carries, rendered the way an author writes them — the
/// one piece of scenario metadata worth a glance in an outline (`@slow`,
/// `@skip`). `None` when there are none, so untagged scenarios stay clean.
fn tag_detail(tags: &[String]) -> Option<String> {
    (!tags.is_empty()).then(|| {
        tags.iter()
            .map(|t| format!("@{t}"))
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// The range covering the whole of `line` (1-based), clamped to what the raw
/// text has. Line numbers agree between raw and normalized text — normalization
/// strips a BOM and appends a trailing newline, neither of which moves an
/// earlier line — so a core-reported line needs no conversion, only bounds.
fn whole_line(raw: &str, line: usize) -> Range {
    let row = u32::try_from(line.saturating_sub(1)).unwrap_or(u32::MAX);
    let width = raw
        .lines()
        .nth(line.saturating_sub(1))
        .map_or(0, |l| u32::try_from(l.chars().count()).unwrap_or(u32::MAX));
    Range {
        start: Position {
            line: row,
            character: 0,
        },
        end: Position {
            line: row,
            character: width,
        },
    }
}
