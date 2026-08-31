//! `proef report [run-id]` — write a self-contained HTML report for a run from
//! its event record. A *derived* view (ADR-0008), replayed like `explain`: the
//! events read back are already redacted at the sink, so the page is too.

use std::path::Path;

use proef_core::error::ExitCode;
use proef_core::event::Event;

use crate::record::RunCompletion;

/// Render the named run (or the latest) to a standalone HTML file. Defaults to
/// `report.html` inside the run dir so the `artifacts/` deep-links resolve.
pub fn report(
    runs_root: &Path,
    run_id: Option<&str>,
    output: Option<&Path>,
    tag_links: &std::collections::BTreeMap<String, String>,
) -> ExitCode {
    let Some(record_dir) = crate::record::resolve_dir(runs_root, run_id) else {
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

    // A rerun record names its base (`rerun_of`, ADR-0008-additive): overlay
    // the base's not-re-run suite scenarios so the page shows the whole
    // suite, not the re-run subset (E2's rerun half). Composition over
    // records — the base's events are filtered in, timestamps stripped so
    // two runs' time axes never mix in the timeline; the record files are
    // untouched. A rotated-away base degrades to today's subset view, said
    // out loud.
    let mut events = events;
    let mut carried_note: Option<usize> = None;
    if let Some(base_id) = &rec.rerun_of {
        // `rerun_of` is a string read out of a record file, and records
        // travel — joined unvalidated, a crafted `"../../elsewhere"` steered
        // this read outside the runs root and spliced a foreign file's events
        // into the rendered page. A run id is a single path component,
        // whatever its spelling (uuid or a user-chosen `--run-id`).
        let escapes_runs_root = std::path::Path::new(base_id).file_name()
            != Some(std::ffi::OsStr::new(base_id.as_str()));
        if escapes_runs_root {
            crate::render::errln!(
                "note: base run id `{base_id}` in the record is not a directory name — \
                 rendering the re-run subset only"
            );
        } else {
            match crate::record::read_events(&runs_root.join(base_id)) {
                Ok(base_events) => {
                    let carried = carried_scenario_events(&base_events, &rec);
                    if !carried.is_empty() {
                        carried_note = Some(count_carried(&carried));
                        events.splice(1..1, carried);
                    }
                }
                Err(_) => {
                    crate::render::errln!(
                        "note: base run {base_id} is no longer on disk — rendering the re-run subset only"
                    );
                }
            }
        }
    }
    let out_path = output.map_or_else(|| record_dir.join("report.html"), Path::to_path_buf);
    let href = artifacts_href(&record_dir, &out_path);
    let mut html = proef_core::html::render_html(&events, &href, tag_links);
    if let Some(count) = carried_note {
        html = banner_carried(&html, count, rec.rerun_of.as_deref().unwrap_or("?"));
    }
    if rec.completion == RunCompletion::Incomplete {
        html = banner_incomplete(&html);
    }
    if let Err(err) = crate::fsutil::create_parents(&out_path) {
        crate::render::errln!(
            "error: cannot create directory for {}: {err}",
            out_path.display()
        );
        return ExitCode::SystemError;
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
/// sits in the run dir (the common case), else a path *relative to the report*
/// so the links resolve from wherever `-o` put the file.
///
/// This used to be absolute, for a real reason: a browser resolves a relative
/// href against the HTML file's own directory, and `runs-dir` defaults to a
/// relative `.proef-runs`, so a bare `artifacts` 404s from anywhere else. But
/// an absolute filesystem path is the wrong repair. It resolves only on the
/// machine that produced it — and `-o` exists to put the report somewhere it
/// will be *published*, which is precisely where that path is dead. Worse, it
/// names the author's home directory in the one output built to be shared:
/// 0.13.0 scrubbed machine identity out of the run record (R12-1), and this
/// put it straight back, twelve times over, in the HTML uploaded beside it.
///
/// A relative path strictly dominates. It resolves everywhere the absolute one
/// did, *plus* wherever report and artifacts travel together with their
/// structure intact — and in the case that matters (`-o public/report.html`
/// inside the project, the CI shape) it names nothing outside the workspace.
///
/// Residual, stated rather than papered over: a report written somewhere that
/// shares no ancestor with the run dir still names the directories between
/// them, because that is what a correct relative path from there *is*. It is
/// no worse than the absolute path it replaces, and that report is not one
/// being shared.
///
/// Purely lexical (like the `std::path::absolute` it replaces): no filesystem
/// access, so it stays usable after the run dir has been rotated away.
fn artifacts_href(record_dir: &Path, out_path: &Path) -> String {
    let out_dir = crate::fsutil::parent_dir(out_path);
    if out_dir == record_dir {
        return "artifacts".to_owned();
    }
    let artifacts = record_dir.join("artifacts");
    relative_from(&out_dir, &artifacts).map_or_else(
        // No relative path exists (different Windows prefixes — a report on
        // `D:` for a run dir on `C:`). Absolute is the only thing that can
        // resolve at all, so it stays the fallback rather than a dead link.
        || {
            std::path::absolute(&artifacts)
                .unwrap_or(artifacts)
                .display()
                .to_string()
        },
        |relative| relative.display().to_string(),
    )
}

/// `to`, expressed relative to the directory `from`; `None` when the two share
/// no root (distinct Windows path prefixes).
///
/// Lexical: `..` components are popped rather than resolved, which is wrong
/// under symlinks and is the same trade `std::path::absolute` already makes
/// here.
fn relative_from(from: &Path, to: &Path) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    let (from_root, from_parts) = split(from)?;
    let (to_root, to_parts) = split(to)?;
    if from_root != to_root {
        return None;
    }
    let shared = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut out = PathBuf::new();
    for _ in shared..from_parts.len() {
        out.push("..");
    }
    for part in &to_parts[shared..] {
        out.push(part);
    }
    // Same directory: a browser needs *something* to join the filename onto.
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    Some(out)
}

/// A lexically-absolute path split into its root (prefix + `/`) and its normal
/// components, with `.` dropped and `..` popped.
fn split(path: &Path) -> Option<(std::ffi::OsString, Vec<std::ffi::OsString>)> {
    use std::path::Component;
    let absolute = std::path::absolute(path).ok()?;
    let mut root = std::ffi::OsString::new();
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => root.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(part) => parts.push(part.to_owned()),
        }
    }
    Some((root, parts))
}

/// The base run's scenario-scoped events for suite identities the rerun did
/// NOT re-run: `scenario_started`/`scenario_finished`/`step_finished` only,
/// with `timestamp_ms`/`worker` stripped — carried scenarios appear in the
/// blocks and tag table but never on the timeline, whose axis belongs to the
/// rerun.
fn carried_scenario_events(base_events: &[Event], rerun: &crate::record::Record) -> Vec<Event> {
    use std::sync::Arc;
    let re_ran = |file: &str, scenario: &str| {
        rerun
            .scenarios
            .contains_key(&(file.to_owned(), scenario.to_owned()))
    };
    let mut carried = Vec::new();
    for event in base_events {
        match event {
            Event::ScenarioStarted {
                scenario,
                file,
                phase: None,
                exclusive,
                ..
            } if !re_ran(file, scenario) => carried.push(Event::ScenarioStarted {
                scenario: Arc::clone(scenario),
                file: Arc::clone(file),
                timestamp_ms: None,
                worker: None,
                phase: None,
                exclusive: *exclusive,
            }),
            Event::ScenarioFinished {
                scenario,
                file,
                status,
                phase: None,
                reason,
                tags,
                ..
            } if !re_ran(file, scenario) => carried.push(Event::ScenarioFinished {
                scenario: Arc::clone(scenario),
                file: Arc::clone(file),
                status: *status,
                timestamp_ms: None,
                worker: None,
                phase: None,
                reason: reason.clone(),
                tags: tags.clone(),
            }),
            Event::StepFinished { scenario, step, .. } if !re_ran(&step.file, scenario) => {
                carried.push(event.clone());
            }
            _ => {}
        }
    }
    carried
}

/// How many scenarios the carried slice holds (one `scenario_finished` each).
fn count_carried(carried: &[Event]) -> usize {
    carried
        .iter()
        .filter(|event| matches!(event, Event::ScenarioFinished { .. }))
        .count()
}

/// The merged-view banner, prepended like the incompleteness one.
fn banner_carried(html: &str, count: usize, base: &str) -> String {
    let note = format!(
        "<p class=\"summary\">merged view: {count} scenario(s) carried from run <code>{}</code></p>",
        base.replace('<', "&lt;")
    );
    match html.find("<h1") {
        Some(i) => {
            let after = html[i..].find("</h1>").map_or(i, |j| i + j + 5);
            format!("{}{}{}", &html[..after], note, &html[after..])
        }
        None => format!("{note}{html}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CI shape — `-o` inside the project, artifacts under `.proef-runs`.
    /// The href must resolve *and* name nothing outside the workspace: this is
    /// the report that gets uploaded, and an absolute path here put the
    /// author's home directory into it (undoing R12-1 for the one artifact
    /// built to be shared).
    #[test]
    fn an_out_of_run_dir_report_links_artifacts_relatively() {
        let record_dir = Path::new(".proef-runs/01ABC");
        let out_path = Path::new("public/report.html");
        let href = artifacts_href(record_dir, out_path);
        assert_eq!(href, "../.proef-runs/01ABC/artifacts");
        assert!(
            !Path::new(&href).is_absolute(),
            "the href must not be absolute: {href}"
        );

        // And it resolves: joined onto the report's own directory (what a
        // browser does), it lands on the real artifacts directory. Compared
        // through `split`, which pops `..` — `std::path::absolute` deliberately
        // does not, so the two spellings would differ as strings while naming
        // the same directory.
        let resolved = crate::fsutil::parent_dir(out_path).join(&href);
        assert_eq!(split(&resolved), split(&record_dir.join("artifacts")));
    }

    /// The leak, pinned directly: whatever the href is, it must not carry an
    /// absolute path rooted at the machine's home directory.
    #[test]
    fn the_artifacts_href_never_names_the_authors_home_directory() {
        let record_dir = Path::new(".proef-runs/01ABC");
        for out in ["public/report.html", "report.html", "out/nested/r.html"] {
            let href = artifacts_href(record_dir, Path::new(out));
            assert!(
                !Path::new(&href).is_absolute(),
                "-o {out} produced an absolute href: {href}"
            );
        }
    }

    #[test]
    fn relative_from_walks_up_and_back_down() {
        assert_eq!(
            relative_from(Path::new("/a/b/c"), Path::new("/a/b/c/d")),
            Some(std::path::PathBuf::from("d"))
        );
        assert_eq!(
            relative_from(Path::new("/a/b/c"), Path::new("/a/x/y")),
            Some(std::path::PathBuf::from("../../x/y"))
        );
        // Same directory still needs a joinable prefix.
        assert_eq!(
            relative_from(Path::new("/a/b"), Path::new("/a/b")),
            Some(std::path::PathBuf::from("."))
        );
        // `..` is popped lexically, so an unnormalized input does not produce
        // a path that walks up too far.
        assert_eq!(
            relative_from(Path::new("/a/b/../b/c"), Path::new("/a/b/c/d")),
            Some(std::path::PathBuf::from("d"))
        );
    }

    #[test]
    fn a_report_in_the_run_dir_keeps_the_bare_href() {
        let record_dir = Path::new("/runs/01ABC");
        let out_path = Path::new("/runs/01ABC/report.html");
        assert_eq!(artifacts_href(record_dir, out_path), "artifacts");
    }
}
