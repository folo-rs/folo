//! Per-operation allocation metrics, retained as raw spans.
//!
//! Every measured span contributes one [`SpanRecord`] carrying the whole-span
//! byte and allocation-count deltas together with the iteration count it covered.
//! Retaining the raw spans (rather than collapsing them into running totals) lets
//! the report derive both the headline pooled means and the warmup-robust
//! dispersion statistics the history tool consumes — allocation figures are not
//! deterministic, since first-run allocations and buffer resizing jitter around
//! the mean over a Criterion-chosen iteration count.

use folo_utils::{Span, SpanStats};

/// Initial capacity reserved for an operation's span buffer.
///
/// Sized to cover Criterion's default sample count (100) plus its warm-up calls
/// with headroom, so that recording a span is an allocation-free push in the
/// common case. A benchmark configured with a larger `sample_size` eventually
/// exceeds this and the next push grows the buffer. That growth allocation does
/// not contaminate the measurement: a span records itself from `Drop`, after it
/// has already captured its allocation delta and before the next span starts, so
/// the reallocation falls between measured spans and is attributed to none of
/// them.
const DEFAULT_SPAN_CAPACITY: usize = 256;

/// One recorded span: the whole-span byte and allocation-count deltas and the
/// number of iterations they covered.
///
/// The raw whole-span totals are retained (not pre-divided per iteration) so that
/// the slope regression can weight each span by its iteration count.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SpanRecord {
    /// Iterations the span measured.
    pub(crate) iterations: u64,

    /// Total bytes allocated during the span.
    pub(crate) bytes: u64,

    /// Total number of allocations during the span.
    pub(crate) count: u64,
}

/// Metrics tracked for each operation in the session.
///
/// Every measured span contributes one [`SpanRecord`]. The pooled means are
/// derived from the summed totals; the dispersion statistics are derived from the
/// per-span population via the shared [`folo_utils::SpanStats`] estimator.
#[derive(Clone, Debug)]
pub(crate) struct OperationMetrics {
    spans: Vec<SpanRecord>,
}

impl Default for OperationMetrics {
    fn default() -> Self {
        Self {
            spans: Vec::with_capacity(DEFAULT_SPAN_CAPACITY),
        }
    }
}

impl OperationMetrics {
    /// Records one span covering `iterations` iterations that allocated `bytes`
    /// bytes across `count` allocations in total.
    pub(crate) fn add_span(&mut self, iterations: u64, bytes: u64, count: u64) {
        self.spans.push(SpanRecord {
            iterations,
            bytes,
            count,
        });
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
    #[cfg(test)]
    pub(crate) fn span_count(&self) -> u64 {
        self.spans.len() as u64
    }

    /// Total iterations across all recorded spans.
    pub(crate) fn total_iterations(&self) -> u64 {
        self.spans.iter().fold(0_u64, |sum, span| {
            sum.checked_add(span.iterations)
                .expect("total iterations overflows u64 - this indicates an unrealistic scenario")
        })
    }

    /// Total bytes allocated across all recorded spans.
    pub(crate) fn total_bytes_allocated(&self) -> u64 {
        self.spans.iter().fold(0_u64, |sum, span| {
            sum.checked_add(span.bytes)
                .expect("total bytes overflows u64 - this indicates an unrealistic scenario")
        })
    }

    /// Total number of allocations across all recorded spans.
    pub(crate) fn total_allocations_count(&self) -> u64 {
        self.spans.iter().fold(0_u64, |sum, span| {
            sum.checked_add(span.count)
                .expect("total allocations overflows u64 - this indicates an unrealistic scenario")
        })
    }

    /// Mean bytes allocated per iteration, pooled across all spans.
    ///
    /// Returns zero when no iterations were recorded.
    pub(crate) fn mean_bytes(&self) -> u64 {
        self.total_bytes_allocated()
            .checked_div(self.total_iterations())
            .unwrap_or(0)
    }

    /// Mean number of allocations per iteration, pooled across all spans.
    ///
    /// Returns zero when no iterations were recorded.
    pub(crate) fn mean_allocations(&self) -> u64 {
        self.total_allocations_count()
            .checked_div(self.total_iterations())
            .unwrap_or(0)
    }

    /// Whether the operation recorded any measurable work.
    pub(crate) fn is_empty(&self) -> bool {
        self.total_iterations() == 0
    }

    /// Dispersion statistics of the per-iteration byte counts across spans, or
    /// `None` when no spans were recorded.
    pub(crate) fn bytes_stats(&self) -> Option<SpanStats> {
        let spans: Vec<Span> = self
            .spans
            .iter()
            .map(|span| Span::new(span.iterations, span.bytes))
            .collect();
        SpanStats::from_spans(&spans)
    }

    /// Dispersion statistics of the per-iteration allocation counts across spans,
    /// or `None` when no spans were recorded.
    pub(crate) fn allocations_stats(&self) -> Option<SpanStats> {
        let spans: Vec<Span> = self
            .spans
            .iter()
            .map(|span| Span::new(span.iterations, span.count))
            .collect();
        SpanStats::from_spans(&spans)
    }

    /// Appends another operation's spans onto this one.
    pub(crate) fn extend_from(&mut self, other: &Self) {
        self.spans.extend_from_slice(&other.spans);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
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
    fn default_preallocates_the_span_buffer() {
        // The buffer is reserved up front so that recording spans does not
        // allocate while the reserved capacity holds.
        let metrics = OperationMetrics::default();
        assert!(metrics.spans.capacity() >= DEFAULT_SPAN_CAPACITY);
    }

    #[test]
    fn recording_within_capacity_does_not_reallocate() {
        let mut metrics = OperationMetrics::default();
        let capacity_before = metrics.spans.capacity();
        for index in 0..DEFAULT_SPAN_CAPACITY {
            metrics.add_span(1, index as u64, 1);
        }
        assert_eq!(metrics.spans.capacity(), capacity_before);
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
    fn extend_from_concatenates_spans() {
        let mut first = OperationMetrics::default();
        first.add_iterations(100, 1, 2);

        let mut second = OperationMetrics::default();
        second.add_iterations(50, 1, 3);

        first.extend_from(&second);
        assert_eq!(first.span_count(), 2);
        assert_eq!(first.total_iterations(), 5);
        assert_eq!(first.total_bytes_allocated(), 350); // 200 + 150
    }

    #[test]
    fn empty_metrics_have_no_statistics() {
        let metrics = OperationMetrics::default();
        assert!(metrics.bytes_stats().is_none());
        assert!(metrics.allocations_stats().is_none());
    }

    #[test]
    fn statistics_weight_spans_by_iteration_count() {
        // A perfectly linear byte series at 5 bytes/iter: the slope recovers 5
        // regardless of the differing iteration counts across spans.
        let mut metrics = OperationMetrics::default();
        metrics.add_span(2, 10, 2);
        metrics.add_span(8, 40, 8);

        let bytes = metrics.bytes_stats().unwrap();
        assert_eq!(bytes.span_count, 2);
        #[expect(clippy::float_cmp, reason = "slope is an exact integer-derived value")]
        {
            assert_eq!(bytes.slope, 5.0);
        }
    }
}
