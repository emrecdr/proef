//! TAP (Test Anything Protocol) v13 output — one test point per scenario,
//! derived from the run's own outcomes (`RunSummary`), never from hurl.
//!
//! `proef test --output tap` prints this to stdout for `prove`/`tappy` while the
//! human report moves to stderr. A scenario that runs but does not gate the exit
//! code (`@quarantine`) maps to the TAP `# TODO` directive when it fails; a
//! `Skipped` scenario to `# SKIP`. Failure detail rides in a YAML diagnostic
//! block, redacted like every other sink.

use std::fmt::Write as _;

use proef_core::report::Redactions;
use proef_core::runner::{Fault, ScenarioOutcome};
use proef_core::step::Status;

/// Render scenario `outcomes` as a TAP v13 stream. `non_gating` holds the
/// `(file, name)` identities whose failure does not gate the run (quarantine),
/// mapped to `# TODO`; `redactions` masks any secret quoted in a failure detail.
pub fn render(
    outcomes: &[ScenarioOutcome],
    non_gating: &[(String, String)],
    redactions: &Redactions,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "TAP version 13");
    let _ = writeln!(out, "1..{}", outcomes.len());
    for (index, outcome) in outcomes.iter().enumerate() {
        let point = index + 1;
        let failed = outcome.status == Status::Failed;
        let quarantined = non_gating.iter().any(|(file, name)| {
            file.as_str() == outcome.file.as_ref() && name.as_str() == outcome.name.as_ref()
        });
        let verb = if failed { "not ok" } else { "ok" };
        let directive = if outcome.status == Status::Skipped {
            " # SKIP not run"
        } else if failed && quarantined {
            " # TODO quarantined"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "{verb} {point} - {}{directive}",
            describe(&outcome.name)
        );
        if failed && let Some(detail) = failure_detail(outcome) {
            // A YAML literal block (`|`) carries the message with no escaping.
            let _ = writeln!(out, "  ---");
            let _ = writeln!(out, "  message: |");
            for line in redactions.apply(&detail).lines() {
                let _ = writeln!(out, "    {line}");
            }
            let _ = writeln!(out, "  ...");
        }
    }
    out
}

/// A one-line TAP description: an unescaped `#` starts a directive and newlines
/// break the line, so both are neutralized — a scenario dedup suffix like
/// `name #2` would otherwise be read as a directive.
fn describe(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        match ch {
            '#' => out.push_str("\\#"),
            '\n' | '\r' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

/// The failure message for a failed scenario: a `User`/`System` fault wins;
/// otherwise the first failed step's detail (an assertion message).
fn failure_detail(outcome: &ScenarioOutcome) -> Option<String> {
    if let Some(fault) = &outcome.fault {
        return Some(match fault {
            Fault::User(message) | Fault::System(message) => message.clone(),
        });
    }
    outcome
        .steps
        .iter()
        .find(|step| step.status == Status::Failed)
        .and_then(|step| step.detail.clone())
}

#[cfg(test)]
mod tests {
    // Why: unwrap/expect are acceptable in `#[cfg(test)]` — a broken assumption
    // surfaces as a test failure, which is exactly the intent.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::Arc;

    use proef_core::step::{StepOutcome, StepRef};

    use super::*;

    fn outcome(file: &str, name: &str, status: Status, detail: Option<&str>) -> ScenarioOutcome {
        let steps = detail
            .map(|d| {
                vec![StepOutcome {
                    step: StepRef {
                        file: Arc::from(file),
                        line: 3,
                        text: Arc::from("a step"),
                    },
                    status: Status::Failed,
                    attempts: 1,
                    duration: std::time::Duration::from_millis(1),
                    detail: Some(d.to_owned()),
                    attempt_details: Vec::new(),
                    reproduce_hint: None,
                    fragment: None,
                }]
            })
            .unwrap_or_default();
        ScenarioOutcome {
            file: Arc::from(file),
            name: Arc::from(name),
            line: 1,
            status,
            steps,
            fault: None,
            artifact_slug: None,
        }
    }

    #[test]
    fn renders_plan_and_one_point_per_scenario() {
        let outcomes = [
            outcome("a.feature", "first passes", Status::Passed, None),
            outcome(
                "b.feature",
                "second fails",
                Status::Failed,
                Some("assert failure: expected 401 but was 200"),
            ),
            outcome("c.feature", "third skipped", Status::Skipped, None),
        ];
        let tap = render(&outcomes, &[], &Redactions::new(std::iter::empty()));
        assert!(tap.starts_with("TAP version 13\n1..3\n"), "{tap}");
        assert!(tap.contains("ok 1 - first passes\n"), "{tap}");
        assert!(tap.contains("not ok 2 - second fails\n"), "{tap}");
        assert!(
            tap.contains("ok 3 - third skipped # SKIP not run\n"),
            "{tap}"
        );
        // The failure detail rides in a YAML block.
        assert!(tap.contains("  message: |\n"), "{tap}");
        assert!(
            tap.contains("    assert failure: expected 401 but was 200\n"),
            "{tap}"
        );
    }

    #[test]
    fn quarantined_failure_is_a_todo() {
        let outcomes = [outcome("q.feature", "flaky", Status::Failed, Some("boom"))];
        let non_gating = vec![("q.feature".to_owned(), "flaky".to_owned())];
        let tap = render(&outcomes, &non_gating, &Redactions::new(std::iter::empty()));
        assert!(
            tap.contains("not ok 1 - flaky # TODO quarantined\n"),
            "{tap}"
        );
    }

    #[test]
    fn hash_in_name_is_escaped_so_it_is_not_a_directive() {
        let outcomes = [outcome("d.feature", "Same name #2", Status::Passed, None)];
        let tap = render(&outcomes, &[], &Redactions::new(std::iter::empty()));
        assert!(tap.contains("ok 1 - Same name \\#2\n"), "{tap}");
    }

    #[test]
    fn secret_in_failure_detail_is_redacted() {
        let outcomes = [outcome(
            "e.feature",
            "leaky",
            Status::Failed,
            Some("token=hunter2"),
        )];
        let tap = render(
            &outcomes,
            &[],
            &Redactions::new(std::iter::once("hunter2".to_owned())),
        );
        assert!(!tap.contains("hunter2"), "secret must not reach TAP: {tap}");
    }
}
