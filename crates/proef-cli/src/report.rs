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
    // Through `record::read_events` — the same reader the base record below
    // already uses, and the only one carrying the record-size ceiling. It
    // returns the parsed events, which is exactly the read-once/parse-once
    // this needs.
    let events: Vec<Event> = match crate::record::read_events(&record_dir) {
        Ok(events) => events,
        Err(err) => {
            crate::render::errln!("error: {err}");
            return ExitCode::UserError;
        }
    };
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
    // Followed **transitively**: fix → rerun → fix → rerun is the workflow
    // this feature exists for, and reading only the immediate base dropped
    // everything from before it. A three-deep chain rendered two scenarios of
    // a thirteen-scenario suite, with no banner saying so, while `docs/CI.md`
    // promises "one report stands for the composed result".
    //
    // `seen` is not defensive dressing: `rerun_of` is a string read out of a
    // record file, and a record travels — a cycle (crafted, or a run id
    // reused) would otherwise splice for ever.
    let mut base_id = rec.rerun_of.clone();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut carried_total = 0usize;
    let mut oldest_base: Option<String> = None;
    while let Some(id) = base_id.take() {
        if !seen.insert(id.clone()) {
            crate::render::errln!(
                "note: base run id `{id}` repeats in the rerun chain — stopping there"
            );
            break;
        }
        // `rerun_of` is joined onto the runs root, so a crafted
        // `"../../elsewhere"` would steer this read outside it and splice a
        // foreign file's events into the page. A run id is a single path
        // component, whatever its spelling (uuid or a user-chosen `--run-id`).
        if std::path::Path::new(&id).file_name() != Some(std::ffi::OsStr::new(id.as_str())) {
            crate::render::errln!(
                "note: base run id `{id}` in the record is not a directory name — \
                 rendering without it"
            );
            break;
        }
        let Ok(base_events) = crate::record::read_events(&runs_root.join(&id)) else {
            crate::render::errln!(
                "note: base run {id} is no longer on disk — rendering without it"
            );
            break;
        };
        // Re-parsed against the *composed* stream so far, so a scenario
        // already carried from a newer base is not carried again from an
        // older one: the newest verdict wins, which is what a merged view
        // means.
        let composed = crate::record::parse_record(&events);
        let carried = carried_scenario_events(&base_events, &composed);
        if !carried.is_empty() {
            carried_total += count_carried(&carried);
            events.splice(1..1, carried);
        }
        oldest_base = Some(id);
        base_id = crate::record::parse_record(&base_events).rerun_of;
    }
    if carried_total > 0 {
        carried_note = Some(carried_total);
        // The tail totals belong to the **re-run**, and this stream is no
        // longer the re-run: the page below lists the composed suite. Left in
        // place they won the headline, so one page read `2 passed · 0 failed`
        // above a tag table summing to eight and a sibling `JUnit` saying
        // `tests="8"`. Dropping them makes the headline count the scenarios
        // actually rendered — no number is invented, one is declined.
        //
        // Safe because this vector is already a derived, in-memory
        // composition, not the record: the record file keeps its own tail
        // untouched (ADR-0008), and `rec.completion` — read before any of
        // this — still drives the incomplete banner.
        events.retain(|event| !matches!(event, Event::RunFinished { .. }));
    }
    let out_path = output.map_or_else(|| record_dir.join("report.html"), Path::to_path_buf);
    let href = artifacts_href(&record_dir, &out_path);
    let mut html = proef_core::html::render_html(&events, &href, tag_links);
    if let Some(count) = carried_note {
        html = banner_carried(&html, count, oldest_base.as_deref().unwrap_or("?"));
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
///
/// The result is a **URL path, not a display path**: components are joined
/// with `/` on every platform. `Path::display` renders `\` on Windows, and a
/// backslash is not a separator in a URL — a browser would not resolve
/// `..\.proef-runs\01ABC\artifacts` at all, so every link in a
/// Windows-generated report would be dead. Building from components rather
/// than replacing separators in the rendered string also keeps a Unix
/// directory whose *name* contains a backslash intact.
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
        // Separators are swapped rather than rebuilt from components because
        // this branch only fires on Windows, where `\` cannot occur inside a
        // file name and the swap is therefore lossless.
        || {
            std::path::absolute(&artifacts)
                .unwrap_or(artifacts)
                .display()
                .to_string()
                .replace('\\', "/")
        },
        |relative| relative.join("/"),
    )
}

/// `to`, expressed relative to the directory `from`, as URL path components;
/// `None` when the two share no root (distinct Windows path prefixes).
///
/// Components rather than a `PathBuf` so the caller joins them with `/`
/// itself — see [`artifacts_href`] on why a rendered path is the wrong thing
/// to put in an `href`.
///
/// Lexical: `..` components are popped rather than resolved, which is wrong
/// under symlinks and is the same trade `std::path::absolute` already makes
/// here.
fn relative_from(from: &Path, to: &Path) -> Option<Vec<String>> {
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
    let mut out: Vec<String> = Vec::new();
    for _ in shared..from_parts.len() {
        out.push("..".to_owned());
    }
    for part in &to_parts[shared..] {
        out.push(part.to_string_lossy().into_owned());
    }
    // Same directory: a browser needs *something* to join the filename onto.
    if out.is_empty() {
        out.push(".".to_owned());
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

        // An href is a URL path on every platform — never `Path::display`,
        // which renders `\` on Windows and would make every link dead.
        assert!(
            !href.contains('\\'),
            "href must not carry a backslash: {href}"
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
            assert!(
                !href.contains('\\'),
                "-o {out} produced a backslash href: {href}"
            );
        }
    }

    /// Written with relative inputs so the assertions hold on Windows too,
    /// where `/a/b` is not an absolute path and `absolute()` would anchor it
    /// to a drive.
    #[test]
    fn relative_from_walks_up_and_back_down() {
        let parts = |from: &str, to: &str| relative_from(Path::new(from), Path::new(to));
        assert_eq!(parts("a/b/c", "a/b/c/d"), Some(vec!["d".to_owned()]));
        assert_eq!(
            parts("a/b/c", "a/x/y"),
            Some(vec![
                "..".to_owned(),
                "..".to_owned(),
                "x".to_owned(),
                "y".to_owned()
            ])
        );
        // Same directory still needs a joinable prefix.
        assert_eq!(parts("a/b", "a/b"), Some(vec![".".to_owned()]));
        // `..` is popped lexically, so an unnormalized input does not walk up
        // too far.
        assert_eq!(parts("a/b/../b/c", "a/b/c/d"), Some(vec!["d".to_owned()]));
    }

    #[test]
    fn a_report_in_the_run_dir_keeps_the_bare_href() {
        let record_dir = Path::new("/runs/01ABC");
        let out_path = Path::new("/runs/01ABC/report.html");
        assert_eq!(artifacts_href(record_dir, out_path), "artifacts");
    }
}
