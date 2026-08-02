//! SARIF 2.1.0 export of validation diagnostics — the `--dry-run` shift-left
//! gate. Each `Diag` maps ~1:1 to a SARIF result (code → `ruleId`, byte `Span` →
//! `region.byteOffset`/`byteLength`, severity → `level`), so unbound steps and
//! pack-lint findings surface as inline annotations in a PR. A parallel
//! serializer to `render::print_all(&[Diag])`; dry-run diagnostics resolve no
//! secrets, so nothing here needs redaction.

use std::collections::BTreeSet;
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
    let results: Vec<serde_json::Value> = diags.iter().map(|d| result(d)).collect();
    let sarif = serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": { "name": "proef", "rules": rules } },
            "results": results,
        }],
    });
    let text = serde_json::to_string_pretty(&sarif).map_err(|err| err.to_string())?;
    std::fs::write(path, text).map_err(|err| format!("cannot write {}: {err}", path.display()))
}

fn result(diag: &Diag) -> serde_json::Value {
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
        result["locations"] = serde_json::json!([{
            "physicalLocation": {
                "artifactLocation": { "uri": name },
                "region": { "byteOffset": span.start, "byteLength": span.end - span.start },
            }
        }]);
    }
    result
}
