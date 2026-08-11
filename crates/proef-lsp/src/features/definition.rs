//! Go-to-definition: two paths onto a macro's definition anchor.
//!
//! Path 1 — cursor on a feature step → the macro it binds. Path 2 — cursor on
//! a `use:` line inside a pack → the macro that reference resolves to. Both
//! paths funnel through `macro_location`, the single rule for a macro's
//! landing anchor: its `match:` line when locatable, else its name key.

use lsp_types::{Location, Position, Uri};

use crate::analysis::Analysis;
use crate::convert::LineIndex;
use crate::documents::{name_to_url, url_to_name};

/// Resolves `position` in the document at `url` to a macro's definition
/// location — either the macro a bound feature step resolves to, or the
/// target of a `use:` reference the cursor sits on — or `None` if neither
/// path matches or the macro has no locatable definition.
pub fn goto(analysis: &Analysis, url: &Uri, position: Position) -> Option<Location> {
    let name = url_to_name(url);
    let raw = analysis.raw.get(&name)?;
    let offset = LineIndex::new(raw).position_to_offset(position);

    // Path 3 — cursor on a `ref:` line in a pack → the fragment's annotation in
    // its own `.hurl` file. Checked first because it lands in a *different*
    // file than the macro paths, so it cannot share `macro_location`.
    if let Some(location) = fragment_location(analysis, &name, offset) {
        return Some(location);
    }

    // Path 1 — cursor on a feature step → the macro it binds. Path 2 (fallback) —
    // cursor on a `use:` line in a pack → the referenced macro. A document name is
    // discovered as either a feature or a pack, so the two keys never both match.
    let macro_name = analysis
        .suite
        .bindings
        .iter()
        .find(|b| b.feature == name && b.step_span.start <= offset && offset < b.step_span.end)
        .map(|b| b.macro_name.as_str())
        .or_else(|| {
            analysis
                .suite
                .use_refs
                .iter()
                .find(|u| u.pack == name && u.span.start <= offset && offset < u.span.end)
                .map(|u| u.target_macro.as_str())
        })?;
    macro_location(analysis, macro_name)
}

/// A `Location` at the annotation line of the fragment a `ref:` under the
/// cursor resolves to. `None` when the cursor is not on a `ref:` line, or the
/// fragment file's text was not captured (an unreadable file — already
/// diagnosed).
fn fragment_location(analysis: &Analysis, pack: &str, offset: usize) -> Option<Location> {
    let target = analysis
        .suite
        .fragment_refs
        .iter()
        .find(|r| r.pack == pack && r.span.start <= offset && offset < r.span.end)
        .map(|r| r.target_fragment.as_str())?;
    let fragment = analysis.suite.fragments.iter().find(|f| f.name == target)?;
    let raw = analysis.raw.get(&fragment.file)?;
    let range = LineIndex::new(raw).span_to_range(fragment.span?);
    Some(Location {
        uri: name_to_url(&fragment.file)?,
        range,
    })
}

/// A `Location` at `macro_name`'s definition anchor — its `match:` line when
/// locatable, else its name key — in the pack that defines it.
fn macro_location(analysis: &Analysis, macro_name: &str) -> Option<Location> {
    let m = analysis
        .suite
        .macros
        .iter()
        .find(|m| m.name == macro_name)?;
    let anchor = m.match_span.or(m.def_span)?;
    let pack_url = name_to_url(&m.pack)?;
    let pack_raw = analysis.raw.get(&m.pack)?;
    let range = LineIndex::new(pack_raw).span_to_range(anchor);
    Some(Location {
        uri: pack_url,
        range,
    })
}
