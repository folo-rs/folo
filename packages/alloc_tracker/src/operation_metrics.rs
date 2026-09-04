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

/// One span's contribution to an operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SpanMeasurement {
    /// How many iterations of the operation the span covered.
    pub iterations: u64,

    /// Bytes allocated over the span's lifetime.
    pub bytes: u64,

    /// Number of allocations over the span's lifetime.
    pub count: u64,

    /// The most bytes the span held allocated at any one moment, or `None` when the span
    /// is of a kind that cannot observe it.
    pub peak_outstanding_bytes: Option<u64>,
}

/// The peak figure accumulated across an operation's spans.
///
/// Spans fold together only while every one of them could observe a peak. A span that could
/// not — a process-scoped span, which has no single thread's watermark to read — leaves the
/// operation's peak unknowable rather than merely understated, so the whole operation
/// reports nothing. Ref: docs/design.md, "Peak outstanding bytes".
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PeakOutstanding {
    /// No span has contributed yet.
    #[default]
    Unmeasured,

    /// Every span so far reported a peak, and this is the highest of them.
    Known(u64),

    /// At least one span could not report a peak.
    Unavailable,
}

impl PeakOutstanding {
    /// Folds in one span's peak.
    fn fold(self, span_peak: Option<u64>) -> Self {
        match (self, span_peak) {
            (Self::Unavailable, _) | (_, None) => Self::Unavailable,
            (Self::Unmeasured, Some(peak)) => Self::Known(peak),
            (Self::Known(highest), Some(peak)) => Self::Known(highest.max(peak)),
        }
    }

    /// Combines two independently accumulated peaks.
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unavailable, _) | (_, Self::Unavailable) => Self::Unavailable,
            (Self::Unmeasured, folded) | (folded, Self::Unmeasured) => folded,
            (Self::Known(first), Self::Known(second)) => Self::Known(first.max(second)),
        }
    }

    /// The peak in bytes, or `None` when it is unmeasured or unavailable.
    fn value(self) -> Option<u64> {
        match self {
            Self::Known(peak) => Some(peak),
            Self::Unmeasured | Self::Unavailable => None,
        }
    }
}

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
    peak_outstanding: PeakOutstanding,
    bytes: SpanAccumulator,
    allocations: SpanAccumulator,
}

impl OperationMetrics {
    /// Records one span's measurement.
    ///
    /// Folding a span is a handful of additions with no allocation, so it is
    /// cheap enough to run inside a measured span.
    pub(crate) fn add_span(&mut self, span: SpanMeasurement) {
        self.total_iterations = self
            .total_iterations
            .checked_add(span.iterations)
            .expect("total iterations overflows u64 - this indicates an unrealistic scenario");
        self.total_bytes = self
            .total_bytes
            .checked_add(span.bytes)
            .expect("total bytes overflows u64 - this indicates an unrealistic scenario");
        self.total_count = self
            .total_count
            .checked_add(span.count)
            .expect("total allocations overflows u64 - this indicates an unrealistic scenario");

        self.peak_outstanding = self.peak_outstanding.fold(span.peak_outstanding_bytes);

        self.bytes.add(span.iterations, span.bytes);
        self.allocations.add(span.iterations, span.count);
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
        self.add_span(SpanMeasurement {
            iterations,
            bytes,
            count,
            peak_outstanding_bytes: Some(bytes),
        });
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

    /// The most bytes any one span held allocated at a single moment.
    ///
    /// Returns `None` when no span has been recorded, or when any recorded span was of a
    /// kind that cannot observe a peak.
    pub(crate) fn peak_outstanding_bytes(&self) -> Option<u64> {
        self.peak_outstanding.value()
    }

    /// Mean bytes allocated per iteration, pooled across all spans.
    ///
    /// Returns zero when no iterations were recorded.
    #[cfg(test)]
    pub(crate) fn mean_bytes(&self) -> u64 {
        self.total_bytes
            .checked_div(self.total_iterations)
            .unwrap_or(0)
    }

    /// Mean number of allocations per iteration, pooled across all spans.
    ///
    /// Returns zero when no iterations were recorded.
    #[cfg(test)]
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
        self.peak_outstanding = self.peak_outstanding.merge(other.peak_outstanding);
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

    /// A span that reports a peak, for tests concerned with the other figures.
    fn span(iterations: u64, bytes: u64, count: u64) -> SpanMeasurement {
        SpanMeasurement {
            iterations,
            bytes,
            count,
            peak_outstanding_bytes: Some(bytes),
        }
    }

    /// A span that cannot report a peak, as produced by process-scoped measurement.
    fn span_without_peak(iterations: u64, bytes: u64, count: u64) -> SpanMeasurement {
        SpanMeasurement {
            iterations,
            bytes,
            count,
            peak_outstanding_bytes: None,
        }
    }

    #[test]
    fn peak_is_absent_without_spans() {
        let metrics = OperationMetrics::default();

        assert_eq!(metrics.peak_outstanding_bytes(), None);
    }

    #[test]
    fn peak_reports_the_highest_span() {
        // The peak answers "how much was live at once", so spans compete rather than sum.
        let mut metrics = OperationMetrics::default();
        metrics.add_span(SpanMeasurement {
            iterations: 1,
            bytes: 100,
            count: 1,
            peak_outstanding_bytes: Some(60),
        });
        metrics.add_span(SpanMeasurement {
            iterations: 1,
            bytes: 100,
            count: 1,
            peak_outstanding_bytes: Some(90),
        });
        metrics.add_span(SpanMeasurement {
            iterations: 1,
            bytes: 100,
            count: 1,
            peak_outstanding_bytes: Some(70),
        });

        assert_eq!(metrics.peak_outstanding_bytes(), Some(90));
    }

    #[test]
    fn one_span_without_a_peak_suppresses_the_operation_peak() {
        let mut metrics = OperationMetrics::default();
        metrics.add_span(span(1, 100, 1));
        metrics.add_span(span_without_peak(1, 100, 1));
        metrics.add_span(span(1, 100, 1));

        assert_eq!(metrics.peak_outstanding_bytes(), None);
    }

    #[test]
    fn merging_takes_the_higher_peak() {
        let mut first = OperationMetrics::default();
        first.add_span(SpanMeasurement {
            iterations: 1,
            bytes: 100,
            count: 1,
            peak_outstanding_bytes: Some(40),
        });

        let mut second = OperationMetrics::default();
        second.add_span(SpanMeasurement {
            iterations: 1,
            bytes: 100,
            count: 1,
            peak_outstanding_bytes: Some(70),
        });

        first.merge(&second);

        assert_eq!(first.peak_outstanding_bytes(), Some(70));
    }

    #[test]
    fn merging_with_an_unmeasured_operation_keeps_the_known_peak() {
        let mut measured = OperationMetrics::default();
        measured.add_span(span(1, 100, 1));

        measured.merge(&OperationMetrics::default());

        assert_eq!(measured.peak_outstanding_bytes(), Some(100));
    }

    #[test]
    fn merging_in_an_unavailable_peak_suppresses_the_result() {
        let mut measured = OperationMetrics::default();
        measured.add_span(span(1, 100, 1));

        let mut unavailable = OperationMetrics::default();
        unavailable.add_span(span_without_peak(1, 100, 1));

        measured.merge(&unavailable);

        assert_eq!(measured.peak_outstanding_bytes(), None);
    }

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
        metrics.add_span(span(2, 10, 2));
        metrics.add_span(span(8, 40, 8));

        assert_eq!(metrics.span_count(), 2);
        assert_eq!(metrics.bytes_slope(), Some(5.0));
    }

    #[test]
    fn intervals_reported_once_two_spans_recorded() {
        // Two spans at a constant per-iteration rate (5 bytes/iter, 1 alloc/iter):
        // with zero residual dispersion each interval collapses onto its slope.
        let mut metrics = OperationMetrics::default();
        metrics.add_span(span(2, 10, 2));
        metrics.add_span(span(4, 20, 4));

        assert_eq!(metrics.bytes_interval(), Some((5.0, 5.0)));
        assert_eq!(metrics.allocations_interval(), Some((1.0, 1.0)));
    }

    #[test]
    fn intervals_absent_with_a_single_span() {
        // One span pins the slopes but carries no dispersion, so neither the byte
        // nor the allocation-count interval is formed.
        let mut metrics = OperationMetrics::default();
        metrics.add_span(span(4, 20, 4));

        assert!(metrics.bytes_interval().is_none());
        assert!(metrics.allocations_interval().is_none());
    }
}
