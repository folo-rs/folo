//! Dispersion statistics derived from an operation's recorded spans.
//!
//! Each measured span contributes one `(iterations, total_nanos)` observation.
//! Because a span corresponds to one Criterion sample (one `iter_custom`
//! closure call), the spans of an operation form the same per-sample population
//! Criterion analyzes for its own wall-clock estimates. The warmup-robust
//! estimator itself — an iterations²-weighted through-origin OLS slope and a
//! bootstrap confidence interval of that slope — lives in
//! [`folo_utils::SpanStats`], shared with the other tracker crates. This module
//! only maps this crate's nanosecond-typed [`SpanRecord`] onto that estimator.
//!
//! All work here runs once, when a [`Report`](crate::Report) is materialized for
//! output — never inside a measured span.

use std::time::Duration;

use folo_utils::{Span, SpanStats};

/// One recorded span: the processor time a single measured span consumed and the
/// number of iterations it covered.
///
/// The raw whole-span total is retained (not pre-divided per iteration) so that
/// the slope regression can weight each span by its iteration count.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SpanRecord {
    /// Iterations the span measured.
    pub(crate) iterations: u64,

    /// Total processor time the span consumed, in nanoseconds.
    pub(crate) total_nanos: u64,
}

/// Dispersion statistics for one operation, derived from its spans.
///
/// Every value is a per-iteration processor time in nanoseconds. Processor time
/// is not deterministic — it jitters run to run with system load and scheduling,
/// and the raw pooled mean silently folds warmup and one-off costs into the
/// per-iteration figure — so the point estimate is a warmup-robust through-origin
/// slope and the interval is a bootstrap confidence interval of that slope. When
/// every span recorded the same per-iteration value the interval collapses onto
/// the point estimate. Exposed through
/// [`ReportOperation::statistics`](crate::ReportOperation::statistics) so callers
/// can consume the same dispersion the JSON output records.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct OperationStatistics {
    /// Number of spans recorded (distinct from the total iteration count).
    pub span_count: u64,

    /// Through-origin OLS slope: the per-iteration processor time point estimate.
    pub slope_nanos: f64,

    /// Sample standard deviation of the per-iteration values across spans.
    pub std_dev_nanos: f64,

    /// Lower bound of the slope's bootstrap confidence interval.
    pub interval_low_nanos: f64,

    /// Upper bound of the slope's bootstrap confidence interval.
    pub interval_high_nanos: f64,

    /// Smallest per-iteration value observed across spans.
    pub min_nanos: f64,

    /// Largest per-iteration value observed across spans.
    pub max_nanos: f64,
}

/// Computes the dispersion statistics for a non-empty set of spans.
///
/// Returns `None` when `spans` is empty, since an operation with no measured
/// work has no statistics to report. Delegates the estimation to the shared
/// [`folo_utils::SpanStats`], then re-labels the unit-agnostic figures as
/// nanoseconds.
pub(crate) fn compute_statistics(spans: &[SpanRecord]) -> Option<OperationStatistics> {
    let owned: Vec<Span> = spans
        .iter()
        .map(|span| Span::new(span.iterations, span.total_nanos))
        .collect();
    let stats = SpanStats::from_spans(&owned)?;
    Some(OperationStatistics {
        span_count: stats.span_count,
        slope_nanos: stats.slope,
        std_dev_nanos: stats.std_dev,
        interval_low_nanos: stats.interval_low,
        interval_high_nanos: stats.interval_high,
        min_nanos: stats.min,
        max_nanos: stats.max,
    })
}

/// Converts a per-iteration processor-time figure in nanoseconds to a `Duration`
/// for human-readable rendering.
///
/// Tiny negative fits — possible from the bootstrap on near-zero data — are
/// clamped to zero so the formatter never panics on a negative input.
pub(crate) fn nanos_to_duration(nanos: f64) -> Duration {
    Duration::from_secs_f64(nanos.max(0.0) / 1_000_000_000.0)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "statistics are exact integer-derived values in these fixtures"
    )]

    use super::*;

    fn span(iterations: u64, total_nanos: u64) -> SpanRecord {
        SpanRecord {
            iterations,
            total_nanos,
        }
    }

    #[test]
    fn empty_spans_have_no_statistics() {
        assert!(compute_statistics(&[]).is_none());
    }

    #[test]
    fn maps_every_field_from_the_shared_estimator() {
        // Per-iteration values 10, 20, 30 (constant single iterations): the slope
        // degenerates to their mean 20, the standard deviation to 10, and the
        // extremes to 10 and 30. This confirms the nanosecond re-labelling wires
        // each shared field to the matching output.
        let spans = [span(1, 10), span(1, 20), span(1, 30)];
        let stats = compute_statistics(&spans).unwrap();
        assert_eq!(stats.span_count, 3);
        assert_eq!(stats.slope_nanos, 20.0);
        assert_eq!(stats.std_dev_nanos, 10.0);
        assert_eq!(stats.min_nanos, 10.0);
        assert_eq!(stats.max_nanos, 30.0);
        assert!(
            stats.interval_low_nanos <= stats.slope_nanos
                && stats.slope_nanos <= stats.interval_high_nanos,
            "point {} must lie within [{}, {}]",
            stats.slope_nanos,
            stats.interval_low_nanos,
            stats.interval_high_nanos
        );
    }

    #[test]
    fn slope_weights_spans_by_iteration_count() {
        // Two spans on a perfectly linear series (5 ns/iter): the slope recovers
        // exactly 5 regardless of the differing iteration counts, so the
        // weighting survives the mapping.
        let spans = [span(2, 10), span(8, 40)];
        let stats = compute_statistics(&spans).unwrap();
        assert_eq!(stats.slope_nanos, 5.0);
    }

    #[test]
    fn single_span_interval_collapses_to_the_point_estimate() {
        let stats = compute_statistics(&[span(4, 80)]).unwrap();
        assert_eq!(stats.slope_nanos, 20.0);
        assert_eq!(stats.interval_low_nanos, 20.0);
        assert_eq!(stats.interval_high_nanos, 20.0);
    }
}
