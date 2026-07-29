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
//!
//! **Parallelism caveat**: each Trial is its own `proef` process, and the
//! persistent World (`.proef-state.json`) has no cross-process lock — a suite
//! using `saveAs: global` across scenarios is last-writer-wins under parallel
//! trials. Run such suites with `--test-threads=1` (or nextest
//! `test-threads = 1`); per-scenario process isolation is the design,
//! cross-trial ordering is deliberately undefined.
