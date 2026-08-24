//! The serde event spine (ADR-0008).
//!
//! One versioned, serde-able [`Event`] enum is the single source of truth for live
//! progress *and* persistence: the JSONL run record **is** the appended event stream —
//! there is no second record format. Changes are **additive-only**; the stream head
//! ([`Event::RunStarted`]) declares [`EVENT_SCHEMA_VERSION`].
//!
//! Secret values never enter events — captures are reported by *name* only
//! (redaction invariant, ADR-0005).

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::step::{Status, StepRef};

/// Version of the event schema, declared once per stream in [`Event::RunStarted`].
pub const EVENT_SCHEMA_VERSION: u32 = 1;

/// One event in a run's stream. Serialized as JSONL, tagged by `event`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The run began. Head of every stream; declares the schema version.
    RunStarted {
        /// Event schema version ([`EVENT_SCHEMA_VERSION`]).
        schema: u32,
        /// Injected run identifier (uuid-v7-derived; core never generates it).
        run_id: Arc<str>,
        /// The active `--env`/`PROEF_ENV` profile name (ADR-0020): user-
        /// chosen input, not a harvested machine fact — without it two
        /// records of one suite against different `[env.<name>]` merges
        /// read as regressions in `diff`. Additive; absent = no profile.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<Arc<str>>,
        /// Explicit user-supplied run metadata (`--meta k=v`, `[meta]`,
        /// `[env.<name>.meta]`), redacted at the sink boundary like every
        /// text field. proef never harvests: no git, no hostname, no CI
        /// env sniffing (ADR-0020; R12-1's boundary). Additive; empty is
        /// unserialized.
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        metadata: std::collections::BTreeMap<String, String>,
        /// The execution order was re-dealt (`--shuffle`); the permutation
        /// is seeded by `run_id`, so this bool plus the id reproduces the
        /// order exactly. Additive; absent = authored order.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        shuffled: bool,
    },
    /// A scenario began executing.
    ScenarioStarted {
        /// Scenario name as authored.
        scenario: Arc<str>,
        /// Feature file the scenario comes from.
        file: Arc<str>,
        /// Milliseconds since the run began — injected at the CLI sink (the
        /// sans-IO core leaves it `None`, like `run_id`). Absent on records
        /// without timing; additive (ADR-0008, ADR-0015).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp_ms: Option<u64>,
        /// 0-based worker index this scenario ran on — injected at the sink.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worker: Option<u64>,
        /// The `[run]` lifecycle phase this scenario belongs to (`"setup"` /
        /// `"teardown"`), absent for an ordinary suite scenario.
        ///
        /// Without it a phase scenario is indistinguishable from a suite one
        /// except by feature path, so every consumer had to re-derive phase
        /// membership from `proef.toml` — and `explain`, `--rerun` and `diff`
        /// each got it wrong in a different way. The record says so itself now.
        /// Additive and optional (ADR-0008): records that predate the field
        /// read as "no phase", which is what they were.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<Arc<str>>,
        /// The scenario ran with the pool to itself (`[run]
        /// exclusive-tags`, ADR-0007 scheduling) — recorded so a timeline
        /// post-mortem can answer "why did the pool drain before this
        /// started" from the record alone (R11-6). Additive; absent =
        /// false, which is what every pre-field record meant.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        exclusive: bool,
    },
    /// A batch of contiguous same-engine steps was dispatched.
    BatchStarted {
        /// Scenario name as authored.
        scenario: Arc<str>,
        /// Engine executing the batch.
        engine: Arc<str>,
        /// Number of steps in the batch.
        steps: usize,
    },
    /// An artifact entry began an execution attempt — the engine's live
    /// progress signal (ADR-0001's `EventListener`, surfaced on the spine).
    /// Additive schema variant: absent from pre-existing streams.
    EntryRunning {
        /// Scenario name as authored.
        scenario: Arc<str>,
        /// Engine executing the entry.
        engine: Arc<str>,
        /// 0-based entry ordinal within the scenario's artifact.
        entry: usize,
        /// Retry number of this attempt (`0` = first attempt).
        retry: u32,
    },
    /// A step finished (in success or failure).
    StepFinished {
        /// Scenario name as authored.
        scenario: Arc<str>,
        /// Engine that executed the step.
        engine: Arc<str>,
        /// Anchor to the authored feature line.
        step: StepRef,
        /// Outcome status.
        status: Status,
        /// Number of attempts made.
        attempts: u32,
        /// Wall-clock duration in milliseconds.
        duration_ms: u64,
        /// Names (never values) of captures produced by this step.
        captures: Vec<String>,
        /// The fragment that supplied this step's request, as `file.hurl#name`
        /// (ADR-0018). Additive schema field: absent for an inline `hurl:`
        /// block, which is every step in every stream written before
        /// fragments existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fragment: Option<String>,
        /// Failure detail, when the step failed (additive schema field —
        /// absent on passing steps, so pre-existing streams are unchanged).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        /// Messages from earlier, failed attempts of a step that ultimately
        /// passed — the flaky-failure detail (`JUnit` `<flakyFailure>`).
        /// Additive schema field: empty (and unserialized) for the common
        /// single-attempt step, so pre-existing streams are unchanged.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attempt_details: Vec<String>,
        /// The redacted curl of the failing request (R18 wave-1). The engine
        /// always computed it, the console always printed it — and the record
        /// dropped it, so `explain` and the HTML report could never show the
        /// one artifact an API post-mortem reaches for first. Additive:
        /// absent on passing steps and on every pre-field stream.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reproduce_hint: Option<String>,
    },
    /// A scenario finished.
    ScenarioFinished {
        /// Scenario name as authored.
        scenario: Arc<str>,
        /// Feature file path — together with `scenario`, the run-wide
        /// identity (scenario names are unique only within one file).
        /// Defaults empty when replaying records that predate the field
        /// (schema 1 is additive-only — ADR-0008).
        #[serde(default = "unknown_file")]
        file: Arc<str>,
        /// Aggregate scenario status.
        status: Status,
        /// Milliseconds since the run began — injected at the CLI sink (the
        /// sans-IO core leaves it `None`, like `run_id`). Absent on records
        /// without timing; additive (ADR-0008, ADR-0015).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp_ms: Option<u64>,
        /// 0-based worker index this scenario ran on — injected at the sink.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worker: Option<u64>,
        /// The `[run]` lifecycle phase this scenario belongs to (`"setup"` /
        /// `"teardown"`), absent for an ordinary suite scenario.
        ///
        /// Without it a phase scenario is indistinguishable from a suite one
        /// except by feature path, so every consumer had to re-derive phase
        /// membership from `proef.toml` — and `explain`, `--rerun` and `diff`
        /// each got it wrong in a different way. The record says so itself now.
        /// Additive and optional (ADR-0008): records that predate the field
        /// read as "no phase", which is what they were.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<Arc<str>>,
        /// Why the scenario is `Skipped` (R18 wave-2): an authored skip
        /// carries the pasteable tag spelling (`@skip` / `@skip:reason`),
        /// which always begins with `@`; a mechanical skip (cancellation)
        /// carries proef-fixed prose, which never does — `--rerun` keys its
        /// re-queue decision on exactly that split. Additive and absent for
        /// every non-skipped scenario and every pre-field stream.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<Arc<str>>,
        /// The scenario's accumulated tags (feature → rule → scenario →
        /// examples, deduped, authored order, `@` stripped) — on the
        /// *finished* event because the cancel-skip path emits no
        /// `scenario_started`, and per-tag skip counts are a column the
        /// rollup needs. Additive; empty (and unserialized) for untagged
        /// scenarios and every pre-field stream.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tags: Vec<String>,
    },
    /// The run finished. Tail of every stream.
    RunFinished {
        /// Scenarios that passed.
        passed: usize,
        /// Scenarios that failed.
        failed: usize,
        /// Scenarios that were skipped.
        skipped: usize,
        /// The run was cancelled before completing (additive schema field —
        /// absent means `false`, and `false` is not serialized, so pre-existing
        /// streams and snapshots are unchanged).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        cancelled: bool,
    },
}

/// Serde default for records that predate the `file` field on
/// [`Event::ScenarioFinished`].
fn unknown_file() -> Arc<str> {
    Arc::from("")
}

/// Fan-out point for [`Event`]s — reporters subscribe here (borrowed
/// events). Cheap to clone; threads share the same sink.
#[derive(Clone)]
pub struct EventSink(Arc<dyn Fn(&Event) + Send + Sync>);

impl EventSink {
    /// A sink invoking `f` for every emitted event.
    pub fn new(f: impl Fn(&Event) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// A sink that discards every event (tests, `--dry-run`).
    pub fn null() -> Self {
        Self(Arc::new(|_| {}))
    }

    /// Emit one event to all consumers.
    pub fn emit(&self, event: &Event) {
        (self.0)(event);
    }
}

impl fmt::Debug for EventSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EventSink(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire shape is a compatibility surface (ADR-0008): pin the JSON of the
    /// stream head exactly. Additive changes only.
    #[test]
    fn run_started_wire_shape_is_stable() {
        let event = Event::RunStarted {
            schema: EVENT_SCHEMA_VERSION,
            run_id: Arc::from("run-0001"),
            env: None,
            metadata: std::collections::BTreeMap::new(),
            shuffled: false,
        };
        let json = serde_json::to_string(&event).unwrap_or_default();
        assert_eq!(
            json,
            r#"{"event":"run_started","schema":1,"run_id":"run-0001"}"#
        );
    }

    #[test]
    fn events_round_trip_through_jsonl() {
        let event = Event::StepFinished {
            scenario: Arc::from("Search finds a record"),
            engine: Arc::from("http"),
            step: StepRef {
                file: Arc::from("tests/features/501_search.feature"),
                line: 12,
                text: Arc::from("the admin searches for \"Jansen\""),
            },
            status: Status::Passed,
            attempts: 2,
            duration_ms: 42,
            captures: vec!["recordId".to_owned()],
            fragment: Some("tests/hurl/admin.hurl#admin.search".to_owned()),
            detail: None,
            attempt_details: vec!["attempt 1: HTTP 404 (retried)".to_owned()],
            reproduce_hint: Some("curl -X POST http://api.invalid/records".to_owned()),
        };
        let json = serde_json::to_string(&event).unwrap_or_default();
        let back: Event = serde_json::from_str(&json).unwrap_or(Event::RunFinished {
            passed: 0,
            failed: 0,
            skipped: 0,
            cancelled: false,
        });
        assert_eq!(back, event);
    }

    #[test]
    fn sink_fans_out_borrowed_events() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        let sink = EventSink::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let clone = sink.clone();
        clone.emit(&Event::RunFinished {
            passed: 1,
            failed: 0,
            skipped: 0,
            cancelled: false,
        });
        sink.emit(&Event::RunFinished {
            passed: 1,
            failed: 0,
            skipped: 0,
            cancelled: false,
        });
        assert_eq!(seen.load(Ordering::SeqCst), 2);
    }
}
