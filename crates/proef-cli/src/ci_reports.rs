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
    // Honest flaky reporting: a scenario that passed only after retries records
    // the attempt count instead of looking identical to a clean pass.
    let attempts = outcome.steps.iter().map(|s| s.attempts).max().unwrap_or(1);
    if attempts > 1 && matches!(outcome.status, Status::Passed | Status::Warned) {
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
                let attempts = if step.attempts > 1 {
                    format!(" _(after {} attempts)_", step.attempts)
                } else {
                    String::new()
                };
                let _ = writeln!(
                    failures,
                    "- `{}:{}`{attempts} — {detail}",
                    step.step.file, step.step.line
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
        if !matches!(outcome.status, Status::Passed | Status::Warned) {
            continue;
        }
        let attempts = outcome.steps.iter().map(|s| s.attempts).max().unwrap_or(1);
        if attempts > 1 {
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
                    step.step.file,
                    step.step.line,
                    enc_prop(&format!("{}: {}", outcome.name, step.step.text)),
                    enc_msg(&redactions.apply(detail)),
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
                outcome.file,
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
