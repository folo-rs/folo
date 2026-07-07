//! Warmup-robust per-iteration processor-time estimate exposed to callers.
//!
//! The estimator itself — a through-origin, iterations²-weighted OLS slope and a
//! closed-form heteroscedasticity-robust confidence interval of that slope —
//! lives in [`folo_utils::SpanAccumulator`], shared with the other tracker
//! crates and folded as each span is measured. This module only defines the
//! nanosecond-typed view of those figures that
//! [`ReportOperation::statistics`](crate::ReportOperation::statistics) exposes,
//! plus a small rendering helper.

use std::time::Duration;

/// Per-iteration processor-time statistics for one operation.
///
/// [`slope_nanos`](Self::slope_nanos) is the per-iteration processor time and
/// [`interval_nanos`](Self::interval_nanos) its 95% confidence bounds (`None`
/// when there is not enough data to estimate them). Exposed through
/// [`ReportOperation::statistics`](crate::ReportOperation::statistics) so callers
/// can consume the same figures the JSON output records.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct OperationStatistics {
    /// Number of spans recorded (distinct from the total iteration count).
    pub span_count: u64,

    /// The per-iteration processor time, in nanoseconds.
    pub slope_nanos: f64,

    /// Confidence interval `(low, high)` for [`slope_nanos`](Self::slope_nanos),
    /// or `None` when it cannot be estimated.
    pub interval_nanos: Option<(f64, f64)>,
}

/// Converts a per-iteration processor-time figure in nanoseconds to a `Duration`
/// for human-readable rendering.
///
/// Tiny negative fits — possible from the interval's lower bound on near-zero
/// data — are clamped to zero so the formatter never panics on a negative input.
pub(crate) fn nanos_to_duration(nanos: f64) -> Duration {
    Duration::from_secs_f64(nanos.max(0.0) / 1_000_000_000.0)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn nanos_to_duration_clamps_negatives_to_zero() {
        // A tiny negative fit (possible from the interval's lower bound on
        // near-zero data) is clamped so the formatter never panics, while a
        // positive figure converts faithfully.
        assert_eq!(nanos_to_duration(-5.0), Duration::ZERO);
        assert_eq!(nanos_to_duration(1_000_000_000.0), Duration::from_secs(1));
    }
}
