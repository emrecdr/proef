//! `proef diff [base] [new]` — compare two run records. The JSONL event stream
//! IS the record (ADR-0008); diff replays two of them and reports scenario
//! status transitions (regressions, fixes) plus per-step flakiness and perf
//! deltas. Identity is `(file, scenario)` — the run-wide identity ADR-0008
//! added `file` for — and steps diff on `(text, ordinal)`, never the volatile
//! `line`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use proef_core::error::ExitCode;
use proef_core::step::Status;

use crate::record::{self, ScenarioRun};

/// A perf regression is only reported when a scenario is both meaningfully
/// (≥50%) and absolutely (≥50ms) slower — single-run timing is noisy, so a
/// conservative gate keeps the signal honest.
const SLOWER_MIN_DELTA_MS: u64 = 50;
const SLOWER_MIN_RATIO_NUM: u64 = 3; // new ≥ base * 3/2
const SLOWER_MIN_RATIO_DEN: u64 = 2;

/// Compare two run records. `base`/`new` are run ids under `runs_dir`; omitted,
/// they default to the previous and latest runs. With `fail_on_regression`, a
/// detected regression exits `1` (a test that now fails) for CI gating.
pub fn diff(
    runs_dir: &str,
    base: Option<&str>,
    new: Option<&str>,
    fail_on_regression: bool,
) -> ExitCode {
    let runs_root = PathBuf::from(runs_dir);
    let (base_dir, new_dir) = match resolve_pair(&runs_root, base, new) {
        Ok(pair) => pair,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::UserError;
        }
    };
    let base_rec = match record::read_record(&base_dir) {
        Ok(rec) => rec,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::UserError;
        }
    };
    let new_rec = match record::read_record(&new_dir) {
        Ok(rec) => rec,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::UserError;
        }
    };

    incomplete_banner("base", &base_dir, base_rec.completion);
    incomplete_banner("new", &new_dir, new_rec.completion);

    let report = Report::compute(&base_rec.scenarios, &new_rec.scenarios);
    report.render(&base_dir, &new_dir);

    if fail_on_regression {
        // An incomplete/cancelled NEW run cannot certify "no regressions".
        if new_rec.completion != record::RunCompletion::Completed {
            eprintln!(
                "error: the new run did not complete ({}) — cannot certify no regressions",
                completion_word(new_rec.completion)
            );
            return ExitCode::TestFailure;
        }
        if !report.regressed.is_empty() {
            return ExitCode::TestFailure;
        }
    }
    ExitCode::Success
}

fn completion_word(c: record::RunCompletion) -> &'static str {
    match c {
        record::RunCompletion::Completed => "completed",
        record::RunCompletion::Cancelled => "cancelled",
        record::RunCompletion::Incomplete => "incomplete — no RunFinished",
    }
}

/// Warn (always, even without --fail-on-regression) when a diffed record did
/// not complete, so a human is never misled by a partial run.
fn incomplete_banner(which: &str, dir: &Path, c: record::RunCompletion) {
    if c != record::RunCompletion::Completed {
        crate::render::outln!(
            "⚠ {which} run {} is {} — results may be partial",
            run_name(dir),
            completion_word(c)
        );
    }
}

/// Resolve the two records to compare. One positional → base vs latest; none →
/// previous vs latest.
fn resolve_pair(
    root: &Path,
    base: Option<&str>,
    new: Option<&str>,
) -> Result<(PathBuf, PathBuf), String> {
    let missing = || format!("no run records under {}", root.display());
    match (base, new) {
        (Some(base), Some(new)) => Ok((root.join(base), root.join(new))),
        (Some(base), None) => Ok((
            root.join(base),
            record::latest_run(root).ok_or_else(missing)?,
        )),
        (None, _) => {
            let runs = record::all_runs(root);
            if runs.len() < 2 {
                return Err(format!(
                    "need at least two runs to diff; found {} under {}",
                    runs.len(),
                    root.display()
                ));
            }
            let latest = runs[runs.len() - 1].clone();
            let prev = runs[runs.len() - 2].clone();
            Ok((prev, latest))
        }
    }
}

/// A `(file, scenario)` key rendered as `file :: scenario`.
type Key = (String, String);

fn label(key: &Key) -> String {
    format!("{} :: {}", key.0, key.1)
}

/// The computed diff, bucketed for rendering.
struct Report {
    regressed: Vec<Key>,     // was not-failing, now failing
    fixed: Vec<Key>,         // was failing, now passing
    still_failing: Vec<Key>, // failing in both
    added: Vec<(Key, Status)>,
    removed: Vec<Key>,
    flaky: Vec<String>,  // scenarios whose retries rose (rendered lines)
    slower: Vec<String>, // scenarios that got meaningfully slower
}

impl Report {
    fn compute(base: &BTreeMap<Key, ScenarioRun>, new: &BTreeMap<Key, ScenarioRun>) -> Self {
        let mut report = Report {
            regressed: Vec::new(),
            fixed: Vec::new(),
            still_failing: Vec::new(),
            added: Vec::new(),
            removed: Vec::new(),
            flaky: Vec::new(),
            slower: Vec::new(),
        };
        for (key, new_run) in new {
            match base.get(key) {
                None => report.added.push((key.clone(), new_run.status)),
                Some(base_run) => {
                    let was_fail = base_run.status == Status::Failed;
                    let now_fail = new_run.status == Status::Failed;
                    match (was_fail, now_fail) {
                        (false, true) => report.regressed.push(key.clone()),
                        (true, false) => report.fixed.push(key.clone()),
                        (true, true) => report.still_failing.push(key.clone()),
                        (false, false) => {}
                    }
                    report.note_flaky(key, base_run, new_run);
                    report.note_slower(key, base_run, new_run);
                }
            }
        }
        for key in base.keys() {
            if !new.contains_key(key) {
                report.removed.push(key.clone());
            }
        }
        report
    }

    /// A step whose attempt count rose between runs is a flakiness signal (the
    /// engine had to retry more). Diffs steps by `(text, ordinal)`, so line
    /// shifts don't lie.
    fn note_flaky(&mut self, key: &Key, base: &ScenarioRun, new: &ScenarioRun) {
        for ((text, ord), new_step) in &new.steps {
            let base_attempts = base
                .steps
                .get(&(text.clone(), *ord))
                .map_or(1, |step| step.attempts);
            if new_step.attempts > base_attempts {
                self.flaky.push(format!(
                    "    ⚠ {} — step \"{text}\" {base_attempts}→{} attempt(s)",
                    label(key),
                    new_step.attempts
                ));
            }
        }
    }

    /// Sum durations over steps present (by `(text, ordinal)`) in both runs; flag a scenario
    /// only when it is both proportionally and absolutely slower.
    fn note_slower(&mut self, key: &Key, base: &ScenarioRun, new: &ScenarioRun) {
        let (mut base_ms, mut new_ms) = (0u64, 0u64);
        for ((text, ord), new_step) in &new.steps {
            if let Some(base_step) = base.steps.get(&(text.clone(), *ord)) {
                base_ms = base_ms.saturating_add(base_step.duration_ms);
                new_ms = new_ms.saturating_add(new_step.duration_ms);
            }
        }
        let delta = new_ms.saturating_sub(base_ms);
        if delta >= SLOWER_MIN_DELTA_MS
            && new_ms.saturating_mul(SLOWER_MIN_RATIO_DEN)
                >= base_ms.saturating_mul(SLOWER_MIN_RATIO_NUM)
        {
            self.slower.push(format!(
                "    ⏱ {}  {base_ms}ms → {new_ms}ms (+{delta}ms)",
                label(key)
            ));
        }
    }

    fn render(&self, base_dir: &Path, new_dir: &Path) {
        crate::render::outln!("diff {} → {}", run_name(base_dir), run_name(new_dir));
        section("regressed", "passed → failed", &self.regressed);
        section("fixed", "failed → passed", &self.fixed);
        section("still failing", "", &self.still_failing);
        if !self.added.is_empty() {
            crate::render::outln!("\n  new ({}):", self.added.len());
            for (key, status) in &self.added {
                crate::render::outln!("    + {}  [{}]", label(key), status_word(*status));
            }
        }
        section("removed", "", &self.removed);
        lines("flaky (retries rose)", &self.flaky);
        lines("slower", &self.slower);
        crate::render::outln!(
            "\nsummary: {} regressed · {} fixed · {} new · {} removed",
            self.regressed.len(),
            self.fixed.len(),
            self.added.len(),
            self.removed.len()
        );
    }
}

/// Print a `(file, scenario)` bucket with a count header, omitting it when empty.
fn section(title: &str, transition: &str, keys: &[Key]) {
    if keys.is_empty() {
        return;
    }
    let suffix = if transition.is_empty() {
        String::new()
    } else {
        format!("   {transition}")
    };
    crate::render::outln!("\n  {title} ({}):{suffix}", keys.len());
    for key in keys {
        crate::render::outln!("    {}", label(key));
    }
}

/// Print a pre-rendered bucket of lines with a count header, omitting it when empty.
fn lines(title: &str, entries: &[String]) {
    if entries.is_empty() {
        return;
    }
    crate::render::outln!("\n  {title} ({}):", entries.len());
    for entry in entries {
        crate::render::outln!("{entry}");
    }
}

/// The run id (dir basename) for display, falling back to the full path.
fn run_name(dir: &Path) -> String {
    dir.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| dir.display().to_string(), str::to_owned)
}

fn status_word(status: Status) -> &'static str {
    match status {
        Status::Passed => "passed",
        Status::Failed => "failed",
        Status::Skipped => "skipped",
        Status::Warned => "warned",
    }
}
