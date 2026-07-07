//! Per-operation allocation metrics, folded into streaming statistics.
//!
//! Every measured span contributes its whole-span byte and allocation-count
//! deltas together with the iteration count it covered. The spans are not
//! retained: each is folded on arrival into running totals (for the pooled means)
//! and into two [`SpanAccumulator`]s (for the warmup-robust per-iteration slope
//! and its confidence interval). Allocation figures are not deterministic —
//! first-run allocations and buffer resizing jitter around the mean over a
//! Criterion-chosen iteration count — so the slope down-weights low-iteration
//! warmup spans and the interval quantifies the residual noise.

use folo_utils::SpanAccumulator;

/// Metrics tracked for each operation in the session.
///
/// Holds the pooled totals (bytes, allocation count, iterations) and two shared
/// [`SpanAccumulator`]s — one over per-iteration bytes, one over per-iteration
/// allocation counts — folded in as each span is recorded. No per-span data is
/// retained.
#[derive(Clone, Debug, Default)]
pub(crate) struct OperationMetrics {
    total_iterations: u64,
    total_bytes: u64,
    total_count: u64,
    bytes: SpanAccumulator,
    allocations: SpanAccumulator,
}

impl OperationMetrics {
    /// Records one span covering `iterations` iterations that allocated `bytes`
    /// bytes across `count` allocations in total.
    ///
    /// Folding a span is a handful of additions with no allocation, so it is
    /// cheap enough to run inside a measured span.
    pub(crate) fn add_span(&mut self, iterations: u64, bytes: u64, count: u64) {
        self.total_iterations = self
            .total_iterations
            .checked_add(iterations)
            .expect("total iterations overflows u64 - this indicates an unrealistic scenario");
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes)
            .expect("total bytes overflows u64 - this indicates an unrealistic scenario");
        self.total_count = self
            .total_count
            .checked_add(count)
            .expect("total allocations overflows u64 - this indicates an unrealistic scenario");

        self.bytes.add(iterations, bytes);
        self.allocations.add(iterations, count);
    }

    /// Records one span by its per-iteration deltas and iteration count.
    ///
    /// A convenience over [`add_span`](Self::add_span) used where per-iteration
    /// figures are already known; the whole-span totals are reconstituted by
    /// multiplying back out.
    #[cfg(test)]
    pub(crate) fn add_iterations(&mut self, bytes_delta: u64, count_delta: u64, iterations: u64) {
        let bytes = bytes_delta
            .checked_mul(iterations)
            .expect("bytes * iterations overflows u64 - this indicates an unrealistic scenario");
        let count = count_delta
            .checked_mul(iterations)
            .expect("count * iterations overflows u64 - this indicates an unrealistic scenario");
        self.add_span(iterations, bytes, count);
    }

    /// Number of spans recorded (distinct from the total iteration count).
    pub(crate) fn span_count(&self) -> u64 {
        self.bytes.span_count()
    }

    /// Total iterations across all recorded spans.
    pub(crate) fn total_iterations(&self) -> u64 {
        self.total_iterations
    }

    /// Total bytes allocated across all recorded spans.
    pub(crate) fn total_bytes_allocated(&self) -> u64 {
        self.total_bytes
    }

    /// Total number of allocations across all recorded spans.
    pub(crate) fn total_allocations_count(&self) -> u64 {
        self.total_count
    }

    /// Mean bytes allocated per iteration, pooled across all spans.
    ///
    /// Returns zero when no iterations were recorded.
    pub(crate) fn mean_bytes(&self) -> u64 {
        self.total_bytes
            .checked_div(self.total_iterations)
            .unwrap_or(0)
    }

    /// Mean number of allocations per iteration, pooled across all spans.
    ///
    /// Returns zero when no iterations were recorded.
    pub(crate) fn mean_allocations(&self) -> u64 {
        self.total_count
            .checked_div(self.total_iterations)
            .unwrap_or(0)
    }

    /// Whether the operation recorded any measurable work.
    pub(crate) fn is_empty(&self) -> bool {
        self.total_iterations == 0
    }

    /// The warmup-robust per-iteration byte slope, or `None` when no spans were
    /// recorded.
    pub(crate) fn bytes_slope(&self) -> Option<f64> {
        self.bytes.slope()
    }

    /// The warmup-robust per-iteration allocation-count slope, or `None` when no
    /// spans were recorded.
    pub(crate) fn allocations_slope(&self) -> Option<f64> {
        self.allocations.slope()
    }

    /// The confidence interval of the per-iteration byte slope, or `None` when it
    /// cannot be estimated (fewer than two spans, or a non-finite estimate).
    pub(crate) fn bytes_interval(&self) -> Option<(f64, f64)> {
        self.bytes.interval()
    }

    /// The confidence interval of the per-iteration allocation-count slope, or
    /// `None` when it cannot be estimated.
    pub(crate) fn allocations_interval(&self) -> Option<(f64, f64)> {
        self.allocations.interval()
    }

    /// Merges another operation's statistics into this one.
    pub(crate) fn merge(&mut self, other: &Self) {
        self.total_iterations = self
            .total_iterations
            .checked_add(other.total_iterations)
            .expect("total iterations overflows u64 - this indicates an unrealistic scenario");
        self.total_bytes = self
            .total_bytes
            .checked_add(other.total_bytes)
            .expect("total bytes overflows u64 - this indicates an unrealistic scenario");
        self.total_count = self
            .total_count
            .checked_add(other.total_count)
            .expect("total allocations overflows u64 - this indicates an unrealistic scenario");
        self.bytes.merge(&other.bytes);
        self.allocations.merge(&other.allocations);
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
        assert_eq!(metrics.total_bytes_allocated(), 0);
        assert_eq!(metrics.total_allocations_count(), 0);
        assert_eq!(metrics.total_iterations(), 0);
        assert_eq!(metrics.span_count(), 0);
        assert!(metrics.is_empty());
    }

    #[test]
    fn add_iterations_basic() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(100, 5, 5);

        assert_eq!(metrics.total_iterations(), 5);
        assert_eq!(metrics.span_count(), 1);
        assert_eq!(metrics.total_bytes_allocated(), 500);
        assert_eq!(metrics.total_allocations_count(), 25);
    }

    #[test]
    fn add_iterations_zero_iterations() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(100, 2, 0);

        assert_eq!(metrics.total_iterations(), 0);
        assert_eq!(metrics.total_bytes_allocated(), 0);
        assert_eq!(metrics.total_allocations_count(), 0);
    }

    #[test]
    fn zero_iteration_span_yields_nan_slopes() {
        // A span that covered zero iterations (e.g. a workload that failed to run)
        // has no per-iteration rate, so both slopes report NaN rather than a
        // misleading zero.
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(100, 2, 0);

        assert!(metrics.bytes_slope().unwrap().is_nan());
        assert!(metrics.allocations_slope().unwrap().is_nan());
        assert_eq!(metrics.bytes_interval(), None);
        assert_eq!(metrics.allocations_interval(), None);
    }

    #[test]
    fn add_iterations_zero_allocation() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(0, 0, 1000);

        assert_eq!(metrics.total_iterations(), 1000);
        assert_eq!(metrics.total_bytes_allocated(), 0);
        assert_eq!(metrics.total_allocations_count(), 0);
    }

    #[test]
    fn add_iterations_accumulates() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(100, 2, 2); // 200 bytes, 4 allocations, 2 iterations
        metrics.add_iterations(200, 3, 3); // 600 bytes, 9 allocations, 3 iterations

        assert_eq!(metrics.total_iterations(), 5);
        assert_eq!(metrics.span_count(), 2);
        assert_eq!(metrics.total_bytes_allocated(), 800);
        assert_eq!(metrics.total_allocations_count(), 13);
    }

    #[test]
    fn pooled_means_divide_totals_by_iterations() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(100, 1, 1);
        metrics.add_iterations(200, 2, 1);
        metrics.add_iterations(300, 3, 1);

        // (100 + 200 + 300) / 3 = 200; (1 + 2 + 3) / 3 = 2.
        assert_eq!(metrics.mean_bytes(), 200);
        assert_eq!(metrics.mean_allocations(), 2);
    }

    #[test]
    fn means_of_empty_metrics_are_zero() {
        let metrics = OperationMetrics::default();
        assert_eq!(metrics.mean_bytes(), 0);
        assert_eq!(metrics.mean_allocations(), 0);
    }

    #[test]
    fn merge_combines_metrics() {
        let mut first = OperationMetrics::default();
        first.add_iterations(100, 1, 2);

        let mut second = OperationMetrics::default();
        second.add_iterations(50, 1, 3);

        first.merge(&second);
        assert_eq!(first.span_count(), 2);
        assert_eq!(first.total_iterations(), 5);
        assert_eq!(first.total_bytes_allocated(), 350); // 200 + 150
    }

    #[test]
    fn empty_metrics_have_no_slope() {
        let metrics = OperationMetrics::default();
        assert!(metrics.bytes_slope().is_none());
        assert!(metrics.allocations_slope().is_none());
    }

    #[test]
    fn slope_weights_spans_by_iteration_count() {
        // A perfectly linear byte series at 5 bytes/iter: the slope recovers 5
        // regardless of the differing iteration counts across spans.
        let mut metrics = OperationMetrics::default();
        metrics.add_span(2, 10, 2);
        metrics.add_span(8, 40, 8);

        assert_eq!(metrics.span_count(), 2);
        assert_eq!(metrics.bytes_slope(), Some(5.0));
    }

    #[test]
    fn intervals_reported_once_two_spans_recorded() {
        // Two spans at a constant per-iteration rate (5 bytes/iter, 1 alloc/iter):
        // with zero residual dispersion each interval collapses onto its slope.
        let mut metrics = OperationMetrics::default();
        metrics.add_span(2, 10, 2);
        metrics.add_span(4, 20, 4);

        assert_eq!(metrics.bytes_interval(), Some((5.0, 5.0)));
        assert_eq!(metrics.allocations_interval(), Some((1.0, 1.0)));
    }

    #[test]
    fn intervals_absent_with_a_single_span() {
        // One span pins the slopes but carries no dispersion, so neither the byte
        // nor the allocation-count interval is formed.
        let mut metrics = OperationMetrics::default();
        metrics.add_span(4, 20, 4);

        assert!(metrics.bytes_interval().is_none());
        assert!(metrics.allocations_interval().is_none());
    }
}
