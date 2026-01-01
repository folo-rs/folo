use std::time::Duration;

/// Metrics tracked for each operation in the session.
#[derive(Clone, Debug, Default)]
pub(crate) struct OperationMetrics {
    pub(crate) total_processor_time: Duration,
    pub(crate) total_iterations: u64,
}

impl OperationMetrics {
    /// Adds multiple iterations of the same duration to the metrics.
    ///
    /// This is a more efficient version of calling individual add operations multiple times with the same duration.
    /// This method is used by operation and span types when they measure multiple iterations.
    pub(crate) fn add_iterations(&mut self, duration: Duration, iterations: u64) {
        // Calculate total duration by multiplying duration by iterations
        let total_duration_nanos = duration.as_nanos()
            .checked_mul(u128::from(iterations))
            .expect("duration multiplied by iterations overflows u128 - this indicates an unrealistic scenario");

        let total_duration = Duration::from_nanos(
            total_duration_nanos
                .try_into()
                .expect("total duration exceeds maximum Duration value - this indicates an unrealistic scenario"),
        );

        self.total_processor_time = self.total_processor_time.checked_add(total_duration).expect(
            "processor time accumulation overflows Duration - this indicates an unrealistic scenario",
        );

        self.total_iterations = self.total_iterations.checked_add(iterations).expect(
            "total iterations count overflows u64 - this indicates an unrealistic scenario",
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn operation_metrics_default_values() {
        let metrics = OperationMetrics::default();
        assert_eq!(metrics.total_processor_time, Duration::ZERO);
        assert_eq!(metrics.total_iterations, 0);
    }

    #[test]
    fn operation_metrics_add_iterations_basic() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(Duration::from_millis(100), 5);

        assert_eq!(metrics.total_iterations, 5);
        assert_eq!(metrics.total_processor_time, Duration::from_millis(500));
    }

    #[test]
    fn operation_metrics_add_iterations_zero_iterations() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(Duration::from_millis(100), 0);

        assert_eq!(metrics.total_iterations, 0);
        assert_eq!(metrics.total_processor_time, Duration::ZERO);
    }

    #[test]
    fn operation_metrics_add_iterations_zero_duration() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(Duration::ZERO, 1000);

        assert_eq!(metrics.total_iterations, 1000);
        assert_eq!(metrics.total_processor_time, Duration::ZERO);
    }

    #[test]
    fn operation_metrics_add_iterations_accumulates() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(Duration::from_millis(100), 2); // 200ms, 2 iterations
        metrics.add_iterations(Duration::from_millis(200), 3); // 600ms, 3 iterations

        assert_eq!(metrics.total_iterations, 5);
        assert_eq!(metrics.total_processor_time, Duration::from_millis(800));
    }
}
