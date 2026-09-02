//! CTRF export (`--ctrf`): the run's outcomes as a Common Test Report Format
//! JSON file, beside `JUnit` and off the same fold.
//!
//! CTRF (<https://ctrf.io>) is the emerging JSON successor to `JUnit` XML for
//! CI dashboards — Microsoft's testing platform ships it first-party — and it
//! models in the *schema* what `JUnit` can only smuggle through extensions:
//! retries, flakiness, tags, a file path per test. proef already tracks all of
//! that, so this is a serializer, not a feature.
//!
//! **The verdicts must agree across sinks.** Every mapping here mirrors the
//! `JUnit` one (`ci_reports::test_case`) case for case — most visibly for a
//! quarantined failure, which ADR-0019 reports as *skipped with a message*
//! because it does not gate the exit code, and a dashboard reading "failed"
//! from one file and exit 0 from the process would contradict itself. A
//! `User`/`System` fault stays `failed` (it gates), with the fault text as the
//! message.
//!
//! The schema requires wall-clock `start`/`stop` (epoch milliseconds) on the
//! summary. Those are measured at the CLI edge like every other clock read
//! (ADR-0015); the sans-IO core and the run record are untouched. The file is
//! a CI report, not a second record (ADR-0008) — the JSONL event stream
//! remains the only record format.

use std::fmt::Write as _;
use std::path::Path;
use std::time::SystemTime;

use proef_core::report::{Redactions, step_label};
use proef_core::runner::{Fault, RunSummary, ScenarioOutcome};
use proef_core::step::Status;

use crate::render::via;

/// The CTRF spec version this file declares. The spec states this value
/// corresponds directly to the `specVersion` field.
const SPEC_VERSION: &str = "0.0.0";

/// Write the CTRF report for a finished run.
///
/// Same inputs as `write_junit`, plus the run's start instant: the two sinks
/// must describe one truth, so they are fed identically — including the
/// carried outcomes of a `--rerun` (the report covers the whole suite) and a
/// failed teardown's own scenarios.
#[allow(clippy::too_many_arguments)] // deliberately the write_junit list + the clock
pub fn write(
    summary: &RunSummary,
    teardown: Option<&RunSummary>,
    carried: &[ScenarioOutcome],
    non_gating: &[(String, String)],
    run_id: &str,
    started: SystemTime,
    path: &Path,
    redactions: &Redactions,
) -> Result<(), String> {
    let outcomes = || {
        summary
            .outcomes
            .iter()
            .chain(carried.iter())
            .chain(teardown.into_iter().flat_map(|t| t.outcomes.iter()))
    };

    let mut tests: Vec<serde_json::Value> = Vec::new();
    let (mut passed, mut failed, mut skipped) = (0u64, 0u64, 0u64);
    for outcome in outcomes() {
        let quarantined = non_gating.iter().any(|(file, name)| {
            file.as_str() == outcome.file.as_ref() && name.as_str() == outcome.name.as_ref()
        });
        let test = test_value(outcome, quarantined, redactions);
        match test["status"].as_str() {
            Some("passed") => passed += 1,
            Some("failed") => failed += 1,
            _ => skipped += 1,
        }
        tests.push(test);
    }

    let epoch_ms = |at: SystemTime| {
        at.duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            })
    };
    let body = serde_json::json!({
        "reportFormat": "CTRF",
        "specVersion": SPEC_VERSION,
        "results": {
            "tool": {
                "name": "proef",
                "version": env!("CARGO_PKG_VERSION"),
                "extra": { "runId": run_id },
            },
            "summary": {
                "tests": tests.len(),
                "passed": passed,
                "failed": failed,
                "skipped": skipped,
                "pending": 0,
                "other": 0,
                "start": epoch_ms(started),
                "stop": epoch_ms(SystemTime::now()),
            },
            "tests": tests,
        },
    });

    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("cannot create directory for {}: {err}", path.display()))?;
    }
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&body)
            .map_err(|err| format!("cannot serialize CTRF report: {err}"))?
    );
    std::fs::write(path, text)
        .map_err(|err| format!("cannot write CTRF report {}: {err}", path.display()))
}

/// A flaky pass's prior attempts, as CTRF `retryAttempts` entries. Empty for
/// a clean pass (the caller then writes neither `flaky` nor `retries`).
///
/// Built from the steps' real `attempt_details` when the engine carried them
/// — each entry is one failed earlier attempt with its redacted message
/// (engine-redacted already; re-applied for defense in depth). An engine that
/// counts retries without messages still gets honest entries: the prior
/// attempts happened and failed, that being what a retry *is*. Either way
/// `retries` and `retryAttempts` agree by construction — the spec wants one
/// entry per re-execution.
fn retry_attempts(outcome: &ScenarioOutcome, redactions: &Redactions) -> Vec<serde_json::Value> {
    let details: Vec<&String> = outcome
        .steps
        .iter()
        .flat_map(|step| &step.attempt_details)
        .collect();
    if details.is_empty() {
        let attempts = outcome.steps.iter().map(|s| s.attempts).max().unwrap_or(1);
        (1..attempts)
            .map(|attempt| serde_json::json!({ "attempt": attempt, "status": "failed" }))
            .collect()
    } else {
        details
            .iter()
            .enumerate()
            .map(|(index, message)| {
                serde_json::json!({
                    "attempt": index + 1,
                    "status": "failed",
                    "message": redactions.apply(message),
                })
            })
            .collect()
    }
}

/// One scenario as a CTRF test object — the `JUnit` `test_case` mapping,
/// case for case, in the other format's vocabulary.
fn test_value(
    outcome: &ScenarioOutcome,
    quarantined: bool,
    redactions: &Redactions,
) -> serde_json::Value {
    let mut test = serde_json::json!({
        "name": outcome.name.as_ref(),
        "status": "other",
        "duration": u64::try_from(outcome.cost().as_millis()).unwrap_or(u64::MAX),
        "suite": [outcome.file.as_ref()],
        "filePath": outcome.file.as_ref(),
    });
    if !outcome.tags.is_empty() {
        test["tags"] = serde_json::json!(outcome.tags.as_ref());
    }

    match (outcome.status, &outcome.fault) {
        (Status::Passed | Status::Warned, _) => {
            test["status"] = "passed".into();
            let entries = retry_attempts(outcome, redactions);
            if !entries.is_empty() {
                test["flaky"] = true.into();
                test["retries"] = entries.len().into();
                test["retryAttempts"] = entries.into();
            }
        }
        (Status::Skipped, _) => {
            test["status"] = "skipped".into();
            let reason = outcome
                .reason
                .as_deref()
                .map(ToOwned::to_owned)
                .or_else(|| {
                    outcome
                        .steps
                        .iter()
                        .find(|s| s.status == Status::Skipped)
                        .and_then(|s| s.detail.clone())
                });
            if let Some(reason) = reason {
                test["message"] = redactions.apply(&reason).into();
            }
        }
        // ADR-0019: a quarantined test-failure does not gate the exit code,
        // and no sink may say otherwise — `JUnit` reports it skipped with a
        // message, so this file does too. Faults stay failures: quarantine is
        // for flaky tests, not broken input.
        (Status::Failed, None) if quarantined => {
            test["status"] = "skipped".into();
            let detail = outcome
                .steps
                .iter()
                .filter(|s| s.status == Status::Failed)
                .filter_map(|s| s.detail.as_deref())
                .collect::<Vec<_>>()
                .join("; ");
            test["message"] = redactions
                .apply(&format!("quarantined failure (non-gating): {detail}"))
                .into();
        }
        (Status::Failed, fault) => {
            test["status"] = "failed".into();
            let message = match fault {
                Some(Fault::System(message) | Fault::User(message)) => message.clone(),
                None => outcome
                    .steps
                    .iter()
                    .filter(|s| s.status == Status::Failed)
                    .filter_map(|s| {
                        s.detail.as_deref().map(|d| {
                            format!(
                                "{d}{}{}",
                                step_label(s.label.as_deref()),
                                via(s.fragment.as_deref())
                            )
                        })
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            };
            test["message"] = redactions.apply(&message).into();
            // The trace channel has room the one-line message does not; as in
            // `JUnit`'s text node, it carries the reproduce hints — the
            // artifact a reader actually wants from a CI results page.
            let mut trace = String::new();
            for step in outcome.steps.iter().filter(|s| s.status == Status::Failed) {
                if let Some(hint) = &step.reproduce_hint {
                    let _ = writeln!(trace, "reproduce: {}", redactions.apply(hint));
                }
            }
            if !trace.is_empty() {
                test["trace"] = trace.trim_end().into();
            }
        }
    }
    test
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;
    use std::time::SystemTime;

    use proef_core::report::Redactions;
    use proef_core::runner::{Fault, RunSummary, ScenarioOutcome};
    use proef_core::step::{Status, StepOutcome, StepRef};

    fn outcome(file: &str, name: &str, status: Status, detail: Option<&str>) -> ScenarioOutcome {
        ScenarioOutcome {
            file: file.into(),
            name: name.into(),
            line: 2,
            status,
            reason: None,
            tags: Arc::from(vec!["smoke".to_owned()]),
            steps: vec![StepOutcome {
                step: StepRef {
                    file: Arc::from(file),
                    line: 3,
                    text: Arc::from("the operator acts"),
                },
                status,
                attempts: 1,
                duration: std::time::Duration::from_millis(1234),
                detail: detail.map(ToOwned::to_owned),
                attempt_details: Vec::new(),
                reproduce_hint: (status == Status::Failed)
                    .then(|| "curl -X GET http://x".to_owned()),
                fragment: None,
                label: None,
            }],
            fault: None,
            artifact_slug: None,
        }
    }

    fn report(
        outcomes: Vec<ScenarioOutcome>,
        non_gating: &[(String, String)],
    ) -> serde_json::Value {
        let summary = RunSummary {
            passed: outcomes.len(),
            failed: 0,
            skipped: 0,
            cancelled: false,
            outcomes,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/ctrf.json");
        super::write(
            &summary,
            None,
            &[],
            non_gating,
            "run-1",
            SystemTime::now(),
            &path,
            &Redactions::default(),
        )
        .unwrap();
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
    }

    /// The envelope the spec requires, and the summary counting what the
    /// tests themselves say. The path is nested on purpose: output paths
    /// create the directories they name (the 0.11.1 contract).
    #[test]
    fn the_envelope_and_summary_match_the_spec() {
        let body = report(
            vec![
                outcome("a.feature", "green", Status::Passed, None),
                outcome("a.feature", "red", Status::Failed, Some("status 500")),
                outcome("b.feature", "held", Status::Skipped, None),
            ],
            &[],
        );
        assert_eq!(body["reportFormat"], "CTRF");
        assert_eq!(body["specVersion"], super::SPEC_VERSION);
        let summary = &body["results"]["summary"];
        assert_eq!(summary["tests"], 3);
        assert_eq!(summary["passed"], 1);
        assert_eq!(summary["failed"], 1);
        assert_eq!(summary["skipped"], 1);
        assert!(
            summary["start"].as_u64().unwrap() <= summary["stop"].as_u64().unwrap(),
            "epoch start precedes stop"
        );
        let tests = body["results"]["tests"].as_array().unwrap();
        assert_eq!(tests.len(), 3);
        assert_eq!(tests[0]["duration"], 1234, "cost is the step-duration sum");
        assert_eq!(tests[0]["filePath"], "a.feature");
        assert_eq!(tests[0]["suite"][0], "a.feature");
        assert_eq!(tests[0]["tags"][0], "smoke");
    }

    /// The failure channel split `JUnit` uses, in CTRF vocabulary: the
    /// one-line detail is the `message`, the reproduce hint rides `trace`.
    #[test]
    fn a_failure_carries_message_and_reproduce_trace() {
        let body = report(
            vec![outcome(
                "a.feature",
                "red",
                Status::Failed,
                Some("status 500"),
            )],
            &[],
        );
        let test = &body["results"]["tests"][0];
        assert_eq!(test["status"], "failed");
        assert_eq!(test["message"], "status 500");
        assert_eq!(test["trace"], "reproduce: curl -X GET http://x");
    }

    /// ADR-0019 across sinks: a quarantined failure does not gate the exit
    /// code, so no sink may count it failed — `JUnit` says skipped with a
    /// message, and this file must agree with both the XML and the exit code.
    #[test]
    fn a_quarantined_failure_is_skipped_with_a_message_like_junit() {
        let body = report(
            vec![outcome("a.feature", "flaky", Status::Failed, Some("boom"))],
            &[("a.feature".to_owned(), "flaky".to_owned())],
        );
        let test = &body["results"]["tests"][0];
        assert_eq!(test["status"], "skipped");
        assert_eq!(test["message"], "quarantined failure (non-gating): boom");
        assert_eq!(body["results"]["summary"]["failed"], 0);
        assert_eq!(body["results"]["summary"]["skipped"], 1);
    }

    /// A fault is not quarantinable (broken input, not a flaky test): it
    /// gates the run, so it stays `failed` even when the identity matches a
    /// quarantine tag — same carve-out as the `JUnit` mapping.
    #[test]
    fn a_fault_stays_failed_even_under_quarantine() {
        let mut faulted = outcome("a.feature", "flaky", Status::Failed, None);
        faulted.fault = Some(Fault::User("missing secret apiToken".to_owned()));
        let body = report(
            vec![faulted],
            &[("a.feature".to_owned(), "flaky".to_owned())],
        );
        let test = &body["results"]["tests"][0];
        assert_eq!(test["status"], "failed");
        assert_eq!(test["message"], "missing secret apiToken");
    }

    /// Flaky pass: the schema-native half of why this format exists. The
    /// prior failed attempts become `retryAttempts` with their real messages,
    /// and `retries` agrees with the entry count by construction.
    #[test]
    fn a_flaky_pass_reports_retries_flaky_and_the_real_attempts() {
        let mut flaky = outcome("a.feature", "eventually", Status::Passed, None);
        flaky.steps[0].attempts = 3;
        flaky.steps[0].attempt_details = vec![
            "attempt 1: status 503".to_owned(),
            "attempt 2: timeout".to_owned(),
        ];
        let body = report(vec![flaky], &[]);
        let test = &body["results"]["tests"][0];
        assert_eq!(test["status"], "passed");
        assert_eq!(test["flaky"], true);
        assert_eq!(test["retries"], 2);
        let attempts = test["retryAttempts"].as_array().unwrap();
        assert_eq!(attempts.len(), 2, "one entry per re-execution, per spec");
        assert_eq!(attempts[0]["attempt"], 1);
        assert_eq!(attempts[0]["status"], "failed");
        assert_eq!(attempts[0]["message"], "attempt 1: status 503");
        assert_eq!(attempts[1]["attempt"], 2);
    }

    /// A clean pass writes none of the flaky fields — absence is the signal
    /// dashboards read, so an all-clean suite must not be a wall of
    /// `flaky: false`.
    #[test]
    fn a_clean_pass_has_no_flaky_fields() {
        let body = report(
            vec![outcome("a.feature", "green", Status::Passed, None)],
            &[],
        );
        let test = &body["results"]["tests"][0];
        for absent in ["flaky", "retries", "retryAttempts", "message", "trace"] {
            assert!(test.get(absent).is_none(), "{absent} must be absent");
        }
    }
}
