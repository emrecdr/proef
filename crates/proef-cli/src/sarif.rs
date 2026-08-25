//! SARIF 2.1.0 export of validation diagnostics — the `--dry-run` shift-left
//! gate. Each `Diag` maps ~1:1 to a SARIF result (code → `ruleId`, byte `Span` →
//! `region.byteOffset`/`byteLength`, severity → `level`), so unbound steps and
//! pack-lint findings surface as inline annotations in a PR. A parallel
//! serializer to `render::print_all(&[Diag])`; dry-run diagnostics resolve no
//! secrets, so nothing here needs redaction.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use proef_core::diag::{Diag, Severity};

/// Write a SARIF 2.1.0 log of `diags` to `path`.
pub fn write(diags: &[&Diag], path: &Path) -> Result<(), String> {
    // The closed diagnostic-code set drives `rules[]` (deduplicated, ordered).
    let rules: Vec<serde_json::Value> = diags
        .iter()
        .map(|d| d.code)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|id| serde_json::json!({ "id": id }))
        .collect();
    // Byte offsets alone upload fine and annotate nothing: GitHub keys inline
    // annotations on `startLine`. Sources are read once each, here at the IO
    // edge, and only to count newlines — `Diag` keeps carrying byte spans.
    let mut sources: BTreeMap<&str, Option<String>> = BTreeMap::new();
    for diag in diags {
        if let Some(name) = &diag.source_name {
            sources
                .entry(name.as_str())
                .or_insert_with(|| std::fs::read_to_string(name).ok());
        }
    }
    let results: Vec<serde_json::Value> = diags.iter().map(|d| result(d, &sources)).collect();
    let sarif = serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": { "name": "proef", "rules": rules } },
            "results": results,
        }],
    });
    let text = serde_json::to_string_pretty(&sarif).map_err(|err| err.to_string())?;
    crate::fsutil::create_parents(path)
        .map_err(|err| format!("cannot create directory for {}: {err}", path.display()))?;
    std::fs::write(path, text).map_err(|err| format!("cannot write {}: {err}", path.display()))
}

/// The 1-based line containing byte `offset`.
///
/// Counts newlines only. `LineCol.column` is char-counted and must never enter
/// byte math (CLAUDE.md); nothing here needs a column, so nothing computes one.
fn line_of(text: &str, offset: usize) -> usize {
    text.as_bytes()
        .iter()
        .take(offset.min(text.len()))
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

fn result(diag: &Diag, sources: &BTreeMap<&str, Option<String>>) -> serde_json::Value {
    let level = match diag.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    let text = match &diag.help {
        Some(help) => format!("{}\nhelp: {help}", diag.message),
        None => diag.message.clone(),
    };
    let mut result = serde_json::json!({
        "ruleId": diag.code,
        "level": level,
        "message": { "text": text },
    });
    // A byte span + source name give a physical location; spanless diagnostics
    // stay run-level (still a valid SARIF result).
    if let (Some(name), Some(span)) = (&diag.source_name, &diag.span) {
        let mut region = serde_json::json!({
            "byteOffset": span.start,
            "byteLength": span.end.saturating_sub(span.start),
        });
        // Line numbers only when the source could actually be read — a guessed
        // line is worse than none, since the annotation would land on innocent
        // code and read as authoritative.
        if let Some(Some(text)) = sources.get(name.as_str()) {
            region["startLine"] = line_of(text, span.start).into();
            region["endLine"] = line_of(text, span.end).into();
        }
        result["locations"] = serde_json::json!([{
            "physicalLocation": {
                "artifactLocation": { "uri": name },
                "region": region,
            }
        }]);
    }
    result
}

#[cfg(test)]
mod line_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{line_of, write};
    use proef_core::diag::Diag;

    /// GitHub keys inline annotations on `startLine`; a byte offset alone
    /// uploads fine and annotates nothing, which is why the flag looked wired
    /// up and delivered nothing.
    #[test]
    fn the_line_is_counted_from_newlines_before_the_offset() {
        let text = "one\ntwo\nthree\n";
        assert_eq!(line_of(text, 0), 1);
        assert_eq!(line_of(text, 4), 2, "first byte of line 2");
        assert_eq!(line_of(text, 8), 3);
        // Past the end clamps rather than panicking — a span can outrun a
        // source the caller re-read after an edit.
        assert_eq!(line_of(text, 9_999), 4);
    }

    /// Multi-byte characters must not shift the count: newlines are bytes and
    /// the span is a byte offset, so no char arithmetic enters this.
    #[test]
    fn multibyte_text_does_not_shift_the_line() {
        let text = "héllo — ok\nsecond\n";
        assert_eq!(line_of(text, 0), 1);
        assert_eq!(line_of(text, text.find("second").unwrap()), 2);
    }

    /// The wiring, not just the helper: `line_of` being correct proves nothing
    /// if the region never carries its result. Removing the two assignments
    /// left the unit tests above green, which is exactly the gap this closes.
    #[test]
    fn the_emitted_region_carries_the_line() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("case.feature");
        std::fs::write(&source, "Feature: F\n  Scenario: S\n    When broken\n").unwrap();

        let text = std::fs::read_to_string(&source).unwrap();
        let offset = text.find("When broken").unwrap();
        let diag = Diag::error("proef::test::x", "boom")
            .with_source(
                source.to_string_lossy().into_owned(),
                std::sync::Arc::from(text.as_str()),
            )
            .with_span(proef_core::diag::Span {
                start: offset,
                end: offset + 5,
            });

        let out = dir.path().join("out.sarif");
        write(&[&diag], &out).unwrap();
        let log: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        let region = &log["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region["startLine"], 3, "{region}");
    }
}
