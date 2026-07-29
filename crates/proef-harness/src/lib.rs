//! The proef nextest/IDE harness (ADR-0008, US-12).
//!
//! The `scenarios` test target (`harness = false`, libtest-mimic) exposes one
//! `Trial` per proef scenario, so `cargo nextest run -p proef-harness` and IDE
//! test UIs drive suites with zero custom protocol work.
//!
//! Configuration (environment):
//! - `PROEF_HARNESS_SUITE` — the feature file/dir to expose (unset = no
//!   trials; keeps plain workspace test runs green without a fixture).
//! - `PROEF_BIN` — path to the `proef` binary (default: `proef` on PATH).
//! - plus whatever the suite itself needs (`PROEF_BASE_URL`,
//!   `PROEF_SECRET_*`, …).
