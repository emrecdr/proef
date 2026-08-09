//! `proef report [run-id]` — write a self-contained HTML report for a run from
//! its event record. A *derived* view (ADR-0008), replayed like `explain`: the
//! events read back are already redacted at the sink, so the page is too.

use std::path::{Path, PathBuf};

use proef_core::error::ExitCode;
use proef_core::event::Event;

use crate::record::RunCompletion;

/// Render the named run (or the latest) to a standalone HTML file. Defaults to
/// `report.html` inside the run dir so the `artifacts/` deep-links resolve.
pub fn report(runs_dir: &str, run_id: Option<&str>, output: Option<&Path>) -> ExitCode {
    let runs_root = PathBuf::from(runs_dir);
    let Some(record_dir) = crate::record::resolve_dir(&runs_root, run_id) else {
        crate::render::errln!("error: no run records under {}", runs_root.display());
        return ExitCode::UserError;
    };
    let events_path = record_dir.join("events.jsonl");
    let text = match std::fs::read_to_string(&events_path) {
        Ok(text) => text,
        Err(err) => {
            crate::render::errln!("error: cannot read {}: {err}", events_path.display());
            return ExitCode::UserError;
        }
    };
    let events: Vec<Event> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    // Read once, parse once: `render_html` needs the raw events (steps,
    // timing, detail — pruned out of `Record`) and completion needs
    // `parse_record`'s fold over the same events, not a second
    // `read_to_string`/parse pass — a live run's tail `RunFinished` landing
    // between two reads would otherwise let one disagree with the other.
    let rec = crate::record::parse_record(&events);

    let out_path = output.map_or_else(|| record_dir.join("report.html"), Path::to_path_buf);
    let href = artifacts_href(&record_dir, &out_path);
    let mut html = proef_core::html::render_html(&events, &href);
    if rec.completion == RunCompletion::Incomplete {
        html = banner_incomplete(&html);
    }
    if let Err(err) = std::fs::write(&out_path, html) {
        crate::render::errln!("error: cannot write {}: {err}", out_path.display());
        return ExitCode::SystemError;
    }
    crate::render::outln!("wrote {}", out_path.display());
    ExitCode::Success
}

/// `render_html` has no banner parameter — `proef-core` stays untouched — so
/// prepend the incompleteness notice to the page's existing heading, the same
/// wording `explain` prints for the same condition.
fn banner_incomplete(html: &str) -> String {
    html.replacen(
        "<h1>",
        "<p class=\"incomplete-banner\">⚠ run incomplete — no run_finished; results are partial</p>\n<h1>",
        1,
    )
}

/// The href prefix for artifact deep-links: a bare `artifacts` when the report
/// sits in the run dir (the common case), else an absolute run-dir `artifacts`
/// path so the links still resolve from wherever `-o` put the file.
fn artifacts_href(record_dir: &Path, out_path: &Path) -> String {
    let out_dir = crate::fsutil::parent_dir(out_path);
    if out_dir == record_dir {
        "artifacts".to_owned()
    } else {
        // A browser resolves a relative href against the HTML file's own
        // directory, and `runs-dir` defaults to a relative `.proef-runs`,
        // so the link has to be absolute to survive `-o` pointing anywhere
        // outside the run dir. `std::path::absolute` is purely lexical (no
        // filesystem access), so it stays usable even after the run dir has
        // been rotated away.
        std::path::absolute(record_dir.join("artifacts"))
            .unwrap_or_else(|_| record_dir.join("artifacts"))
            .display()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_out_of_run_dir_report_gets_an_absolute_artifacts_href() {
        // `runs-dir` defaults to a relative `.proef-runs`, and a browser
        // resolves a relative href against the HTML file's own directory —
        // so a report written elsewhere needs an absolute path or every
        // artifact link 404s while the command reports success.
        let record_dir = Path::new(".proef-runs/01ABC");
        let out_path = Path::new("/tmp/elsewhere/report.html");
        let href = artifacts_href(record_dir, out_path);
        assert!(
            Path::new(&href).is_absolute(),
            "href must not be relative to the output file: {href}"
        );
        assert!(
            href.ends_with("artifacts"),
            "href must point at the artifacts dir: {href}"
        );
    }

    #[test]
    fn a_report_in_the_run_dir_keeps_the_bare_href() {
        let record_dir = Path::new("/runs/01ABC");
        let out_path = Path::new("/runs/01ABC/report.html");
        assert_eq!(artifacts_href(record_dir, out_path), "artifacts");
    }
}
