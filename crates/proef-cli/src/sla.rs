//! Run-level SLA gate (opt-in): an absolute latency budget over a finished
//! run's per-step durations.
//!
//! Deliberately distinct from hurl's per-request `duration < …` assert (which
//! fails a single step): the SLA gate is an *aggregate* budget over the whole
//! run, configured in `proef.toml` `[sla]` (env-overridable) and **off by
//! default**. With no `[sla]` table the gate is inert, so a run without SLA
//! config is byte-identical to before and the pinned exit-0 corpus is
//! untouched. A breach folds into the exit code as a test failure (exit 1) —
//! never a new exit code, never masking a `User`/`System` fault.

use std::fmt::Write as _;

use proef_core::runner::RunSummary;
use proef_core::step::Status;

/// Resolved SLA thresholds: base `[sla]` merged under the active
/// `[env.<name>.sla]`, field by field. All-`None` is the opt-out default (no
/// gate).
#[derive(Debug, Clone, Copy, Default)]
pub struct SlaThresholds {
    /// 95th-percentile per-step duration ceiling, in milliseconds.
    pub p95_ms: Option<u64>,
    /// Maximum single-step duration ceiling, in milliseconds.
    pub max_ms: Option<u64>,
}

impl SlaThresholds {
    /// Whether any threshold is set. `false` — the default when `proef.toml`
    /// declares no `[sla]` — means the gate is entirely inert.
    pub fn is_set(self) -> bool {
        self.p95_ms.is_some() || self.max_ms.is_some()
    }
}

/// Nearest-rank percentile of an ascending-sorted slice (`p` in `1..=100`), or
/// `None` for an empty slice. All arithmetic stays in `usize` (step counts are
/// small) to avoid a lossy index cast.
fn percentile(sorted_ms: &[u64], p: usize) -> Option<u64> {
    let n = sorted_ms.len();
    if n == 0 {
        return None;
    }
    let rank = (p * n).div_ceil(100).max(1); // 1-based nearest rank
    sorted_ms.get((rank - 1).min(n - 1)).copied()
}

/// Evaluate a finished run against its SLA budget. Returns a human-readable
/// breach report (for stderr) when a ceiling is exceeded, or `None` when there
/// is no gate or the run stayed within budget.
///
/// Population: per-step wall-clock duration for steps that issued a request.
/// `Skipped` steps never reach the network, so they are excluded rather than
/// skewing the percentile toward zero. Step file/line are authored feature
/// prose, never secret values, so the report needs no redaction.
pub fn check(summary: &RunSummary, thresholds: SlaThresholds) -> Option<String> {
    if !thresholds.is_set() {
        return None;
    }
    let steps: Vec<(u64, &str, usize)> = summary
        .outcomes
        .iter()
        .flat_map(|outcome| &outcome.steps)
        .filter(|step| step.status != Status::Skipped)
        .map(|step| {
            let ms = u64::try_from(step.duration.as_millis()).unwrap_or(u64::MAX);
            (ms, step.step.file.as_ref(), step.step.line)
        })
        .collect();
    evaluate(steps, thresholds)
}

/// The pure core of [`check`], over pre-extracted `(duration_ms, file, line)`
/// rows — kept separate so the percentile/breach logic is unit-testable without
/// building a whole [`RunSummary`].
fn evaluate(mut steps: Vec<(u64, &str, usize)>, thresholds: SlaThresholds) -> Option<String> {
    if steps.is_empty() {
        return None;
    }
    steps.sort_unstable_by_key(|&(ms, _, _)| ms);
    let durations: Vec<u64> = steps.iter().map(|&(ms, _, _)| ms).collect();

    let mut breaches: Vec<String> = Vec::new();
    if let (Some(budget), Some(measured)) = (thresholds.p95_ms, percentile(&durations, 95))
        && measured > budget
    {
        breaches.push(format!("p95 duration {measured}ms > {budget}ms budget"));
    }
    if let (Some(budget), Some(&measured)) = (thresholds.max_ms, durations.last())
        && measured > budget
    {
        breaches.push(format!("max duration {measured}ms > {budget}ms budget"));
    }
    if breaches.is_empty() {
        return None;
    }

    let mut report = String::new();
    let _ = writeln!(report, "SLA breach ({} step(s) measured):", durations.len());
    for breach in &breaches {
        let _ = writeln!(report, "  {breach}");
    }
    let _ = writeln!(report, "  slowest:");
    for &(ms, file, line) in steps.iter().rev().take(3) {
        let _ = writeln!(report, "    {ms}ms  {file}:{line}");
    }
    report.truncate(report.trim_end().len());
    Some(report)
}

#[cfg(test)]
mod tests {
    // Why: unwrap/expect are acceptable in `#[cfg(test)]` — a broken assumption
    // surfaces as a test failure, which is exactly the intent.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn percentile_nearest_rank() {
        assert_eq!(percentile(&[], 95), None);
        assert_eq!(percentile(&[10], 95), Some(10));
        // 1..=100: rank = ceil(95*100/100) = 95 → index 94 → value 95.
        let hundred: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&hundred, 95), Some(95));
        // [10,20,30,40] p50: rank = ceil(50*4/100) = 2 → index 1 → 20.
        assert_eq!(percentile(&[10, 20, 30, 40], 50), Some(20));
    }

    fn rows(ms: &[u64]) -> Vec<(u64, &'static str, usize)> {
        ms.iter().map(|&m| (m, "f.feature", 1)).collect()
    }

    #[test]
    fn no_thresholds_is_inert() {
        assert!(evaluate(rows(&[10, 999]), SlaThresholds::default()).is_none());
    }

    #[test]
    fn within_budget_reports_nothing() {
        let t = SlaThresholds {
            p95_ms: Some(1000),
            max_ms: Some(1000),
        };
        assert!(evaluate(rows(&[10, 20, 30]), t).is_none());
    }

    #[test]
    fn p95_breach_is_reported() {
        let t = SlaThresholds {
            p95_ms: Some(50),
            max_ms: None,
        };
        // [10,20,30,100] p95 → 100 > 50.
        let report = evaluate(rows(&[10, 20, 30, 100]), t).expect("breach");
        assert!(report.contains("p95 duration 100ms > 50ms"), "{report}");
        assert!(report.contains("slowest"), "{report}");
    }

    #[test]
    fn max_breach_is_reported_independently() {
        let t = SlaThresholds {
            p95_ms: None,
            max_ms: Some(50),
        };
        let report = evaluate(rows(&[10, 20, 100]), t).expect("breach");
        assert!(report.contains("max duration 100ms > 50ms"), "{report}");
    }

    #[test]
    fn empty_run_never_breaches() {
        let t = SlaThresholds {
            p95_ms: Some(1),
            max_ms: Some(1),
        };
        assert!(evaluate(Vec::new(), t).is_none());
    }
}
