//! CI-facing reports (M4, US-8): `JUnit` XML via `quick-junit` and the GitHub
//! Actions job summary. Both derive from the run outcome — never a second
//! source of truth (the JSONL event stream remains the record, ADR-0008).

use std::fmt::Write as _;
use std::path::Path;

use crate::render::via;
use proef_core::report::Redactions;
use proef_core::runner::{Fault, RunSummary, ScenarioOutcome};
use proef_core::step::Status;
use quick_junit::{NonSuccessKind, Report, TestCase, TestCaseStatus, TestRerun, TestSuite};

/// Write `report.junit.xml` for the run: one suite per feature file, one test
/// case per scenario, engine-measured times, failure details inline.
pub fn write_junit(
    summary: &RunSummary,
    run_id: &str,
    path: &Path,
    redactions: &Redactions,
) -> Result<(), String> {
    let mut report = Report::new("proef");
    report.set_uuid(uuid::Uuid::parse_str(run_id).unwrap_or_else(|_| uuid::Uuid::nil()));

    let mut files: Vec<&str> = summary.outcomes.iter().map(|o| o.file.as_ref()).collect();
    files.sort_unstable();
    files.dedup();

    let mut total = std::time::Duration::ZERO;
    for file in files {
        let mut suite = TestSuite::new(file);
        let mut suite_time = std::time::Duration::ZERO;
        for outcome in summary.outcomes.iter().filter(|o| o.file.as_ref() == file) {
            suite_time += outcome.steps.iter().map(|s| s.duration).sum::<std::time::Duration>();
            suite.add_test_case(test_case(outcome, redactions));
        }
        // GitLab reads `time` on both `testsuite` and `testsuites` (it ignores
        // the count attributes and `timestamp`); Jenkins reads suite `time`
        // for duration. `timestamp` and `hostname` are deliberately absent:
        // GitLab ignores both, Jenkins substitutes its own build clock and
        // never reads `hostname` — and naming the machine would undo R12-1.
        suite.set_time(suite_time);
        total += suite_time;
        report.add_test_suite(suite);
    }
    report.set_time(total);

    crate::fsutil::create_parents(path)
        .map_err(|err| format!("cannot create directory for {}: {err}", path.display()))?;
    let file = std::fs::File::create(path)
        .map_err(|err| format!("cannot create {}: {err}", path.display()))?;
    report
        .serialize(file)
        .map_err(|err| format!("cannot serialize JUnit report: {err}"))
}

/// The attempt count a scenario finally passed on, if it went green only after
/// retries — the single home for the "flaky pass?" query (`JUnit` + job summary).
fn flaky_pass_attempts(outcome: &ScenarioOutcome) -> Option<u32> {
    if !matches!(outcome.status, Status::Passed | Status::Warned) {
        return None;
    }
    let attempts = outcome.steps.iter().map(|s| s.attempts).max().unwrap_or(1);
    (attempts > 1).then_some(attempts)
}

fn test_case(outcome: &ScenarioOutcome, redactions: &Redactions) -> TestCase {
    let status = match (outcome.status, &outcome.fault) {
        (Status::Passed | Status::Warned, _) => {
            let mut status = TestCaseStatus::success();
            // Flaky pass: each earlier failed attempt of a step that
            // ultimately passed becomes a `<flakyFailure>` (quick-junit
            // serializes reruns on a success as flakyFailure). Messages arrive
            // engine-redacted; re-apply for defense in depth.
            for message in outcome.steps.iter().flat_map(|step| &step.attempt_details) {
                let mut rerun = TestRerun::new(NonSuccessKind::Failure);
                rerun.set_message(redactions.apply(message));
                status.add_rerun(rerun);
            }
            status
        }
        (Status::Skipped, _) => TestCaseStatus::skipped(),
        (Status::Failed, fault) => {
            let (kind, message) = match fault {
                Some(Fault::System(message) | Fault::User(message)) => {
                    (NonSuccessKind::Error, message.clone())
                }
                None => (
                    NonSuccessKind::Failure,
                    outcome
                        .steps
                        .iter()
                        .filter(|s| s.status == Status::Failed)
                        .filter_map(|s| {
                            s.detail
                                .as_deref()
                                .map(|d| format!("{d}{}", via(s.fragment.as_deref())))
                        })
                        .collect::<Vec<_>>()
                        .join("; "),
                ),
            };
            let mut status = TestCaseStatus::non_success(kind);
            status.set_message(redactions.apply(&message));
            status
        }
    };
    // Identity is `classname` + `name` in both consumers that matter: Jenkins
    // keys test history on the pair, and GitLab's merge-request widget diffs
    // head against base by it. The old single `name` embedded `file:line`, so
    // any edit above a scenario re-identified every test below it and both
    // tools saw a fleet of "new" tests. `classname` carries the file (GitLab
    // displays it as the suite column; Jenkins groups by it), `name` carries
    // the scenario alone — unique per file by construction, since outline
    // instances are already `#N`-disambiguated. The line number is not
    // identity: it lives in the failure detail and the artifact reference.
    let mut case = TestCase::new(outcome.name.as_ref(), status);
    case.set_classname(outcome.file.as_ref());
    // GitLab reads a `file` attribute on the testcase for source linking;
    // quick-junit does not model it, so it rides the extra-attribute map.
    case.extra
        .insert("file".into(), outcome.file.as_ref().into());
    case.set_time(outcome.steps.iter().map(|s| s.duration).sum());
    // Honest flaky reporting: a scenario that passed only after retries records
    // the attempt count instead of looking identical to a clean pass.
    if let Some(attempts) = flaky_pass_attempts(outcome) {
        case.set_system_out(format!("passed on attempt {attempts}"));
    }
    case
}

/// Append the run summary to `$GITHUB_STEP_SUMMARY` when running in Actions
/// (US-8/G7). Failures list their feature anchor and detail.
pub fn write_github_summary(summary: &RunSummary, run_id: &str, redactions: &Redactions) {
    let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") else {
        return;
    };
    let body = summary_body(summary, run_id);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
    {
        use std::io::Write as _;
        // One pass over the final body covers names and details alike.
        let _ = writeln!(file, "{}", redactions.apply(&body));
    }
}

/// The summary's markdown, separated from the file append so it can be asserted
/// without an env var — `std::env::set_var` is `unsafe` in edition 2024, and a
/// sink that can only be tested by mutating process state tends not to be.
fn summary_body(summary: &RunSummary, run_id: &str) -> String {
    let mut body = format!(
        "## proef run `{run_id}`\n\n**{} passed · {} failed · {} skipped**\n\n| scenario | status | steps |\n|---|---|---|\n",
        summary.passed, summary.failed, summary.skipped
    );
    for outcome in &summary.outcomes {
        let _ = writeln!(
            body,
            "| `{}:{}` {} | {:?} | {} |",
            enc_cell(&outcome.file),
            outcome.line,
            enc_cell(&outcome.name),
            outcome.status,
            outcome.steps.len()
        );
    }
    let mut failures = String::new();
    for outcome in &summary.outcomes {
        for step in outcome.steps.iter().filter(|s| s.status == Status::Failed) {
            if let Some(detail) = &step.detail {
                let attempts = if step.attempts > 1 {
                    format!(" _(after {} attempts)_", step.attempts)
                } else {
                    String::new()
                };
                let _ = writeln!(
                    failures,
                    "- `{}:{}`{attempts} — {detail}{}",
                    step.step.file,
                    step.step.line,
                    via(step.fragment.as_deref())
                );
            }
        }
    }
    if !failures.is_empty() {
        body.push_str("\n### failures\n\n");
        body.push_str(&failures);
    }

    // Honest flaky reporting: scenarios that passed only after retries, so a
    // green-on-attempt-N run is visible rather than silently masked.
    let mut flaky = String::new();
    for outcome in &summary.outcomes {
        if let Some(attempts) = flaky_pass_attempts(outcome) {
            let _ = writeln!(
                flaky,
                "- `{}:{}` {} — passed on attempt {attempts}",
                outcome.file, outcome.line, outcome.name
            );
        }
    }
    if !flaky.is_empty() {
        body.push_str("\n### flaky passes\n\n");
        body.push_str(&flaky);
    }
    body
}

/// Emit GitHub Actions `::error` annotations to stdout for failures, so each one
/// renders in the PR "Files changed" gutter. Distinct sink from the job summary
/// (a file append): these are stdout workflow commands, so the caller invokes
/// this only under Actions AND when the human report — not `--output json` —
/// owns stdout.
pub fn github_annotations(summary: &RunSummary, redactions: &Redactions) -> String {
    let mut out = String::new();
    for outcome in &summary.outcomes {
        let mut anchored_a_step = false;
        for step in outcome.steps.iter().filter(|s| s.status == Status::Failed) {
            if let Some(detail) = &step.detail {
                anchored_a_step = true;
                let _ = writeln!(
                    out,
                    "::error file={},line={},title={}::{}",
                    enc_prop(&step.step.file),
                    step.step.line,
                    enc_prop(&format!("{}: {}", outcome.name, step.step.text)),
                    enc_msg(
                        &redactions.apply(&format!("{detail}{}", via(step.fragment.as_deref())))
                    ),
                );
            }
        }
        // Scenario-level faults (user/system) have no failing step to anchor —
        // annotate the scenario header line instead.
        if !anchored_a_step
            && let Some(Fault::System(message) | Fault::User(message)) = &outcome.fault
        {
            let _ = writeln!(
                out,
                "::error file={},line={},title={}::{}",
                enc_prop(&outcome.file),
                outcome.line,
                enc_prop(&outcome.name),
                enc_msg(&redactions.apply(message)),
            );
        }
    }
    out
}

/// Percent-encode a workflow-command message body (GitHub's rules: `%`, CR, LF).
fn enc_msg(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Property values (`title`) additionally escape `:` and `,`.
/// A value going into a Markdown table cell.
///
/// `|` ends a cell, so an unescaped one in a scenario name or a path silently
/// splits the row and shifts every column after it. A newline ends the row
/// outright. Neither is exotic: a scenario name is free prose.
fn enc_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\r', '\n'], " ")
}

fn enc_prop(s: &str) -> String {
    enc_msg(s).replace(':', "%3A").replace(',', "%2C")
}

#[cfg(test)]
mod tests {
    use super::{enc_msg, enc_prop};

    #[test]
    fn enc_escapes_workflow_command_metacharacters() {
        // Message bodies escape %, CR, LF — and % first, so nothing double-encodes.
        assert_eq!(enc_msg("a%b\r\nc"), "a%25b%0D%0Ac");
        // Property values additionally escape `:` and `,` so a title cannot break
        // the `key=value,key=value` parse.
        assert_eq!(enc_prop("Scenario: a, b"), "Scenario%3A a%2C b");
    }
}

#[cfg(test)]
mod provenance_tests {
    #![allow(clippy::unwrap_used)]

    use super::{github_annotations, summary_body};
    use proef_core::report::Redactions;
    use proef_core::runner::{RunSummary, ScenarioOutcome};
    use proef_core::step::{Status, StepOutcome, StepRef};
    use std::sync::Arc;

    fn failed_run(fragment: Option<&str>) -> RunSummary {
        RunSummary {
            outcomes: vec![ScenarioOutcome {
                file: "tests/features/a.feature".into(),
                name: "S".into(),
                line: 2,
                status: Status::Failed,
                steps: vec![StepOutcome {
                    step: StepRef {
                        file: Arc::from("tests/features/a.feature"),
                        line: 3,
                        text: Arc::from("the operator searches"),
                    },
                    status: Status::Failed,
                    attempts: 1,
                    duration: std::time::Duration::from_millis(1),
                    detail: Some("Assert status code".to_owned()),
                    attempt_details: Vec::new(),
                    reproduce_hint: None,
                    fragment: fragment.map(ToOwned::to_owned),
                }],
                fault: None,
                artifact_slug: None,
            }],
            passed: 0,
            failed: 1,
            skipped: 0,
            cancelled: false,
        }
    }

    /// CI is where a reader is least able to go looking, so every CI-facing
    /// sink names the fragment a failure came from (ADR-0018) — and an inline
    /// step, which has none, adds nothing rather than an empty `via`.
    #[test]
    fn every_ci_sink_names_the_fragment_a_failure_came_from() {
        let none = Redactions::new(std::iter::empty());

        let summary = failed_run(Some("tests/hurl/admin.hurl#admin.search"));
        let annotations = github_annotations(&summary, &none);
        // In the *message* body, not a property: `enc_msg` encodes only `%`,
        // CR and LF, so the `#` of `file.hurl#name` survives verbatim and the
        // line stays copy-pasteable back into a pack's `ref:`.
        assert!(
            annotations.contains("(via tests/hurl/admin.hurl#admin.search)"),
            "the annotation must carry it: {annotations}"
        );
        let summary_md = summary_body(&summary, "run-1");
        assert!(
            summary_md.contains("(via tests/hurl/admin.hurl#admin.search)"),
            "the job summary must carry it: {summary_md}"
        );

        let inline = failed_run(None);
        assert!(
            !github_annotations(&inline, &none).contains("via "),
            "an inline step has no fragment and must not render an empty one"
        );
        assert!(
            !summary_body(&inline, "run-1").contains("via "),
            "an inline step has no fragment and must not render an empty one"
        );
    }
}

#[cfg(test)]
mod escaping_tests {
    #![allow(clippy::unwrap_used)]

    use super::{enc_cell, github_annotations};
    use proef_core::report::Redactions;
    use proef_core::runner::{Fault, RunSummary, ScenarioOutcome};
    use proef_core::step::Status;

    /// `file=` is a workflow-command *property*, parsed out of a
    /// `key=value,key=value` list terminated by `::`. It was the one argument
    /// in its own `writeln!` passed raw while `title=` and the message beside
    /// it were encoded — so a path carrying `,` or `:` (every Windows path
    /// carries a `:`) broke the annotation into nonsense.
    #[test]
    fn the_annotation_encodes_its_file_property() {
        let summary = RunSummary {
            outcomes: vec![ScenarioOutcome {
                file: "C:\\proj,odd\\x.feature".into(),
                name: "s".into(),
                line: 3,
                status: Status::Failed,
                steps: Vec::new(),
                fault: Some(Fault::System("boom".to_owned())),
                artifact_slug: None,
            }],
            passed: 0,
            failed: 1,
            skipped: 0,
            cancelled: false,
        };
        let out = github_annotations(&summary, &Redactions::new(std::iter::empty()));
        assert!(
            out.contains("file=C%3A\\proj%2Codd\\x.feature"),
            "the file property must be encoded like every other one:\n{out}"
        );
    }

    /// A scenario name is free prose and a path is arbitrary, so both can carry
    /// `|` — which ends a Markdown cell and shifts every column after it. The
    /// row still renders, just wrongly, which is why nobody notices.
    #[test]
    fn table_cells_escape_the_pipe_that_would_split_the_row() {
        assert_eq!(enc_cell("a | b"), r"a \| b");
        assert_eq!(enc_cell("plain"), "plain");
        // A newline ends the row outright.
        assert_eq!(enc_cell("two\nlines"), "two lines");
    }
}
