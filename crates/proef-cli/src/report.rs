//! `proef report [run-id]` — write a self-contained HTML report for a run from
//! its event record. A *derived* view (ADR-0008), replayed like `explain`: the
//! events read back are already redacted at the sink, so the page is too.

use std::path::{Path, PathBuf};

use proef_core::error::ExitCode;
use proef_core::event::Event;

/// Render the named run (or the latest) to a standalone HTML file. Defaults to
/// `report.html` inside the run dir so the `artifacts/` deep-links resolve.
pub fn report(runs_dir: &str, run_id: Option<&str>, output: Option<&Path>) -> ExitCode {
    let runs_root = PathBuf::from(runs_dir);
    let Some(record_dir) = crate::record::resolve_dir(&runs_root, run_id) else {
        eprintln!("error: no run records under {}", runs_root.display());
        return ExitCode::UserError;
    };
    let events_path = record_dir.join("events.jsonl");
    let text = match std::fs::read_to_string(&events_path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("error: cannot read {}: {err}", events_path.display());
            return ExitCode::UserError;
        }
    };
    let events: Vec<Event> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();

    let out_path = output.map_or_else(|| record_dir.join("report.html"), Path::to_path_buf);
    let href = artifacts_href(&record_dir, &out_path);
    let html = proef_core::html::render_html(&events, &href);
    if let Err(err) = std::fs::write(&out_path, html) {
        eprintln!("error: cannot write {}: {err}", out_path.display());
        return ExitCode::SystemError;
    }
    crate::render::outln!("wrote {}", out_path.display());
    ExitCode::Success
}

/// The href prefix for artifact deep-links: a bare `artifacts` when the report
/// sits in the run dir (the common case), else the run dir's `artifacts` path so
/// the links still resolve from wherever `-o` put the file.
fn artifacts_href(record_dir: &Path, out_path: &Path) -> String {
    let out_dir = out_path.parent().unwrap_or(Path::new(""));
    if out_dir == record_dir {
        "artifacts".to_owned()
    } else {
        record_dir.join("artifacts").display().to_string()
    }
}
