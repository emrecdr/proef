//! `proef report`: the HTML report is a *derived* view over the event
//! stream (ADR-0008), pure and deterministic in the events, so its canonical
//! form is snapshot-locked here (change it only via `cargo insta review`).
//! HTML-escaping of authored text is pinned by including `<`/`&`/`"` in the
//! fixture events.

#![allow(clippy::unwrap_used, clippy::expect_used)]
// The `step` fixture builder mirrors `Event::StepFinished`'s field list one-to-one.
#![allow(clippy::too_many_arguments)]

use std::sync::Arc;

use proef_core::event::{EVENT_SCHEMA_VERSION, Event};
use proef_core::html::render_html;
use proef_core::step::{Status, StepRef};

fn step(
    file: &str,
    line: usize,
    text: &str,
    scenario: &str,
    status: Status,
    attempts: u32,
    duration_ms: u64,
    detail: Option<&str>,
) -> Event {
    Event::StepFinished {
        scenario: Arc::from(scenario),
        engine: Arc::from("http"),
        step: StepRef {
            file: Arc::from(file),
            line,
            text: Arc::from(text),
        },
        status,
        attempts,
        duration_ms,
        captures: Vec::new(),
        detail: detail.map(str::to_owned),
        attempt_details: Vec::new(),
    }
}

fn finished(scenario: &str, file: &str, status: Status) -> Event {
    Event::ScenarioFinished {
        scenario: Arc::from(scenario),
        file: Arc::from(file),
        status,
        timestamp_ms: None,
        worker: None,
    }
}

#[test]
fn html_report_is_snapshot_locked() {
    let events = vec![
        Event::RunStarted {
            schema: EVENT_SCHEMA_VERSION,
            run_id: Arc::from("report-0001"),
        },
        step(
            "tests/features/501_search.feature",
            6,
            "the admin searches for \"Jansen\"",
            "Search <admin> & guest",
            Status::Passed,
            1,
            40,
            None,
        ),
        finished(
            "Search <admin> & guest",
            "tests/features/501_search.feature",
            Status::Passed,
        ),
        step(
            "tests/features/502_login.feature",
            4,
            "the guest logs in",
            "Login rejects a bad password",
            Status::Failed,
            2,
            120,
            Some(
                "assert failure: expected HTTP 401 but was 200\n  at 502-login--login-rejects-a-bad-password.hurl:9",
            ),
        ),
        finished(
            "Login rejects a bad password",
            "tests/features/502_login.feature",
            Status::Failed,
        ),
        Event::RunFinished {
            passed: 1,
            failed: 1,
            skipped: 0,
            cancelled: false,
        },
    ];

    insta::assert_snapshot!("html_report", render_html(&events, "artifacts"));
}

/// When the record carries injected timing (ADR-0015), the report adds a
/// cross-worker timeline — a lane per worker with a bar per scenario on a shared
/// run-relative axis. Absent timing (the snapshot test above), it is omitted.
#[test]
fn timeline_renders_from_injected_timing() {
    let started = |scenario: &str, file: &str, ts: u64, worker: u64| Event::ScenarioStarted {
        scenario: Arc::from(scenario),
        file: Arc::from(file),
        timestamp_ms: Some(ts),
        worker: Some(worker),
    };
    let done = |scenario: &str, file: &str, status: Status, ts: u64, worker: u64| {
        Event::ScenarioFinished {
            scenario: Arc::from(scenario),
            file: Arc::from(file),
            status,
            timestamp_ms: Some(ts),
            worker: Some(worker),
        }
    };
    let events = vec![
        Event::RunStarted {
            schema: EVENT_SCHEMA_VERSION,
            run_id: Arc::from("t"),
        },
        started("A", "a.feature", 0, 0),
        started("B", "b.feature", 5, 1),
        done("A", "a.feature", Status::Passed, 40, 0),
        done("B", "b.feature", Status::Failed, 60, 1),
        Event::RunFinished {
            passed: 1,
            failed: 1,
            skipped: 0,
            cancelled: false,
        },
    ];
    let html = render_html(&events, "artifacts");
    assert!(html.contains("class=\"timeline\""), "{html}");
    assert!(
        html.contains("worker 0") && html.contains("worker 1"),
        "{html}"
    );
    assert!(
        html.contains("class=\"tbar pass\"") && html.contains("class=\"tbar fail\""),
        "{html}"
    );
    assert!(html.contains("60ms"), "run length is the max end: {html}");
}
