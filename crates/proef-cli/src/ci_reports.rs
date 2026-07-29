//! CI-facing reports (M4, US-8): `JUnit` XML via `quick-junit` and the GitHub
//! Actions job summary. Both derive from the run outcome — never a second
//! source of truth (the JSONL event stream remains the record, ADR-0008).

use std::fmt::Write as _;
use std::path::Path;

use proef_core::report::Redactions;
use proef_core::runner::{Fault, RunSummary, ScenarioOutcome};
use proef_core::step::Status;
use quick_junit::{NonSuccessKind, Report, TestCase, TestCaseStatus, TestSuite};

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

    for file in files {
        let mut suite = TestSuite::new(file);
        for outcome in summary.outcomes.iter().filter(|o| o.file.as_ref() == file) {
            suite.add_test_case(test_case(outcome, redactions));
        }
        report.add_test_suite(suite);
    }

    let file = std::fs::File::create(path)
        .map_err(|err| format!("cannot create {}: {err}", path.display()))?;
    report
        .serialize(file)
        .map_err(|err| format!("cannot serialize JUnit report: {err}"))
}

fn test_case(outcome: &ScenarioOutcome, redactions: &Redactions) -> TestCase {
    let status = match (outcome.status, &outcome.fault) {
        (Status::Passed | Status::Warned, _) => TestCaseStatus::success(),
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
                        .filter_map(|s| s.detail.clone())
                        .collect::<Vec<_>>()
                        .join("; "),
                ),
            };
            let mut status = TestCaseStatus::non_success(kind);
            status.set_message(redactions.apply(&message));
            status
        }
    };
    let mut case = TestCase::new(
        format!("{}:{} {}", outcome.file, outcome.line, outcome.name),
        status,
    );
    case.set_time(outcome.steps.iter().map(|s| s.duration).sum());
    case
}

/// Append the run summary to `$GITHUB_STEP_SUMMARY` when running in Actions
/// (US-8/G7). Failures list their feature anchor and detail.
pub fn write_github_summary(summary: &RunSummary, run_id: &str, redactions: &Redactions) {
    let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") else {
        return;
    };
    let mut body = format!(
        "## proef run `{run_id}`\n\n**{} passed · {} failed · {} skipped**\n\n| scenario | status | steps |\n|---|---|---|\n",
        summary.passed, summary.failed, summary.skipped
    );
    for outcome in &summary.outcomes {
        let _ = writeln!(
            body,
            "| `{}:{}` {} | {:?} | {} |",
            outcome.file,
            outcome.line,
            outcome.name,
            outcome.status,
            outcome.steps.len()
        );
    }
    let mut failures = String::new();
    for outcome in &summary.outcomes {
        for step in outcome.steps.iter().filter(|s| s.status == Status::Failed) {
            if let Some(detail) = &step.detail {
                let _ = writeln!(
                    failures,
                    "- `{}:{}` — {detail}",
                    step.step.file, step.step.line
                );
            }
        }
    }
    if !failures.is_empty() {
        body.push_str("\n### failures\n\n");
        body.push_str(&failures);
    }
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
