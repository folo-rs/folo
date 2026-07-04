use std::time::Duration;

use crate::{OperationStatistics, SpanRecord, compute_slope_nanos, compute_statistics};

/// Initial capacity reserved for an operation's span buffer.
///
/// Sized to cover Criterion's default sample count (100) plus its warm-up calls
/// with headroom, so that recording a span is an allocation-free push in the
/// common case. A benchmark configured with a larger `sample_size` eventually
/// exceeds this and the next push grows the buffer. That growth is harmless to
/// the measurement: a span records itself from `Drop`, after it has already
/// captured its elapsed delta and before the next span starts, so the
/// reallocation falls between measured spans and never lands inside one.
const DEFAULT_SPAN_CAPACITY: usize = 256;

/// Metrics tracked for each operation in the session.
///
/// Every measured span contributes one [`SpanRecord`]. Retaining the raw spans
/// (rather than collapsing them into running totals) lets the report derive both
/// the headline mean and the dispersion statistics the history tool consumes.
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
    /// Records one span covering `iterations` iterations that consumed
    /// `total_nanos` nanoseconds of processor time in total.
    pub(crate) fn add_span(&mut self, iterations: u64, total_nanos: u64) {
        self.spans.push(SpanRecord {
            iterations,
            total_nanos,
        });
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

    /// Number of spans recorded.
    #[cfg(test)]
    pub(crate) fn span_count(&self) -> u64 {
        self.spans.len() as u64
    }

    /// Total iterations across all recorded spans.
    pub(crate) fn total_iterations(&self) -> u64 {
        self.spans
            .iter()
            .fold(0_u64, |sum, span| sum.saturating_add(span.iterations))
    }

    /// Total processor time across all recorded spans.
    pub(crate) fn total_processor_time(&self) -> Duration {
        let total_nanos = self.spans.iter().fold(0_u128, |sum, span| {
            sum.saturating_add(u128::from(span.total_nanos))
        });
        Duration::from_nanos_u128(total_nanos)
    }

    /// Mean processor time per iteration across all recorded spans.
    ///
    /// Returns zero when no iterations were recorded.
    pub(crate) fn mean(&self) -> Duration {
        let total_iterations = self.total_iterations();
        if total_iterations == 0 {
            Duration::ZERO
        } else {
            Duration::from_nanos_u128(
                self.total_processor_time()
                    .as_nanos()
                    .checked_div(u128::from(total_iterations))
                    .expect("guarded by the zero check above"),
            )
        }
    }

    /// Whether the operation recorded any measurable work.
    pub(crate) fn is_empty(&self) -> bool {
        self.total_iterations() == 0
    }

    /// Dispersion statistics derived from the recorded spans, or `None` when no
    /// spans were recorded.
    pub(crate) fn statistics(&self) -> Option<OperationStatistics> {
        compute_statistics(&self.spans)
    }

    /// The warmup-robust per-iteration slope in nanoseconds, or `None` when no
    /// spans were recorded.
    ///
    /// The cheap path for the console summary: it computes only the headline
    /// slope, skipping the bootstrap confidence interval that
    /// [`statistics`](Self::statistics) resamples.
    pub(crate) fn slope_nanos(&self) -> Option<f64> {
        compute_slope_nanos(&self.spans)
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
        assert_eq!(metrics.total_processor_time(), Duration::ZERO);
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
            metrics.add_span(1, index as u64);
        }
        assert_eq!(metrics.spans.capacity(), capacity_before);
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
    fn extend_from_concatenates_spans() {
        let mut first = OperationMetrics::default();
        first.add_iterations(Duration::from_millis(100), 2);

        let mut second = OperationMetrics::default();
        second.add_iterations(Duration::from_millis(50), 3);

        first.extend_from(&second);
        assert_eq!(first.span_count(), 2);
        assert_eq!(first.total_iterations(), 5);
    }

    #[test]
    fn empty_metrics_have_no_statistics() {
        let metrics = OperationMetrics::default();
        assert!(metrics.statistics().is_none());
    }
}
