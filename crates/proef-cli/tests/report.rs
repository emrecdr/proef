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
        fragment: None,
        label: None,
        detail: detail.map(str::to_owned),
        attempt_details: Vec::new(),
        // Failing steps carry the redacted curl in the real stream — populate
        // it here so the snapshot pins the report's reproduce line.
        reproduce_hint: (status == Status::Failed)
            .then(|| format!("curl {}", text.replace(' ', "-"))),
    }
}

fn finished(scenario: &str, file: &str, status: Status) -> Event {
    Event::ScenarioFinished {
        scenario: Arc::from(scenario),
        file: Arc::from(file),
        status,
        timestamp_ms: None,
        worker: None,
        phase: None,
        reason: None,
        // The snapshot pins the by-tag table: derive a stable tag from
        // the file stem so each fixture scenario carries one.
        tags: vec![
            std::path::Path::new(file)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ],
    }
}

#[test]
fn html_report_is_snapshot_locked() {
    let events = vec![
        Event::RunStarted {
            schema: EVENT_SCHEMA_VERSION,
            run_id: Arc::from("report-0001"),
            env: None,
            metadata: std::collections::BTreeMap::new(),
            shuffled: false,
            rerun_of: None,
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

    insta::assert_snapshot!(
        "html_report",
        render_html(
            &events,
            "artifacts",
            &[(
                "501_*".to_owned(),
                "https://tracker.example/{tag}".to_owned()
            )]
            .into(),
        )
    );
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
        phase: None,
        exclusive: false,
    };
    let done = |scenario: &str, file: &str, status: Status, ts: u64, worker: u64| {
        Event::ScenarioFinished {
            scenario: Arc::from(scenario),
            file: Arc::from(file),
            status,
            timestamp_ms: Some(ts),
            worker: Some(worker),
            phase: None,
            reason: None,
            tags: Vec::new(),
        }
    };
    let events = vec![
        Event::RunStarted {
            schema: EVENT_SCHEMA_VERSION,
            run_id: Arc::from("t"),
            env: None,
            metadata: std::collections::BTreeMap::new(),
            shuffled: false,
            rerun_of: None,
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
    let html = render_html(&events, "artifacts", &std::collections::BTreeMap::new());
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

/// A `ref:` step's fragment reaches the HTML report, under the failure reason —
/// same order `explain` uses (ADR-0018). CI artifacts are read by people who
/// cannot grep the checkout, so this is the sink that matters most.
#[test]
fn the_html_report_names_the_fragment_a_step_ran() {
    let with_fragment = Event::StepFinished {
        scenario: Arc::from("S"),
        engine: Arc::from("http"),
        step: StepRef {
            file: Arc::from("tests/features/a.feature"),
            line: 3,
            text: Arc::from("the operator searches"),
        },
        status: Status::Failed,
        attempts: 1,
        duration_ms: 4,
        captures: Vec::new(),
        fragment: Some("tests/hurl/admin.hurl#admin.search".to_owned()),
        label: None,
        detail: Some("Assert status code".to_owned()),
        attempt_details: Vec::new(),
        reproduce_hint: None,
    };
    let events = vec![
        Event::RunStarted {
            schema: EVENT_SCHEMA_VERSION,
            run_id: Arc::from("run-1"),
            env: None,
            metadata: std::collections::BTreeMap::new(),
            shuffled: false,
            rerun_of: None,
        },
        with_fragment,
        finished("S", "tests/features/a.feature", Status::Failed),
    ];
    let html = render_html(&events, "artifacts", &std::collections::BTreeMap::new());
    assert!(
        html.contains("<p class=\"via\">via tests/hurl/admin.hurl#admin.search</p>"),
        "the fragment must render under the step: {html}"
    );

    // An inline step carries none, and must not render an empty marker.
    let inline = vec![
        Event::RunStarted {
            schema: EVENT_SCHEMA_VERSION,
            run_id: Arc::from("run-1"),
            env: None,
            metadata: std::collections::BTreeMap::new(),
            shuffled: false,
            rerun_of: None,
        },
        step(
            "tests/features/a.feature",
            3,
            "the operator searches",
            "S",
            Status::Failed,
            1,
            4,
            Some("Assert status code"),
        ),
        finished("S", "tests/features/a.feature", Status::Failed),
    ];
    assert!(
        !render_html(&inline, "artifacts", &std::collections::BTreeMap::new())
            .contains("class=\"via\""),
        "an inline step has no fragment to name"
    );
}

/// Every section a reader can see must be a section a reader can *navigate*:
/// the timeline already carried an `<h2>`, the tag table and the scenario list
/// were rendered with no heading at all, so the document had one heading and no
/// outline. Pinned structurally, so a section added without a heading fails
/// here rather than shipping.
#[test]
fn each_rendered_section_carries_a_heading() {
    // Timing (for the timeline) and a tag (for the rollup) at once — the only
    // fixture in this file that renders all three sections together.
    let events = vec![
        Event::RunStarted {
            schema: EVENT_SCHEMA_VERSION,
            run_id: Arc::from("h"),
            env: None,
            metadata: std::collections::BTreeMap::new(),
            shuffled: false,
            rerun_of: None,
        },
        Event::ScenarioStarted {
            scenario: Arc::from("A"),
            file: Arc::from("a.feature"),
            timestamp_ms: Some(0),
            worker: Some(0),
            phase: None,
            exclusive: false,
        },
        step("a.feature", 3, "a step", "A", Status::Passed, 1, 20, None),
        Event::ScenarioFinished {
            scenario: Arc::from("A"),
            file: Arc::from("a.feature"),
            status: Status::Passed,
            timestamp_ms: Some(40),
            worker: Some(0),
            phase: None,
            reason: None,
            tags: vec!["smoke".to_owned()],
        },
        Event::RunFinished {
            passed: 1,
            failed: 0,
            skipped: 0,
            cancelled: false,
        },
    ];
    let html = render_html(&events, "artifacts", &std::collections::BTreeMap::new());
    for (section, heading) in [
        ("<table class=\"tags\">", "id=\"by-tag\""),
        ("<details class=\"scenario", "id=\"scenarios\""),
        ("class=\"timeline\"", ">Timeline "),
    ] {
        assert!(
            html.contains(section),
            "fixture must render `{section}`: {html}"
        );
        assert!(
            html.contains(heading),
            "the `{section}` section has no heading: {html}"
        );
    }
    assert_eq!(html.matches("<h1").count(), 1, "{html}");
    assert_eq!(
        html.matches("<h2").count(),
        3,
        "one heading per section, no level skipped: {html}"
    );
}

/// The defect this field exists for, stated as a fixture: one feature sentence
/// lowering to two engine steps produces two `step_finished` with an identical
/// `StepRef` — same file, same line, same text — and previously two identical
/// rows, one green and one red, with nothing to say which was which.
#[test]
fn two_engine_steps_of_one_sentence_are_told_apart_by_their_labels() {
    let named = |label: &str, status| Event::StepFinished {
        scenario: Arc::from("S"),
        engine: Arc::from("http"),
        // Deliberately identical across both events: this is the whole point.
        step: StepRef {
            file: Arc::from("tests/features/a.feature"),
            line: 9,
            text: Arc::from("the workspace is provisioned"),
        },
        status,
        attempts: 1,
        duration_ms: 4,
        captures: Vec::new(),
        fragment: None,
        label: Some(label.to_owned()),
        detail: None,
        attempt_details: Vec::new(),
        reproduce_hint: None,
    };
    let events = vec![
        Event::RunStarted {
            schema: EVENT_SCHEMA_VERSION,
            run_id: Arc::from("run-1"),
            env: None,
            metadata: std::collections::BTreeMap::new(),
            shuffled: false,
            rerun_of: None,
        },
        named("fixture warm-up probe", Status::Warned),
        named("provision the environment", Status::Failed),
        finished("S", "tests/features/a.feature", Status::Failed),
    ];
    let html = render_html(&events, "artifacts", &std::collections::BTreeMap::new());
    for label in ["fixture warm-up probe", "provision the environment"] {
        assert!(
            html.contains(&format!("<span class=\"steplabel\"> › {label}</span>")),
            "the label must render beside the sentence it disambiguates: {html}"
        );
    }

    // The rows are no longer byte-identical — which is the property, not the
    // presence of the markup. Compare the two `<li>` rows directly.
    let rows: Vec<&str> = html
        .split("<li class=")
        .skip(1)
        .map(|row| row.split("</li>").next().unwrap_or(row))
        .collect();
    assert_eq!(rows.len(), 2, "two steps, two rows: {html}");
    assert_ne!(rows[0], rows[1], "the two rows still read identically");

    // A label carrying markup is escaped like every other authored string.
    let hostile = vec![
        Event::RunStarted {
            schema: EVENT_SCHEMA_VERSION,
            run_id: Arc::from("run-1"),
            env: None,
            metadata: std::collections::BTreeMap::new(),
            shuffled: false,
            rerun_of: None,
        },
        named("<script>alert(1)</script>", Status::Passed),
        finished("S", "tests/features/a.feature", Status::Passed),
    ];
    let html = render_html(&hostile, "artifacts", &std::collections::BTreeMap::new());
    assert!(!html.contains("<script>alert(1)</script>"), "{html}");
    assert!(html.contains("&lt;script&gt;"), "{html}");
}
