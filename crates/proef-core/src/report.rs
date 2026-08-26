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

/// Known secret values, replaced by `***` in every rendered string —
/// **including their common encoded forms**.
///
/// Exact-match on the raw bytes is not enough, demonstrated live: a server
/// that reflects a bearer token base64-encoded (an OAuth introspection
/// endpoint, a debug echo, a JWT claim) puts `dG9r…` into an assert-failure
/// detail, the raw needle never fires, and a string trivially decodable back
/// to the live credential lands in the console and `events.jsonl` — the
/// retained record CI uploads. GitHub's own masking documents the same
/// limitation and the same remedy: register each transformed value as a
/// needle too. So [`Self::new`] derives, per secret: base64 (standard and
/// URL-safe alphabets, with and without padding), lowercase and uppercase
/// hex, RFC 3986 percent-encoding, and the JSON-string escape. Derivation
/// happens *here* so every construction site — the CLI sink, the engine's
/// internal renderer, TAP — is covered by construction rather than by each
/// remembering.
///
/// This cannot be complete, and does not claim to be: a secret reflected
/// hashed, split, or re-encrypted matches no needle list. The set covers the
/// reversible transforms that actually occur at HTTP boundaries.
/// Over-redaction is the accepted failure direction — a false `***` in a
/// detail string costs a puzzled reader, a false miss costs a credential.
#[derive(Debug, Clone, Default)]
pub struct Redactions(Vec<String>);

impl Redactions {
    /// Redact these values and their derived encoded forms (empty values are
    /// ignored — nothing to leak).
    pub fn new(values: impl IntoIterator<Item = String>) -> Self {
        let mut needles: Vec<String> = Vec::new();
        for value in values.into_iter().filter(|value| !value.is_empty()) {
            needles.extend(derived_forms(&value));
            needles.push(value);
        }
        needles.sort_unstable();
        needles.dedup();
        // Longest first: the unpadded base64 form is a prefix of the padded
        // one, and replacing the short needle first would leave a dangling
        // `***==`. Harmless to the invariant, confusing to a reader.
        needles.sort_by_key(|needle| std::cmp::Reverse(needle.len()));
        Self(needles)
    }

    /// `text` with every known secret value replaced.
    pub fn apply(&self, text: &str) -> String {
        match self.applied(text) {
            Some(redacted) => redacted,
            None => text.to_owned(),
        }
    }

    /// `Some(redacted)` when a needle occurred, `None` when the text is clean.
    ///
    /// The split exists for the miss path, which is nearly every call: this
    /// runs per string field per event, inside the sink and **under the
    /// reporter-stack mutex**, and deriving encoded forms multiplied the
    /// needle count roughly ninefold per secret. A `contains` probe is a scan
    /// with no allocation; `replace` allocates a fresh copy of the whole text
    /// per needle whether or not anything matched. On a clean field —
    /// a secret in a rendered string is the exceptional case — this now
    /// allocates nothing, and [`Self::apply_event`] can hand the original
    /// `Arc` back instead of re-wrapping an identical string per field.
    fn applied(&self, text: &str) -> Option<String> {
        let mut out: Option<String> = None;
        for needle in &self.0 {
            let haystack = out.as_deref().unwrap_or(text);
            if haystack.contains(needle.as_str()) {
                out = Some(haystack.replace(needle.as_str(), "***"));
            }
        }
        out
    }

    /// No values to redact.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Does `text` carry any guarded value — raw, or in a derived encoded
    /// form? The probe half of [`Self::apply`]: a scan, no allocation. What
    /// the World's `saveAs: global` gate asks before persisting a capture
    /// (ADR-0005) — one needle set for redaction and refusal, never two.
    pub fn taints(&self, text: &str) -> bool {
        self.0.iter().any(|needle| text.contains(needle.as_str()))
    }

    /// The event with every string field redacted. The match is exhaustive on
    /// purpose: adding an event variant forces a redaction decision here, so
    /// the invariant (ADR-0005: secrets reach **no** sink) cannot silently
    /// erode as the schema grows.
    pub fn apply_event(&self, event: &Event) -> Event {
        // Clean fields — nearly all of them — keep their original `Arc`.
        let s = |text: &Arc<str>| -> Arc<str> {
            match self.applied(text) {
                Some(redacted) => Arc::from(redacted),
                None => Arc::clone(text),
            }
        };
        match event {
            Event::RunStarted {
                schema,
                run_id,
                env,
                metadata,
                shuffled,
                rerun_of,
            } => Event::RunStarted {
                schema: *schema,
                run_id: s(run_id),
                env: env
                    .as_deref()
                    .map(|text| Arc::from(self.apply(text).as_str())),
                // Keys and values both masked — a user can paste a
                // secret-bearing URL into either position.
                metadata: metadata
                    .iter()
                    .map(|(key, value)| (self.apply(key), self.apply(value)))
                    .collect(),
                shuffled: *shuffled,
                rerun_of: rerun_of
                    .as_deref()
                    .map(|text| Arc::from(self.apply(text).as_str())),
            },
            Event::ScenarioStarted {
                scenario,
                file,
                timestamp_ms,
                worker,
                phase,
                exclusive,
            } => Event::ScenarioStarted {
                scenario: s(scenario),
                file: s(file),
                timestamp_ms: *timestamp_ms,
                worker: *worker,
                phase: phase.clone(),
                exclusive: *exclusive,
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
            Event::StepFinished { .. } => self.apply_step_finished(event),
            Event::ScenarioFinished {
                scenario,
                file,
                status,
                timestamp_ms,
                worker,
                phase,
                reason,
                tags,
            } => Event::ScenarioFinished {
                scenario: s(scenario),
                file: s(file),
                status: *status,
                timestamp_ms: *timestamp_ms,
                worker: *worker,
                phase: phase.clone(),
                reason: reason
                    .as_deref()
                    .map(|text| Arc::from(self.apply(text).as_str())),
                // Masked like `captures`: authored identifiers, but the
                // masker's contract is that no secret substring survives
                // anywhere in the stream.
                tags: tags.iter().map(|tag| self.apply(tag)).collect(),
            },
            Event::RunFinished { .. } => event.clone(),
        }
    }

    /// The `step_finished` half of [`Self::apply_event`], split out to keep
    /// the match readable: every text-bearing field masked, none exempted.
    fn apply_step_finished(&self, event: &Event) -> Event {
        let s = |text: &Arc<str>| -> Arc<str> { Arc::from(self.apply(text).as_str()) };
        let Event::StepFinished {
            scenario,
            engine,
            step,
            status,
            attempts,
            duration_ms,
            captures,
            fragment,
            detail,
            attempt_details,
            reproduce_hint,
        } = event
        else {
            unreachable!("caller matched StepFinished")
        };
        Event::StepFinished {
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
            // Masked like `captures`, though both are authored identifiers
            // rather than data: the masker's contract is that no secret
            // substring survives anywhere in the stream, and a field
            // exempted "because it can't contain one" is how that stops
            // being true later.
            fragment: fragment.as_deref().map(|text| self.apply(text)),
            detail: detail.as_deref().map(|text| self.apply(text)),
            attempt_details: attempt_details
                .iter()
                .map(|text| self.apply(text))
                .collect(),
            reproduce_hint: reproduce_hint.as_deref().map(|text| self.apply(text)),
        }
    }
}

/// Every *encoded* form of `value` a needle list can catch: the reversible
/// transforms that occur at HTTP boundaries. Forms equal to the raw value —
/// the percent-encoding of a secret with no reserved characters, the JSON
/// escape of one with nothing to escape — are filtered by the caller's dedup.
///
/// Deliberately not here: hashes (not reversible — the credential is not in
/// the record), gzip/deflate (never rendered as text), double encodings
/// (base64-of-base64 starts an unbounded tower; one level is what echo
/// endpoints produce).
fn derived_forms(value: &str) -> Vec<String> {
    use base64::Engine as _;
    use base64::engine::general_purpose as b64;

    let bytes = value.as_bytes();
    let mut forms = vec![
        b64::STANDARD.encode(bytes),
        b64::STANDARD_NO_PAD.encode(bytes),
        b64::URL_SAFE.encode(bytes),
        b64::URL_SAFE_NO_PAD.encode(bytes),
        hex(bytes, false),
        hex(bytes, true),
        percent_encode(value),
    ];
    // The JSON string escape (`"` → `\"`, `\` → `\\`, control chars → `\uXXXX`)
    // — what the secret looks like *inside* a rendered JSON body. serde_json
    // wraps in quotes; the needle is the inner text.
    if let Ok(quoted) = serde_json::to_string(value) {
        forms.push(quoted[1..quoted.len() - 1].to_owned());
    }
    forms
}

/// Hex encoding of `bytes`, in one case. Both cases are needles — encoders
/// split roughly evenly on this, unlike percent-encoding where uppercase is
/// near-universal.
fn hex(bytes: &[u8], upper: bool) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        if upper {
            let _ = write!(out, "{byte:02X}");
        } else {
            let _ = write!(out, "{byte:02x}");
        }
    }
    out
}

/// RFC 3986 percent-encoding: everything outside the unreserved set
/// (`ALPHA / DIGIT / "-" / "." / "_" / "~"`) becomes `%XX` with uppercase hex
/// — the form `encodeURIComponent`, Python's `quote`, and Go's `QueryEscape`
/// all emit. Hand-rolled rather than a dependency: it is ten lines, and the
/// crate that does it only emits one variant anyway.
///
/// Private: redaction needles are its only caller. It was public while
/// `proef-lsp` hand-rolled document-URI encoding against the same unreserved
/// set; that bridge is now `url::Url`'s own, so the sharing rationale is gone
/// with it.
fn percent_encode(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
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
        // Poison recovery: fan-out holds no cross-event invariant, and
        // dropping events after one reporter panic would truncate the run
        // record and lose its terminal event (ADR-0008 — the JSONL stream IS
        // the record).
        let mut stack = stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    })
}

/// Buffer key: `(feature file, scenario name)` — the run-wide scenario
/// identity (names are unique only within one file).
type ScenarioKey = (Arc<str>, Arc<str>);

/// How much the console says per scenario (`--console`). Pure
/// presentation — the record, the exit code, and the post-pool failure
/// details are identical in every mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConsoleMode {
    /// The buffered BDD tree — every step of every scenario (today's
    /// output, the default).
    #[default]
    Full,
    /// One glyph per scenario (`.` passed, `F` failed, `s` skipped, `w`
    /// warned — pytest/RF case convention: lowercase is non-gating),
    /// wrapped at 80, flushed per glyph. Failures still print in full
    /// after the pool.
    Dotted,
    /// The run line and the summary only.
    Quiet,
}

/// Console BDD tree, buffered per scenario — keyed by `(file, scenario)`
/// (the run-wide identity), since two same-named scenarios under `--jobs > 1`
/// must never share a buffer. The sink mutex serializes `on_event`, so a
/// dotted glyph can never interleave mid-line.
pub struct ConsoleReporter<W: Write + Send> {
    out: W,
    redactions: Redactions,
    mode: ConsoleMode,
    color: bool,
    dotted_col: usize,
    buffers: Vec<(ScenarioKey, Vec<String>)>,
}

impl<W: Write + Send> ConsoleReporter<W> {
    /// A console reporter writing to `out` in `mode`, painting the status
    /// vocabulary with ANSI color when `color` is set. The TTY/`NO_COLOR`
    /// probe is the CLI edge's call — that read is IO, which stays out of
    /// the sans-IO core — and color is paint on identical bytes, never
    /// information: the record, the log mirror (which strips it) and the
    /// exit code cannot depend on it.
    pub fn new(out: W, redactions: Redactions, mode: ConsoleMode, color: bool) -> Self {
        Self {
            out,
            redactions,
            mode,
            color,
            dotted_col: 0,
            buffers: Vec::new(),
        }
    }

    /// `text` painted in the status's color when color is on.
    fn painted(&self, status: Status, text: &str) -> String {
        if !self.color {
            return text.to_owned();
        }
        let code = match status {
            Status::Passed => "32",
            Status::Failed => "31",
            Status::Warned => "33",
            Status::Skipped => "2",
        };
        format!("\x1b[{code}m{text}\x1b[0m")
    }

    fn buffer_for(&mut self, file: &Arc<str>, scenario: &Arc<str>) -> &mut Vec<String> {
        if let Some(position) = self
            .buffers
            .iter()
            .position(|((f, name), _)| f == file && name == scenario)
        {
            &mut self.buffers[position].1
        } else {
            self.buffers
                .push(((Arc::clone(file), Arc::clone(scenario)), Vec::new()));
            &mut self
                .buffers
                .last_mut()
                .unwrap_or_else(|| unreachable!("buffer just pushed"))
                .1
        }
    }
    /// The `scenario_finished` half of the console: glyph, tree flush, or
    /// nothing, by mode.
    fn finish_scenario_line(
        &mut self,
        scenario: &Arc<str>,
        file: &Arc<str>,
        status: Status,
        reason: Option<&str>,
    ) {
        match self.mode {
            ConsoleMode::Quiet => {}
            ConsoleMode::Dotted => {
                let glyph = match status {
                    Status::Passed => '.',
                    Status::Failed => 'F',
                    Status::Skipped => 's',
                    Status::Warned => 'w',
                };
                let _ = write!(self.out, "{}", self.painted(status, &glyph.to_string()));
                self.dotted_col += 1;
                if self.dotted_col >= 80 {
                    let _ = writeln!(self.out);
                    self.dotted_col = 0;
                }
                // A dot the user cannot see is not progress.
                let _ = self.out.flush();
            }
            ConsoleMode::Full => {
                let lines = self
                    .buffers
                    .iter()
                    .position(|((f, name), _)| f == file && name == scenario)
                    .map(|position| self.buffers.remove(position).1)
                    .unwrap_or_default();
                for line in lines {
                    let _ = writeln!(self.out, "{line}");
                }
                let why = reason
                    .map(|reason| format!(" — {reason}"))
                    .unwrap_or_default();
                let _ = writeln!(
                    self.out,
                    "    {} scenario {scenario}{why}",
                    self.painted(status, glyph(status))
                );
            }
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
            Event::ScenarioStarted { scenario, file, .. } => {
                if self.mode == ConsoleMode::Full {
                    let header = format!("\n  Scenario: {scenario} ({file})");
                    self.buffer_for(file, scenario).push(header);
                }
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
                if self.mode != ConsoleMode::Full {
                    return;
                }
                let attempts_note = if *attempts > 1 {
                    format!(", {attempts} attempts")
                } else {
                    String::new()
                };
                let line = format!(
                    "    {} {}:{} — {} ({duration_ms}ms{attempts_note})",
                    self.painted(*status, glyph(*status)),
                    step.file,
                    step.line,
                    step.text
                );
                let line = self.redactions.apply(&line);
                // A warning or a skip with no reason is unusable — say
                // why. (Failures get the richer end-of-run list instead.)
                let warn_detail = matches!(status, Status::Warned | Status::Skipped)
                    .then_some(detail.as_deref())
                    .flatten()
                    .map(|d| self.redactions.apply(&format!("      ↳ {d}")));
                let buffer = self.buffer_for(&step.file, scenario);
                buffer.push(line);
                if let Some(warn_detail) = warn_detail {
                    buffer.push(warn_detail);
                }
            }
            Event::ScenarioFinished {
                scenario,
                file,
                status,
                reason,
                ..
            } => {
                self.finish_scenario_line(scenario, file, *status, reason.as_deref());
            }
            Event::RunFinished {
                passed,
                failed,
                skipped,
                cancelled,
            } => {
                if self.mode == ConsoleMode::Dotted && self.dotted_col > 0 {
                    let _ = writeln!(self.out);
                    self.dotted_col = 0;
                }
                let note = if *cancelled { " · cancelled" } else { "" };
                // Paint the half of the verdict that matters: the failure
                // count in red when anything failed, the pass count in green
                // on a clean run — never both, one signal for the eye.
                let (passed_text, failed_text) = if *failed > 0 {
                    (
                        format!("{passed} passed"),
                        self.painted(Status::Failed, &format!("{failed} failed")),
                    )
                } else {
                    (
                        self.painted(Status::Passed, &format!("{passed} passed")),
                        format!("{failed} failed"),
                    )
                };
                let _ = writeln!(
                    self.out,
                    "\nsummary: {passed_text} · {failed_text} · {skipped} skipped{note}"
                );
                let _ = self.out.flush();
            }
        }
    }
}

/// Run totals derived from the event stream — the `Summarize` leg of the
/// decorator stack (ADR-0008): leaves (GitHub summary, `--format json`, …)
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
                env: None,
                metadata: std::collections::BTreeMap::new(),
                shuffled: false,
                rerun_of: None,
            },
            Event::ScenarioStarted {
                scenario: Arc::from("S"),
                file: Arc::from("f.feature"),
                timestamp_ms: None,
                worker: None,
                phase: None,
                exclusive: false,
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
                fragment: None,
                detail: None,
                attempt_details: Vec::new(),
                reproduce_hint: None,
            },
            Event::ScenarioFinished {
                scenario: Arc::from("S"),
                file: Arc::from("f.feature"),
                status: Status::Passed,
                timestamp_ms: None,
                worker: None,
                phase: None,
                reason: None,
                tags: Vec::new(),
            },
            Event::RunFinished {
                passed: 1,
                failed: 0,
                skipped: 0,
                cancelled: false,
            },
        ]
    }

    /// Color is paint on identical content: with color on, the status
    /// vocabulary wraps in ANSI; with color off, not one escape byte is
    /// emitted — the form the record, the log mirror and every text
    /// assertion in this suite rely on.
    #[test]
    fn color_wraps_the_status_vocabulary_and_off_means_no_escapes() {
        let render = |color: bool| {
            let mut out: Vec<u8> = Vec::new();
            let mut console =
                ConsoleReporter::new(&mut out, Redactions::default(), ConsoleMode::Full, color);
            console.on_event(&Event::ScenarioFinished {
                scenario: Arc::from("S"),
                file: Arc::from("a.feature"),
                status: Status::Failed,
                timestamp_ms: None,
                worker: None,
                phase: None,
                reason: None,
                tags: Vec::new(),
            });
            console.on_event(&Event::RunFinished {
                passed: 0,
                failed: 1,
                skipped: 0,
                cancelled: false,
            });
            String::from_utf8(out).unwrap()
        };
        let painted = render(true);
        assert!(painted.contains("\x1b[31m✗\x1b[0m"), "{painted}");
        assert!(painted.contains("\x1b[31m1 failed\x1b[0m"), "{painted}");
        let plain = render(false);
        assert!(!plain.contains('\x1b'), "{plain}");
        assert!(plain.contains("summary: 0 passed · 1 failed"), "{plain}");
    }

    #[test]
    fn console_buffers_per_scenario_and_prints_on_finish() {
        let mut out = Vec::new();
        {
            let mut console =
                ConsoleReporter::new(&mut out, Redactions::default(), ConsoleMode::Full, false);
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

            /// The invariant over the *encoded* forms (S1): a server that
            /// reflects a secret base64-, hex-, percent- or JSON-escape-encoded
            /// puts a string trivially decodable back to the credential into an
            /// assert detail, and the raw needle never fires on it — measured
            /// live before this existed, with the base64 form reaching the
            /// console and `events.jsonl`.
            ///
            /// The generator includes `+`, `/`, spaces, quotes and backslashes
            /// so the standard/URL-safe base64 alphabets actually diverge and
            /// the percent- and JSON-escape forms actually differ from the raw
            /// value — an alphanumeric-only secret would make three of the
            /// seven assertions vacuously equal to the raw-needle case.
            #[test]
            fn redaction_removes_encoded_forms(
                secret in "[a-zA-Z0-9+/ \"\\\\-]{6,24}",
                prefix in "[a-z]{0,10}",
                suffix in "[a-z]{0,10}",
            ) {
                let redactions = Redactions::new([secret.clone()]);
                // The production list itself, not a hand-copy of it: a form
                // added to `derived_forms` later is then tested the moment it
                // exists, instead of silently passing on a subset. Oracle
                // independence lives elsewhere — the end-to-end test pins the
                // exact base64 literal a real server reflects.
                for form in super::super::derived_forms(&secret) {
                    let rendered = format!("{prefix}{form}{suffix}");
                    let redacted = redactions.apply(&rendered);
                    prop_assert!(
                        !redacted.contains(form.as_str()),
                        "encoded form survived redaction: {form}"
                    );
                }
            }

            /// Ordinary text is left alone: a needle list derived from one
            /// secret must not eat an unrelated rendering. (Over-redaction is
            /// the accepted failure direction, but only on real matches —
            /// this pins that derivation introduces no wildcards.)
            #[test]
            fn text_free_of_the_secret_and_its_forms_is_untouched(
                secret in "[a-zA-Z0-9]{16,24}",
                text in "[ -~]{0,60}",
            ) {
                let redactions = Redactions::new([secret.clone()]);
                let redacted = redactions.apply(&text);
                if redacted != text {
                    // The only permitted reason for a change is that some
                    // needle genuinely occurred in the input.
                    prop_assert!(redacted.contains("***"));
                }
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
                    timestamp_ms: None,
                    worker: None,
                    phase: None,
                    exclusive: false,
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
                    attempts: 2,
                    duration_ms: 1,
                    captures: vec![format!("cap-{secret}")],
                    fragment: Some(format!("{secret}.hurl#admin.search")),
                    detail: Some(format!("boom {secret}")),
                    reproduce_hint: None,
                    attempt_details: vec![format!("earlier boom {secret}")],
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
                        ConsoleMode::Full,
                        false,
                    );
                    console.on_event(&Event::ScenarioStarted {
                        scenario: Arc::from("S"),
                        file: Arc::from("f"),
                        timestamp_ms: None,
                        worker: None,
                        phase: None,
                        exclusive: false,
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
                        fragment: None,
                        detail: None,
                        attempt_details: Vec::new(),
                    reproduce_hint: None,
                    });
                    console.on_event(&Event::ScenarioFinished {
                        scenario: Arc::from("S"),
                        file: Arc::from("f"),
                        status: Status::Failed,
                        timestamp_ms: None,
                        worker: None,
                        phase: None,
            reason: None,
            tags: Vec::new(),
                    });
                }
                let text = String::from_utf8(out).unwrap();
                prop_assert!(!text.contains(&secret), "{text}");
            }
        }
    }
}
