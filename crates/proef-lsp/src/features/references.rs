//! Find-references: a macro (from a step that uses it, or from its definition)
//! → every feature step bound to it. Falls straight out of the binding index.

use lsp_types::{Location, Position, Uri};

use crate::analysis::Analysis;
use crate::convert::LineIndex;
use crate::documents::{name_to_url, url_to_name};

/// Resolves `position` in the document at `url` to the macro under the
/// cursor — from either side of the binding — then returns a [`Location`]
/// for every feature step bound to that macro.
///
/// The cursor resolves to a macro name via either:
///   - a feature step whose `step_span` contains the offset (its `macro_name`), or
///   - a pack position inside a macro's `def_span` (that macro's `name`).
pub fn find(analysis: &Analysis, url: &Uri, position: Position) -> Vec<Location> {
    let name = url_to_name(url);
    let Some(raw) = analysis.raw.get(&name) else {
        return Vec::new();
    };
    let offset = LineIndex::new(raw).position_to_offset(position);

    // Resolve the macro under the cursor: either a step in a feature, or the
    // macro's name key in a pack. Span containment is half-open — spans are
    // byte offsets, end-exclusive.
    let macro_name = analysis
        .suite
        .bindings
        .iter()
        .find(|b| b.feature == name && b.step_span.start <= offset && offset < b.step_span.end)
        .map(|b| b.macro_name.clone())
        .or_else(|| {
            analysis.suite.macros.iter().find_map(|m| {
                let span = m.def_span?;
                (m.pack == name && span.start <= offset && offset < span.end)
                    .then(|| m.name.clone())
            })
        });
    let Some(macro_name) = macro_name else {
        return Vec::new();
    };

    analysis
        .suite
        .bindings
        .iter()
        .filter(|b| b.macro_name == macro_name)
        .filter_map(|b| {
            let feat_url = name_to_url(&b.feature)?;
            let feat_raw = analysis.raw.get(&b.feature)?;
            let range = LineIndex::new(feat_raw).span_to_range(b.step_span);
            Some(Location {
                uri: feat_url,
                range,
            })
        })
        .collect()
}
