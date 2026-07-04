//! Per-operation processor-time metrics, folded into streaming statistics.
//!
//! Every measured span contributes its whole-span processor-time total together
//! with the iteration count it covered. The spans are not retained: each is
//! folded on arrival into running totals (for the pooled mean) and into a
//! [`SpanAccumulator`] (for the warmup-robust per-iteration slope and its
//! confidence interval). Processor time is not deterministic — first-run costs
//! and scheduling jitter vary around the mean over a Criterion-chosen iteration
//! count — so the slope down-weights low-iteration warmup spans and the interval
//! quantifies the residual noise.

use std::time::Duration;

use folo_utils::SpanAccumulator;

use crate::OperationStatistics;

/// Metrics tracked for each operation in the session.
///
/// Holds the pooled totals (processor-time nanoseconds, iterations) and a shared
/// [`SpanAccumulator`] over per-iteration processor time, folded in as each span
/// is recorded. No per-span data is retained.
#[derive(Clone, Debug, Default)]
pub(crate) struct OperationMetrics {
    total_iterations: u64,
    total_nanos: u128,
    nanos: SpanAccumulator,
}

impl OperationMetrics {
    /// Records one span covering `iterations` iterations that consumed
    /// `total_nanos` nanoseconds of processor time in total.
    ///
    /// Folding a span is a handful of additions with no allocation, so it is
    /// cheap enough to run inside a measured span.
    pub(crate) fn add_span(&mut self, iterations: u64, total_nanos: u64) {
        self.total_iterations = self
            .total_iterations
            .checked_add(iterations)
            .expect("total iterations overflows u64 - this indicates an unrealistic scenario");
        self.total_nanos = self
            .total_nanos
            .checked_add(u128::from(total_nanos))
            .expect("total processor time overflows u128 - this indicates an unrealistic scenario");

        self.nanos.add(iterations, total_nanos);
    }

    /// Records one span by its per-iteration duration and iteration count.
    ///
    /// A convenience over [`add_span`](Self::add_span) used where a per-iteration
    /// duration is already known; the whole-span total is reconstituted by
    /// multiplying back out.
    #[cfg(test)]
    pub(crate) fn add_iterations(&mut self, duration: Duration, iterations: u64) {
        let per_iteration = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        let total_nanos = per_iteration.saturating_mul(iterations);
        self.add_span(iterations, total_nanos);
    }

    /// Number of spans recorded (distinct from the total iteration count).
    #[cfg(test)]
    pub(crate) fn span_count(&self) -> u64 {
        self.nanos.span_count()
    }

    /// Total iterations across all recorded spans.
    pub(crate) fn total_iterations(&self) -> u64 {
        self.total_iterations
    }

    /// Total processor time across all recorded spans.
    pub(crate) fn total_processor_time(&self) -> Duration {
        Duration::from_nanos_u128(self.total_nanos)
    }

    /// Mean processor time per iteration across all recorded spans.
    ///
    /// Returns zero when no iterations were recorded.
    pub(crate) fn mean(&self) -> Duration {
        let mean_nanos = self
            .total_nanos
            .checked_div(u128::from(self.total_iterations))
            .unwrap_or(0);
        Duration::from_nanos_u128(mean_nanos)
    }

    /// Whether the operation recorded any measurable work.
    pub(crate) fn is_empty(&self) -> bool {
        self.total_iterations == 0
    }

    /// The warmup-robust per-iteration slope in nanoseconds, or `None` when no
    /// spans were recorded.
    pub(crate) fn slope_nanos(&self) -> Option<f64> {
        self.nanos.slope()
    }

    /// Warmup-robust per-iteration statistics derived from the recorded spans, or
    /// `None` when no spans were recorded.
    pub(crate) fn statistics(&self) -> Option<OperationStatistics> {
        Some(OperationStatistics {
            span_count: self.nanos.span_count(),
            slope_nanos: self.nanos.slope()?,
            interval_nanos: self.nanos.interval(),
        })
    }

    /// Merges another operation's statistics into this one.
    pub(crate) fn merge(&mut self, other: &Self) {
        self.total_iterations = self
            .total_iterations
            .checked_add(other.total_iterations)
            .expect("total iterations overflows u64 - this indicates an unrealistic scenario");
        self.total_nanos = self
            .total_nanos
            .checked_add(other.total_nanos)
            .expect("total processor time overflows u128 - this indicates an unrealistic scenario");
        self.nanos.merge(&other.nanos);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "slope assertions are exact integer-derived values in these fixtures"
    )]

    use super::*;

    #[test]
    fn default_has_no_spans() {
        let metrics = OperationMetrics::default();
        assert_eq!(metrics.total_processor_time(), Duration::ZERO);
        assert_eq!(metrics.total_iterations(), 0);
        assert_eq!(metrics.span_count(), 0);
        assert!(metrics.is_empty());
    }

    #[test]
    fn add_iterations_basic() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(Duration::from_millis(100), 5);

        assert_eq!(metrics.total_iterations(), 5);
        assert_eq!(metrics.span_count(), 1);
        assert_eq!(metrics.total_processor_time(), Duration::from_millis(500));
    }

    #[test]
    fn add_iterations_zero_iterations() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(Duration::from_millis(100), 0);

        assert_eq!(metrics.total_iterations(), 0);
        assert_eq!(metrics.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn add_iterations_zero_duration() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(Duration::ZERO, 1000);

        assert_eq!(metrics.total_iterations(), 1000);
        assert_eq!(metrics.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn add_iterations_accumulates() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(Duration::from_millis(100), 2); // 200ms, 2 iterations
        metrics.add_iterations(Duration::from_millis(200), 3); // 600ms, 3 iterations

        assert_eq!(metrics.total_iterations(), 5);
        assert_eq!(metrics.span_count(), 2);
        assert_eq!(metrics.total_processor_time(), Duration::from_millis(800));
    }

    #[test]
    fn mean_divides_total_by_iterations() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(Duration::from_millis(100), 1);
        metrics.add_iterations(Duration::from_millis(200), 1);
        metrics.add_iterations(Duration::from_millis(300), 1);

        // (100 + 200 + 300) / 3 = 200ms.
        assert_eq!(metrics.mean(), Duration::from_millis(200));
    }

    #[test]
    fn mean_of_empty_metrics_is_zero() {
        let metrics = OperationMetrics::default();
        assert_eq!(metrics.mean(), Duration::ZERO);
    }

    #[test]
    fn merge_combines_metrics() {
        let mut first = OperationMetrics::default();
        first.add_iterations(Duration::from_millis(100), 2);

        let mut second = OperationMetrics::default();
        second.add_iterations(Duration::from_millis(50), 3);

        first.merge(&second);
        assert_eq!(first.span_count(), 2);
        assert_eq!(first.total_iterations(), 5);
    }

    #[test]
    fn slope_weights_spans_by_iteration_count() {
        // A perfectly linear series at 5 ns/iter: the slope recovers 5 regardless
        // of the differing iteration counts across spans.
        let mut metrics = OperationMetrics::default();
        metrics.add_span(2, 10);
        metrics.add_span(8, 40);

        assert_eq!(metrics.span_count(), 2);
        assert_eq!(metrics.slope_nanos(), Some(5.0));
    }

    #[test]
    fn empty_metrics_have_no_statistics() {
        let metrics = OperationMetrics::default();
        assert!(metrics.statistics().is_none());
        assert!(metrics.slope_nanos().is_none());
    }

    #[test]
    fn single_span_interval_collapses_to_the_point_estimate() {
        let mut metrics = OperationMetrics::default();
        metrics.add_span(4, 80);

        let stats = metrics.statistics().unwrap();
        assert_eq!(stats.span_count, 1);
        assert_eq!(stats.slope_nanos, 20.0);
        // With a single span there is no dispersion information, so the interval
        // is absent.
        assert!(stats.interval_nanos.is_none());
    }

    #[test]
    fn repeated_identical_spans_collapse_the_interval_onto_the_slope() {
        let mut metrics = OperationMetrics::default();
        metrics.add_span(1, 20);
        metrics.add_span(1, 20);

        let stats = metrics.statistics().unwrap();
        assert_eq!(stats.slope_nanos, 20.0);
        assert_eq!(stats.interval_nanos, Some((20.0, 20.0)));
    }
}
