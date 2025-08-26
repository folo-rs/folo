/// Metrics tracked for each operation in the session.
#[derive(Clone, Debug, Default)]
#[expect(clippy::struct_field_names, reason = "field names are descriptive and clear")]
pub(crate) struct OperationMetrics {
    pub(crate) total_bytes_allocated: u64,
    pub(crate) total_allocations_count: u64,
    pub(crate) total_iterations: u64,
}

impl OperationMetrics {
    /// Adds multiple iterations of the same allocation to the metrics.
    ///
    /// This is a more efficient version of calling individual add operations multiple times with the same delta.
    /// This method is used by operation and span types when they measure multiple iterations.
    pub(crate) fn add_iterations(&mut self, bytes_delta: u64, count_delta: u64, iterations: u64) {
        let total_bytes = bytes_delta
            .checked_mul(iterations)
            .expect("bytes * iterations overflows u64 - this indicates an unrealistic scenario");

        let total_count = count_delta
            .checked_mul(iterations)
            .expect("count * iterations overflows u64 - this indicates an unrealistic scenario");

        self.total_bytes_allocated = self
            .total_bytes_allocated
            .checked_add(total_bytes)
            .expect("total bytes allocated overflows u64 - this indicates an unrealistic scenario");

        self.total_allocations_count = self
            .total_allocations_count
            .checked_add(total_count)
            .expect("total allocations count overflows u64 - this indicates an unrealistic scenario");

        self.total_iterations = self.total_iterations.checked_add(iterations).expect(
            "total iterations count overflows u64 - this indicates an unrealistic scenario",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_metrics_default_values() {
        let metrics = OperationMetrics::default();
        assert_eq!(metrics.total_bytes_allocated, 0);
        assert_eq!(metrics.total_allocations_count, 0);
        assert_eq!(metrics.total_iterations, 0);
    }

    #[test]
    fn operation_metrics_add_iterations_basic() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(100, 5, 5);

        assert_eq!(metrics.total_iterations, 5);
        assert_eq!(metrics.total_bytes_allocated, 500);
        assert_eq!(metrics.total_allocations_count, 25);
    }

    #[test]
    fn operation_metrics_add_iterations_zero_iterations() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(100, 2, 0);

        assert_eq!(metrics.total_iterations, 0);
        assert_eq!(metrics.total_bytes_allocated, 0);
        assert_eq!(metrics.total_allocations_count, 0);
    }

    #[test]
    fn operation_metrics_add_iterations_zero_allocation() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(0, 0, 1000);

        assert_eq!(metrics.total_iterations, 1000);
        assert_eq!(metrics.total_bytes_allocated, 0);
        assert_eq!(metrics.total_allocations_count, 0);
    }

    #[test]
    fn operation_metrics_add_iterations_accumulates() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(100, 2, 2); // 200 bytes, 4 allocations, 2 iterations
        metrics.add_iterations(200, 3, 3); // 600 bytes, 9 allocations, 3 iterations

        assert_eq!(metrics.total_iterations, 5);
        assert_eq!(metrics.total_bytes_allocated, 800);
        assert_eq!(metrics.total_allocations_count, 13);
    }
}
