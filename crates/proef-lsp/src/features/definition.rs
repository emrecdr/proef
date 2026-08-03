//! Go-to-definition: a feature step's macro → the macro's definition anchor.
//!
//! Deviation from spec §7, deliberate: the spec says definition targets the
//! macro's `match:` line. No `match:`-line span exists in `proef-core`
//! (`serde_norway` yields no spans for valid YAML), and a text-scanning
//! fallback for it is out of scope. `MacroRef::def_span` is the macro's
//! *name-key* span — the idiomatic "jump to the symbol's definition" target,
//! and the only anchor the data model actually carries. Landing precisely on
//! `match:`, and go-to-def *from* a `use:` reference (no `use:` span exists
//! either), are noted future refinements, not v1.

use lsp_types::{Location, Position, Uri};

use crate::analysis::Analysis;
use crate::convert::LineIndex;
use crate::documents::{name_to_url, url_to_name};

/// Resolves `position` in the document at `url` to the definition location of
/// the macro the enclosing step is bound to, or `None` if the cursor is not
/// inside a bound step or the macro has no locatable definition.
pub fn goto(analysis: &Analysis, url: &Uri, position: Position) -> Option<Location> {
    let name = url_to_name(url);
    let raw = analysis.raw.get(&name)?;
    let offset = LineIndex::new(raw).position_to_offset(position);

    // Which bound step's span contains the cursor?
    let macro_name = analysis
        .suite
        .bindings
        .iter()
        .find(|b| b.feature == name && b.step_span.start <= offset && offset < b.step_span.end)
        .map(|b| b.macro_name.as_str())?;

    // The macro's definition anchor (its name key in the pack).
    let m = analysis
        .suite
        .macros
        .iter()
        .find(|m| m.name == macro_name)?;
    let def_span = m.def_span?;
    let pack_url = name_to_url(&m.pack)?;
    let pack_raw = analysis.raw.get(&m.pack)?;
    let range = LineIndex::new(pack_raw).span_to_range(def_span);
    Some(Location {
        uri: pack_url,
        range,
    })
}
