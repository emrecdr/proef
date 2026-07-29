//! Reporters (ADR-0008): composable consumers of the event spine.
//!
//! The JSONL run record **is** the appended event stream — no second record
//! format. The console reporter buffers per scenario (a natural `Normalize`:
//! parallel scenarios never interleave their lines) and prints a BDD tree with
//! attempts and engine-measured timings on completion.
//!
//! Secret values never enter events by construction (capture *names* only,
//! engine-redacted details); [`Redactions`] is the defense-in-depth applied to
//! every rendered string, property-tested in this module.

use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::event::{Event, EventSink};
use crate::step::Status;

/// Known secret values, replaced by `***` in every rendered string.
#[derive(Debug, Clone, Default)]
pub struct Redactions(Vec<String>);

impl Redactions {
    /// Redact these values (empty values are ignored — nothing to leak).
    pub fn new(values: impl IntoIterator<Item = String>) -> Self {
        Self(values.into_iter().filter(|v| !v.is_empty()).collect())
    }

    /// `text` with every known secret value replaced.
    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for value in &self.0 {
            out = out.replace(value, "***");
        }
        out
    }

    /// No values to redact.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The event with every string field redacted. The match is exhaustive on
    /// purpose: adding an event variant forces a redaction decision here, so
    /// the invariant (ADR-0005: secrets reach **no** sink) cannot silently
    /// erode as the schema grows.
    pub fn apply_event(&self, event: &Event) -> Event {
        let s = |text: &Arc<str>| -> Arc<str> { Arc::from(self.apply(text)) };
        match event {
            Event::RunStarted { schema, run_id } => Event::RunStarted {
                schema: *schema,
                run_id: s(run_id),
            },
            Event::ScenarioStarted { scenario, file } => Event::ScenarioStarted {
                scenario: s(scenario),
                file: s(file),
            },
            Event::BatchStarted {
                scenario,
                engine,
                steps,
            } => Event::BatchStarted {
                scenario: s(scenario),
                engine: s(engine),
                steps: *steps,
            },
            Event::EntryRunning {
                scenario,
                engine,
                entry,
                retry,
            } => Event::EntryRunning {
                scenario: s(scenario),
                engine: s(engine),
                entry: *entry,
                retry: *retry,
            },
            Event::StepFinished {
                scenario,
                engine,
                step,
                status,
                attempts,
                duration_ms,
                captures,
                detail,
            } => Event::StepFinished {
                scenario: s(scenario),
                engine: s(engine),
                step: crate::step::StepRef {
                    file: s(&step.file),
                    line: step.line,
                    text: s(&step.text),
                },
                status: *status,
                attempts: *attempts,
                duration_ms: *duration_ms,
                captures: captures.iter().map(|name| self.apply(name)).collect(),
                detail: detail.as_deref().map(|text| self.apply(text)),
            },
            Event::ScenarioFinished { scenario, status } => Event::ScenarioFinished {
                scenario: s(scenario),
                status: *status,
            },
            Event::RunFinished { .. } => event.clone(),
        }
    }
}

/// One reporter in the stack.
pub trait Reporter: Send {
    /// Consume one event.
    fn on_event(&mut self, event: &Event);
}

/// Fan a reporter stack out as an [`EventSink`] (thread-safe: scenario threads
/// share the sink). Redaction happens **here**, once, before fan-out — every
/// reporter (console, JSONL record, future sinks) sees only redacted events,
/// so the invariant does not depend on each leaf remembering to redact.
pub fn sink(reporters: Vec<Box<dyn Reporter>>, redactions: Redactions) -> EventSink {
    let stack = Arc::new(Mutex::new(reporters));
    EventSink::new(move |event| {
        if let Ok(mut stack) = stack.lock() {
            if redactions.is_empty() {
                for reporter in stack.iter_mut() {
                    reporter.on_event(event);
                }
            } else {
                let redacted = redactions.apply_event(event);
                for reporter in stack.iter_mut() {
                    reporter.on_event(&redacted);
                }
            }
        }
    })
}

/// Console BDD tree, buffered per scenario.
pub struct ConsoleReporter<W: Write + Send> {
    out: W,
    redactions: Redactions,
    buffers: Vec<(Arc<str>, Vec<String>)>,
}

impl<W: Write + Send> ConsoleReporter<W> {
    /// A console reporter writing to `out`.
    pub fn new(out: W, redactions: Redactions) -> Self {
        Self {
            out,
            redactions,
            buffers: Vec::new(),
        }
    }

    fn buffer_for(&mut self, scenario: &Arc<str>) -> &mut Vec<String> {
        if let Some(position) = self.buffers.iter().position(|(name, _)| name == scenario) {
            &mut self.buffers[position].1
        } else {
            self.buffers.push((Arc::clone(scenario), Vec::new()));
            &mut self
                .buffers
                .last_mut()
                .unwrap_or_else(|| unreachable!("buffer just pushed"))
                .1
        }
    }
}

fn glyph(status: Status) -> &'static str {
    match status {
        Status::Passed => "✓",
        Status::Failed => "✗",
        Status::Skipped => "∅",
        Status::Warned => "⚠",
    }
}

impl<W: Write + Send> Reporter for ConsoleReporter<W> {
    fn on_event(&mut self, event: &Event) {
        match event {
            Event::RunStarted { run_id, .. } => {
                let _ = writeln!(self.out, "proef run {run_id}");
            }
            Event::ScenarioStarted { scenario, file } => {
                let header = format!("\n  Scenario: {scenario} ({file})");
                self.buffer_for(scenario).push(header);
            }
            // Buffered console renders on completion; the live progress
            // signal is for streaming consumers (JSONL record, future TTY).
            Event::BatchStarted { .. } | Event::EntryRunning { .. } => {}
            Event::StepFinished {
                scenario,
                step,
                status,
                attempts,
                duration_ms,
                detail,
                ..
            } => {
                let attempts_note = if *attempts > 1 {
                    format!(", {attempts} attempts")
                } else {
                    String::new()
                };
                let line = format!(
                    "    {} {}:{} — {} ({duration_ms}ms{attempts_note})",
                    glyph(*status),
                    step.file,
                    step.line,
                    step.text
                );
                let line = self.redactions.apply(&line);
                // A warning with no reason is unusable — say why. (Failures
                // get the richer end-of-run list instead.)
                let warn_detail = (*status == Status::Warned)
                    .then_some(detail.as_deref())
                    .flatten()
                    .map(|d| self.redactions.apply(&format!("      ↳ {d}")));
                let buffer = self.buffer_for(scenario);
                buffer.push(line);
                if let Some(warn_detail) = warn_detail {
                    buffer.push(warn_detail);
                }
            }
            Event::ScenarioFinished { scenario, status } => {
                let lines = self
                    .buffers
                    .iter()
                    .position(|(name, _)| name == scenario)
                    .map(|position| self.buffers.remove(position).1)
                    .unwrap_or_default();
                for line in lines {
                    let _ = writeln!(self.out, "{line}");
                }
                let _ = writeln!(self.out, "    {} scenario {scenario}", glyph(*status));
            }
            Event::RunFinished {
                passed,
                failed,
                skipped,
                cancelled,
            } => {
                let note = if *cancelled { " · cancelled" } else { "" };
                let _ = writeln!(
                    self.out,
                    "\nsummary: {passed} passed · {failed} failed · {skipped} skipped{note}"
                );
                let _ = self.out.flush();
            }
        }
    }
}

/// Run totals derived from the event stream — the `Summarize` leg of the
/// decorator stack (ADR-0008): leaves (GitHub summary, `--output json`, …)
/// consume the totals at `RunFinished` instead of re-deriving them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunTotals {
    /// Scenarios that passed.
    pub passed: usize,
    /// Scenarios that failed.
    pub failed: usize,
    /// Scenarios skipped.
    pub skipped: usize,
    /// Steps finished (all statuses).
    pub steps: usize,
    /// Total attempts across steps (retries included).
    pub attempts: u64,
}

impl RunTotals {
    /// Fold one event into the totals.
    pub fn observe(&mut self, event: &Event) {
        match event {
            Event::StepFinished { attempts, .. } => {
                self.steps += 1;
                self.attempts += u64::from(*attempts);
            }
            Event::RunFinished {
                passed,
                failed,
                skipped,
                ..
            } => {
                self.passed = *passed;
                self.failed = *failed;
                self.skipped = *skipped;
            }
            _ => {}
        }
    }
}

/// JSONL appender: the run record is the raw event stream, in arrival order
/// (replays and tests normalize; TESTING-STRATEGY flake rule).
pub struct JsonlReporter<W: Write + Send> {
    out: W,
}

impl<W: Write + Send> JsonlReporter<W> {
    /// A JSONL reporter writing to `out`.
    pub fn new(out: W) -> Self {
        Self { out }
    }
}

impl<W: Write + Send> Reporter for JsonlReporter<W> {
    fn on_event(&mut self, event: &Event) {
        if let Ok(json) = serde_json::to_string(event) {
            let _ = writeln!(self.out, "{json}");
        }
        if matches!(event, Event::RunFinished { .. }) {
            let _ = self.out.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::step::StepRef;

    fn sample_events() -> Vec<Event> {
        vec![
            Event::RunStarted {
                schema: 1,
                run_id: Arc::from("run-1"),
            },
            Event::ScenarioStarted {
                scenario: Arc::from("S"),
                file: Arc::from("f.feature"),
            },
            Event::StepFinished {
                scenario: Arc::from("S"),
                engine: Arc::from("hurl"),
                step: StepRef {
                    file: Arc::from("f.feature"),
                    line: 3,
                    text: Arc::from("I log in"),
                },
                status: Status::Passed,
                attempts: 2,
                duration_ms: 12,
                captures: vec!["token".to_owned()],
                detail: None,
            },
            Event::ScenarioFinished {
                scenario: Arc::from("S"),
                status: Status::Passed,
            },
            Event::RunFinished {
                passed: 1,
                failed: 0,
                skipped: 0,
                cancelled: false,
            },
        ]
    }

    #[test]
    fn console_buffers_per_scenario_and_prints_on_finish() {
        let mut out = Vec::new();
        {
            let mut console = ConsoleReporter::new(&mut out, Redactions::default());
            for event in sample_events() {
                console.on_event(&event);
            }
        }
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Scenario: S (f.feature)"), "{text}");
        assert!(
            text.contains("✓ f.feature:3 — I log in (12ms, 2 attempts)"),
            "{text}"
        );
        assert!(
            text.contains("summary: 1 passed · 0 failed · 0 skipped"),
            "{text}"
        );
    }

    #[test]
    fn jsonl_is_the_event_stream() {
        let mut out = Vec::new();
        {
            let mut jsonl = JsonlReporter::new(&mut out);
            for event in sample_events() {
                jsonl.on_event(&event);
            }
        }
        let text = String::from_utf8(out).unwrap();
        let parsed: Vec<Event> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(parsed, sample_events());
    }

    #[test]
    fn totals_fold_steps_and_run_counts() {
        let mut totals = RunTotals::default();
        for event in sample_events() {
            totals.observe(&event);
        }
        assert_eq!(
            totals,
            RunTotals {
                passed: 1,
                failed: 0,
                skipped: 0,
                steps: 1,
                attempts: 2,
            }
        );
    }

    mod properties {
        #![allow(clippy::ignored_unit_patterns)]

        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// The secret-mask invariant (ADR-0005, TESTING-STRATEGY): for any
            /// rendered text containing a known secret value, the redacted
            /// output never contains that value.
            #[test]
            fn redaction_removes_known_values(
                secret in "[a-zA-Z0-9]{4,24}",
                prefix in ".{0,30}",
                suffix in ".{0,30}",
            ) {
                let redactions = Redactions::new([secret.clone()]);
                let rendered = format!("{prefix}{secret}{suffix}");
                let redacted = redactions.apply(&rendered);
                prop_assert!(!redacted.contains(&secret));
            }

            /// The sink boundary redacts before fan-out: the JSONL run record
            /// (and therefore every reporter) never contains a known secret,
            /// wherever it appears in an event's string fields.
            #[test]
            fn sink_redacts_before_any_reporter(secret in "[a-zA-Z0-9]{6,20}") {
                #[derive(Clone)]
                struct Shared(Arc<Mutex<Vec<u8>>>);
                impl Write for Shared {
                    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                        if let Ok(mut out) = self.0.lock() {
                            out.extend_from_slice(buf);
                        }
                        Ok(buf.len())
                    }
                    fn flush(&mut self) -> std::io::Result<()> {
                        Ok(())
                    }
                }
                let out = Shared(Arc::new(Mutex::new(Vec::new())));
                let sink = sink(
                    vec![Box::new(JsonlReporter::new(out.clone()))],
                    Redactions::new([secret.clone()]),
                );
                sink.emit(&Event::ScenarioStarted {
                    scenario: Arc::from(format!("uses {secret}")),
                    file: Arc::from(format!("{secret}.feature")),
                });
                sink.emit(&Event::StepFinished {
                    scenario: Arc::from(format!("uses {secret}")),
                    engine: Arc::from("hurl"),
                    step: crate::step::StepRef {
                        file: Arc::from(format!("{secret}.feature")),
                        line: 1,
                        text: Arc::from(format!("token is {secret}")),
                    },
                    status: Status::Failed,
                    attempts: 1,
                    duration_ms: 1,
                    captures: vec![format!("cap-{secret}")],
                    detail: Some(format!("boom {secret}")),
                });
                sink.emit(&Event::RunFinished {
                    passed: 0,
                    failed: 1,
                    skipped: 0,
                    cancelled: false,
                });
                let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
                prop_assert!(!text.is_empty());
                prop_assert!(!text.contains(&secret), "{text}");
            }

            /// Console output never leaks a secret embedded in step text.
            #[test]
            fn console_never_prints_known_secrets(secret in "[a-zA-Z0-9]{6,20}") {
                let mut out = Vec::new();
                {
                    let mut console = ConsoleReporter::new(
                        &mut out,
                        Redactions::new([secret.clone()]),
                    );
                    console.on_event(&Event::ScenarioStarted {
                        scenario: Arc::from("S"),
                        file: Arc::from("f"),
                    });
                    console.on_event(&Event::StepFinished {
                        scenario: Arc::from("S"),
                        engine: Arc::from("hurl"),
                        step: StepRef {
                            file: Arc::from("f"),
                            line: 1,
                            text: Arc::from(format!("token is {secret}")),
                        },
                        status: Status::Failed,
                        attempts: 1,
                        duration_ms: 1,
                        captures: Vec::new(),
                        detail: None,
                    });
                    console.on_event(&Event::ScenarioFinished {
                        scenario: Arc::from("S"),
                        status: Status::Failed,
                    });
                }
                let text = String::from_utf8(out).unwrap();
                prop_assert!(!text.contains(&secret), "{text}");
            }
        }
    }
}
