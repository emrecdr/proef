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
    teardown: Option<&RunSummary>,
    carried: &[ScenarioOutcome],
    non_gating: &[(String, String)],
    run_id: &str,
    path: &Path,
    redactions: &Redactions,
) -> Result<(), String> {
    let mut report = Report::new("proef");
    report.set_uuid(uuid::Uuid::parse_str(run_id).unwrap_or_else(|_| uuid::Uuid::nil()));

    // A failed teardown's outcomes ride along as their own suite (named by
    // the phase feature file, like #78's setup) — a JUnit-gated pipeline used
    // to see a fully-passing report on an exit-3 run (R17-2.5). The summary
    // totals elsewhere stay suite-only (ADR-0014); JUnit counts are per-case
    // by construction, so the phase failure is visible without touching them.
    let outcomes = || {
        summary
            .outcomes
            .iter()
            .chain(carried.iter())
            .chain(teardown.into_iter().flat_map(|t| t.outcomes.iter()))
    };
    let mut files: Vec<&str> = outcomes().map(|o| o.file.as_ref()).collect();
    files.sort_unstable();
    files.dedup();

    let mut total = std::time::Duration::ZERO;
    for file in files {
        let mut suite = TestSuite::new(file);
        let mut suite_time = std::time::Duration::ZERO;
        for outcome in outcomes().filter(|o| o.file.as_ref() == file) {
            suite_time += outcome
                .steps
                .iter()
                .map(|s| s.duration)
                .sum::<std::time::Duration>();
            suite.add_test_case(test_case(
                outcome,
                non_gating.iter().any(|(file, name)| {
                    file.as_str() == outcome.file.as_ref() && name.as_str() == outcome.name.as_ref()
                }),
                redactions,
            ));
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

fn test_case(outcome: &ScenarioOutcome, quarantined: bool, redactions: &Redactions) -> TestCase {
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
        (Status::Skipped, _) => {
            let mut status = TestCaseStatus::skipped();
            if let Some(reason) = outcome
                .reason
                .as_deref()
                .map(ToOwned::to_owned)
                .or_else(|| {
                    outcome
                        .steps
                        .iter()
                        .find(|s| s.status == Status::Skipped)
                        .and_then(|s| s.detail.clone())
                })
            {
                let reason = redactions.apply(&reason);
                // Attribute *and* text node, here and on every non-success
                // below: Azure reads `message=` as the error-message field
                // and the element text as the stack trace; GitLab parses
                // only the text. Either alone loses half the platforms —
                // an authored `@skip:reason` rode the attribute only, so
                // GitLab showed a bare skip with the reason nowhere.
                status.set_message(reason.clone());
                status.set_description(reason);
            }
            status
        }
        // A quarantined test-failure does not gate the exit code, and the
        // XML must not say otherwise: Jenkins marks UNSTABLE on a plain
        // <failure> regardless of exit 0, so every dashboard contradicted
        // the verdict (RF-audit; RF converts the status for the same
        // reason). User/System faults stay failures — quarantine is for
        // flaky tests, not broken input.
        (Status::Failed, None) if quarantined => {
            let mut status = TestCaseStatus::skipped();
            let detail = outcome
                .steps
                .iter()
                .filter(|s| s.status == Status::Failed)
                .filter_map(|s| s.detail.as_deref())
                .collect::<Vec<_>>()
                .join("; ");
            let text = redactions.apply(&format!("quarantined failure (non-gating): {detail}"));
            status.set_message(text.clone());
            status.set_description(text);
            status
        }
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
            let message = redactions.apply(&message);
            status.set_message(message.clone());
            // The text node carries the message plus each failing step's
            // reproduce hint — the content channel has room the one-line
            // attribute does not, and the hint is the artifact the reader
            // actually wants from a CI results page.
            let mut description = message;
            for step in outcome.steps.iter().filter(|s| s.status == Status::Failed) {
                if let Some(hint) = &step.reproduce_hint {
                    let _ = write!(description, "\nreproduce: {}", redactions.apply(hint));
                }
            }
            status.set_description(description);
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
    // identity — deliberately not carried here at all: the failure detail
    // names the *artifact* line (the replayable thing), and the feature line
    // is one `proef explain` away. Consumers read `file`, not `line`.
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
pub fn write_github_summary(
    summary: &RunSummary,
    tag_links: &std::collections::BTreeMap<String, String>,
    run_id: &str,
    redactions: &Redactions,
) {
    let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") else {
        return;
    };
    let body = summary_body(summary, tag_links, run_id);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
    {
        use std::io::Write as _;
        // One pass over the final body covers names and details alike; the
        // cap runs after redaction so the budget is measured on the bytes
        // actually written.
        let _ = writeln!(file, "{}", capped_summary(redactions.apply(&body)));
    }
}

/// The summary's markdown, separated from the file append so it can be asserted
/// without an env var — `std::env::set_var` is `unsafe` in edition 2024, and a
/// sink that can only be tested by mutating process state tends not to be.
fn summary_body(
    summary: &RunSummary,
    tag_links: &std::collections::BTreeMap<String, String>,
    run_id: &str,
) -> String {
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
    // Per-tag rollup (RF's statistics-by-tag): the suite summary's outcomes
    // are suite-only by construction (phases run their own), so the table
    // needs no phase filter; Warned counts with passed like RunSummary does.
    let mut tag_rows: std::collections::BTreeMap<&str, (usize, usize, usize)> =
        std::collections::BTreeMap::new();
    for outcome in &summary.outcomes {
        for tag in outcome.tags.iter() {
            let row = tag_rows.entry(tag.as_str()).or_default();
            match outcome.status {
                Status::Passed | Status::Warned => row.0 += 1,
                Status::Failed => row.1 += 1,
                Status::Skipped => row.2 += 1,
            }
        }
    }
    if !tag_rows.is_empty() {
        body.push_str("\n### by tag\n\n| tag | passed | failed | skipped |\n|---|---|---|---|\n");
        for (tag, (passed, failed, skipped)) in tag_rows {
            // `[tag-links]`: a matching glob turns the cell into a tracker
            // link — same mechanism as the HTML table, markdown spelling.
            // The substituted tag is percent-encoded: it is free prose, and a
            // `)` or `|` in it closed the markdown link (or split the table
            // row) exactly the way `enc_cell` was introduced to prevent for
            // the label. Non-http(s) templates render as plain text — a
            // template is config, but the *tag* riding into a `javascript:`
            // href is not a link this summary should mint.
            let cell = tag_links
                .iter()
                .find(|(pattern, _)| proef_core::tags::atom_matches_public(pattern, tag))
                .filter(|(_, template)| {
                    template.starts_with("https://") || template.starts_with("http://")
                })
                .map_or_else(
                    || format!("@{}", enc_cell(tag)),
                    |(_, template)| {
                        format!(
                            "[@{}]({})",
                            enc_cell(tag),
                            template.replace("{tag}", &enc_url_component(tag))
                        )
                    },
                );
            let _ = writeln!(body, "| {cell} | {passed} | {failed} | {skipped} |");
        }
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
/// this only under Actions AND when the human report — not `--format json` —
/// owns stdout.
pub fn github_annotations(summary: &RunSummary, redactions: &Redactions) -> String {
    // GitHub keeps at most ten error annotations per step and *silently
    // drops* the rest — an uncapped list made a forty-failure run look like
    // exactly ten failures, indistinguishable from a real ten. Budgeting one
    // annotation per failing *scenario* (its first failing step with detail,
    // else its fault) spreads the ten across ten scenarios instead of
    // spending them all on one scenario's step list, and the closing
    // `::notice` says what the ten are out of.
    const MAX_ERROR_ANNOTATIONS: usize = 10;
    let mut out = String::new();
    let mut failing = 0usize;
    for outcome in &summary.outcomes {
        let annotation = outcome
            .steps
            .iter()
            .find(|step| step.status == Status::Failed && step.detail.is_some())
            .map(|step| {
                let detail = step.detail.as_deref().unwrap_or_default();
                format!(
                    "::error file={},line={},title={}::{}",
                    enc_prop(&step.step.file),
                    step.step.line,
                    // GitHub caps `title` at 255 characters; scenario names
                    // and step text are free prose, so clip before encoding
                    // (encoding expands, never shrinks).
                    enc_prop(&clip_chars(
                        &format!("{}: {}", outcome.name, step.step.text),
                        200
                    )),
                    enc_msg(
                        &redactions.apply(&format!("{detail}{}", via(step.fragment.as_deref())))
                    ),
                )
            })
            .or_else(|| {
                // Scenario-level faults (user/system) have no failing step to
                // anchor — annotate the scenario header line instead.
                let (Fault::System(message) | Fault::User(message)) = outcome.fault.as_ref()?;
                Some(format!(
                    "::error file={},line={},title={}::{}",
                    enc_prop(&outcome.file),
                    outcome.line,
                    enc_prop(&clip_chars(&outcome.name, 200)),
                    enc_msg(&redactions.apply(message)),
                ))
            });
        if let Some(annotation) = annotation {
            failing += 1;
            if failing <= MAX_ERROR_ANNOTATIONS {
                let _ = writeln!(out, "{annotation}");
            }
        }
    }
    if failing > MAX_ERROR_ANNOTATIONS {
        let _ = writeln!(
            out,
            "::notice title=proef::showing {MAX_ERROR_ANNOTATIONS} of {failing} failing \
             scenarios — the full list is in the job summary"
        );
    }
    out
}

/// The job-summary byte budget, safely under GitHub's 1 MiB-per-step cap.
/// The failure mode *at* the cap is documented-silent — the summary simply
/// never appears, and near it an oversized write has aborted jobs in shipped
/// first-party actions — so a summary that names its own truncation beats a
/// complete one nobody sees. A failing rerun-overlay suite with per-tag
/// tables crosses 1 MiB more easily than it looks.
const SUMMARY_BUDGET_BYTES: usize = 900 * 1024;

/// `body`, truncated at a line boundary under [`SUMMARY_BUDGET_BYTES`] with a
/// trailer saying how much was cut and where the full detail lives.
fn capped_summary(body: String) -> String {
    if body.len() <= SUMMARY_BUDGET_BYTES {
        return body;
    }
    let mut end = SUMMARY_BUDGET_BYTES;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    let cut = body[..end].rfind('\n').unwrap_or(0);
    let omitted = body[cut..].lines().count();
    format!(
        "{}\n\n…truncated: {omitted} more line(s) omitted to stay under GitHub's 1 MiB \
         summary cap — the full detail is in the run record (`proef explain`) and the \
         HTML report\n",
        &body[..cut]
    )
}

/// The first `max` characters of `s` (char-counted, so no mid-codepoint cut),
/// with an ellipsis when anything was dropped.
fn clip_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
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

/// Percent-encode a tag for the `{tag}` slot of a `[tag-links]` URL template.
/// RFC 3986 unreserved characters pass through; everything else — including
/// the `)` that closes a markdown link and the `|` that splits a table row —
/// is encoded, so no tag spelling can escape the URL position.
fn enc_url_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("%{byte:02X}"));
            }
        }
    }
    out
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
                reason: None,
                tags: std::sync::Arc::from(Vec::new()),
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
        let summary_md = summary_body(&summary, &std::collections::BTreeMap::new(), "run-1");
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
            !summary_body(&inline, &std::collections::BTreeMap::new(), "run-1").contains("via "),
            "an inline step has no fragment and must not render an empty one"
        );
    }
    /// The by-tag rollup: one row per tag, Warned counts with passed,
    /// rendered only when any outcome carries tags.
    #[test]
    fn the_summary_rolls_up_by_tag() {
        let outcome = |name: &str, status: Status, tags: &[&str]| ScenarioOutcome {
            file: "f.feature".into(),
            name: name.into(),
            line: 1,
            status,
            reason: None,
            tags: Arc::from(tags.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>()),
            steps: Vec::new(),
            fault: None,
            artifact_slug: None,
        };
        let summary = RunSummary {
            outcomes: vec![
                outcome("a", Status::Passed, &["smoke", "api"]),
                outcome("b", Status::Failed, &["api"]),
                outcome("c", Status::Warned, &["smoke"]),
            ],
            passed: 2,
            failed: 1,
            skipped: 0,
            cancelled: false,
        };
        let body = summary_body(&summary, &std::collections::BTreeMap::new(), "run-1");
        assert!(body.contains("### by tag"), "{body}");
        assert!(body.contains("| @api | 1 | 1 | 0 |"), "{body}");

        // `[tag-links]`: a matching glob linkifies the cell; non-matching
        // tags stay plain.
        let links: std::collections::BTreeMap<String, String> =
            [("smoke".to_owned(), "https://ci.example/t/{tag}".to_owned())].into();
        let linked = summary_body(&summary, &links, "run-1");
        assert!(
            linked.contains("| [@smoke](https://ci.example/t/smoke) | 2 | 0 | 0 |"),
            "{linked}"
        );
        assert!(
            linked.contains("| @api | 1 | 1 | 0 |"),
            "unmatched stays plain: {linked}"
        );
        assert!(
            body.contains("| @smoke | 2 | 0 | 0 |"),
            "warned counts with passed: {body}"
        );

        let untagged = RunSummary {
            outcomes: vec![outcome("a", Status::Passed, &[])],
            passed: 1,
            failed: 0,
            skipped: 0,
            cancelled: false,
        };
        assert!(
            !summary_body(&untagged, &std::collections::BTreeMap::new(), "run-1")
                .contains("### by tag"),
            "a tagless suite gets no empty section"
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
                reason: None,
                tags: std::sync::Arc::from(Vec::new()),
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

#[cfg(test)]
mod junit_tests {
    #![allow(clippy::unwrap_used)]

    use super::write_junit;
    use proef_core::report::Redactions;
    use proef_core::runner::{RunSummary, ScenarioOutcome};
    use proef_core::step::{Status, StepOutcome, StepRef};
    use std::sync::Arc;

    fn outcome(file: &str, name: &str, status: Status, detail: Option<&str>) -> ScenarioOutcome {
        ScenarioOutcome {
            file: file.into(),
            name: name.into(),
            line: 2,
            status,
            reason: None,
            tags: Arc::from(Vec::new()),
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
            }],
            fault: None,
            artifact_slug: None,
        }
    }

    fn summary(outcomes: Vec<ScenarioOutcome>) -> RunSummary {
        let failed = outcomes
            .iter()
            .filter(|o| o.status == Status::Failed)
            .count();
        let passed = outcomes.len() - failed;
        RunSummary {
            outcomes,
            passed,
            failed,
            skipped: 0,
            cancelled: false,
        }
    }

    fn serialize(
        run: &RunSummary,
        teardown: Option<&RunSummary>,
        carried: &[ScenarioOutcome],
    ) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.junit.xml");
        write_junit(
            run,
            teardown,
            carried,
            &[],
            "run-1",
            &path,
            &Redactions::default(),
        )
        .unwrap();
        std::fs::read_to_string(&path).unwrap()
    }

    /// Failure detail must reach the element *text node*, not only the
    /// `message` attribute: GitLab parses only the content, Azure maps the
    /// content to its stack-trace field — either alone loses half the
    /// platforms. The reproduce hint rides the content channel, where a
    /// one-line attribute has no room.
    #[test]
    fn failure_detail_reaches_attribute_and_text_node_alike() {
        let xml = serialize(
            &summary(vec![outcome(
                "a.feature",
                "S",
                Status::Failed,
                Some("Assert status code"),
            )]),
            None,
            &[],
        );
        assert!(
            xml.contains(r#"message="Assert status code""#),
            "attribute: {xml}"
        );
        assert!(
            xml.contains(">Assert status code\nreproduce: curl -X GET http://x<"),
            "text node with the reproduce hint: {xml}"
        );
    }

    /// quick-junit's `XmlString` strips ANSI escapes and XML-1.0-illegal
    /// control characters on every setter — the one protection that keeps a
    /// binary response byte from invalidating the whole report on
    /// Jenkins/GitLab (their parsers reject the file outright, not the one
    /// test). Pinned here so a quick-junit bump cannot shed it silently.
    #[test]
    fn illegal_bytes_and_ansi_never_reach_the_xml() {
        let xml = serialize(
            &summary(vec![outcome(
                "a.feature",
                "S",
                Status::Failed,
                Some("\u{1b}[31mred\u{0}null\u{1b}[0m plain"),
            )]),
            None,
            &[],
        );
        assert!(!xml.contains('\u{1b}'), "no ESC byte survives: {xml}");
        assert!(!xml.contains('\u{0}'), "no NUL byte survives: {xml}");
        assert!(
            xml.contains("rednull plain"),
            "the text itself stays: {xml}"
        );
    }

    /// `time` is seconds with exactly three decimals, never exponent
    /// notation — the strictest reference schema pattern, and the format
    /// Azure sums into the run duration. Library-provided today; pinned so
    /// it stays that way.
    #[test]
    fn times_are_three_decimal_seconds() {
        let xml = serialize(
            &summary(vec![outcome("a.feature", "S", Status::Passed, None)]),
            None,
            &[],
        );
        let times: Vec<&str> = xml
            .split("time=\"")
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap())
            .collect();
        assert!(!times.is_empty());
        for time in times {
            assert!(
                time.chars().all(|c| c.is_ascii_digit() || c == '.')
                    && time.split('.').nth(1).map(str::len) == Some(3),
                "not a 3-decimal plain number: {time}"
            );
        }
    }

    /// `classname`+`name` is the identity every consumer keys history on,
    /// and GitLab *silently drops* duplicates — so the composed report
    /// (suite + rerun-carried + teardown) must yield each identity exactly
    /// once. Pinned over the real composition path.
    #[test]
    fn composed_identities_form_a_set() {
        let run = summary(vec![
            outcome("a.feature", "S1", Status::Failed, Some("boom")),
            outcome("a.feature", "S2", Status::Passed, None),
        ]);
        let teardown = summary(vec![outcome(
            "teardown.feature",
            "cleanup",
            Status::Failed,
            Some("boom"),
        )]);
        let carried = vec![outcome("b.feature", "S1", Status::Passed, None)];
        let xml = serialize(&run, Some(&teardown), &carried);
        let mut identities: Vec<(String, String)> = Vec::new();
        for case in xml.split("<testcase ").skip(1) {
            let attr = |key: &str| {
                case.split(&format!("{key}=\""))
                    .nth(1)
                    .unwrap()
                    .split('"')
                    .next()
                    .unwrap()
                    .to_owned()
            };
            identities.push((attr("classname"), attr("name")));
        }
        assert_eq!(identities.len(), 4, "{xml}");
        let unique: std::collections::BTreeSet<_> = identities.iter().collect();
        assert_eq!(
            unique.len(),
            identities.len(),
            "duplicate identity: {identities:?}"
        );
    }
}

#[cfg(test)]
mod budget_tests {
    #![allow(clippy::unwrap_used)]

    use super::{SUMMARY_BUDGET_BYTES, capped_summary, clip_chars, github_annotations};
    use proef_core::report::Redactions;
    use proef_core::runner::{RunSummary, ScenarioOutcome};
    use proef_core::step::{Status, StepOutcome, StepRef};
    use std::sync::Arc;

    fn failing(name: &str, steps: usize) -> ScenarioOutcome {
        ScenarioOutcome {
            file: "a.feature".into(),
            name: name.into(),
            line: 2,
            status: Status::Failed,
            reason: None,
            tags: Arc::from(Vec::new()),
            steps: (0..steps)
                .map(|i| StepOutcome {
                    step: StepRef {
                        file: Arc::from("a.feature"),
                        line: 3 + i,
                        text: Arc::from("a step"),
                    },
                    status: Status::Failed,
                    attempts: 1,
                    duration: std::time::Duration::from_millis(1),
                    detail: Some("boom".to_owned()),
                    attempt_details: Vec::new(),
                    reproduce_hint: None,
                    fragment: None,
                })
                .collect(),
            fault: None,
            artifact_slug: None,
        }
    }

    /// GitHub silently drops error annotations past ten per step: emitting
    /// eleven made a forty-failure run *look like* exactly ten. The budget
    /// is one annotation per failing scenario, and the closing notice names
    /// what the ten are out of.
    #[test]
    fn annotations_cap_at_ten_with_an_honest_notice() {
        let run = RunSummary {
            outcomes: (0..12).map(|i| failing(&format!("S{i}"), 3)).collect(),
            passed: 0,
            failed: 12,
            skipped: 0,
            cancelled: false,
        };
        let out = github_annotations(&run, &Redactions::default());
        assert_eq!(out.matches("::error ").count(), 10, "{out}");
        assert!(out.contains("showing 10 of 12"), "{out}");
        // One annotation per failing *scenario* — a three-failing-step
        // scenario must not spend three of the ten slots.
        let one = github_annotations(
            &RunSummary {
                outcomes: vec![failing("S", 3)],
                passed: 0,
                failed: 1,
                skipped: 0,
                cancelled: false,
            },
            &Redactions::default(),
        );
        assert_eq!(one.matches("::error ").count(), 1, "{one}");
        assert!(!one.contains("::notice"), "no notice under the cap: {one}");
    }

    /// The 1 MiB failure mode is silent disappearance, so the cap truncates
    /// deterministically at a line boundary and says what was cut.
    #[test]
    fn an_oversized_summary_truncates_and_says_so() {
        let line = "x".repeat(99);
        let big = (0..(SUMMARY_BUDGET_BYTES / 100 + 100))
            .map(|_| line.clone())
            .collect::<Vec<_>>()
            .join("\n");
        let capped = capped_summary(big.clone());
        assert!(
            capped.len() < SUMMARY_BUDGET_BYTES + 300,
            "{}",
            capped.len()
        );
        assert!(capped.contains("…truncated:"), "names its own truncation");
        assert!(capped.contains("more line(s) omitted"));
        let small = "fine".to_owned();
        assert_eq!(
            capped_summary(small.clone()),
            small,
            "under budget: unchanged"
        );
    }

    #[test]
    fn clip_is_char_counted() {
        assert_eq!(clip_chars("abc", 5), "abc");
        assert_eq!(clip_chars("ééééé", 3), "ééé…");
    }
}
