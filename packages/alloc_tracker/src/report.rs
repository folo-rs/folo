//! Memory allocation tracking reports.

use std::collections::HashMap;
use std::fmt;

use crate::buckets::{AllocationBucket, BucketCounts};

/// Thread-safe memory allocation tracking report.
///
/// A `Report` contains the captured memory allocation statistics from a [`Session`](crate::Session)
/// and can be safely sent to other threads for processing. Reports can be merged together
/// and processed independently.
///
/// # Examples
///
/// ```
/// use alloc_tracker::{Allocator, Session};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
///
/// # fn main() {
/// let session = Session::new();
/// {
///     let operation = session.operation("test_work");
///     let _span = operation.measure_process();
///     let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
/// }
///
/// let report = session.to_report();
/// report.print_to_stdout();
/// # }
/// ```
///
/// # Merging reports
///
/// ```
/// use alloc_tracker::{Allocator, Report, Session};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
///
/// # fn main() {
/// // Create two separate sessions
/// let session1 = Session::new();
/// let session2 = Session::new();
///
/// // Record some work in each
/// {
///     let op1 = session1.operation("work");
///     let _span1 = op1.measure_process();
///     let _data1 = vec![1, 2, 3]; // This allocates memory
/// }
///
/// {
///     let op2 = session2.operation("work");
///     let _span2 = op2.measure_process();
///     let _data2 = vec![4, 5, 6, 7]; // This allocates more memory
/// }
///
/// // Convert to reports and merge
/// let report1 = session1.to_report();
/// let report2 = session2.to_report();
/// let merged = Report::merge(&report1, &report2);
///
/// merged.print_to_stdout();
/// # }
/// ```
#[derive(Clone, Debug, Default)]
pub struct Report {
    operations: HashMap<String, ReportOperation>,
}

/// Memory allocation statistics for a single operation in a report.
#[derive(Clone, Debug)]
pub struct ReportOperation {
    total_bytes_allocated: u64,
    total_allocations_count: u64,
    total_iterations: u64,
    bucket_counts: Option<BucketCounts>,
}

impl Report {
    /// Creates an empty report.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            operations: HashMap::new(),
        }
    }

    /// Creates a report from shared operation data.
    #[must_use]
    pub(crate) fn from_operation_data(
        operation_data: &HashMap<String, crate::operation_metrics::OperationMetrics>,
    ) -> Self {
        let report_operations = operation_data
            .iter()
            .map(|(name, op_data)| {
                (
                    name.clone(),
                    ReportOperation {
                        total_bytes_allocated: op_data.total_bytes_allocated,
                        total_allocations_count: op_data.total_allocations_count,
                        total_iterations: op_data.total_iterations,
                        bucket_counts: op_data.bucket_counts,
                    },
                )
            })
            .collect();

        Self {
            operations: report_operations,
        }
    }

    /// Merges two reports into a new report.
    ///
    /// The resulting report contains the combined statistics from both input reports.
    /// Operations with the same name have their statistics combined as if all spans
    /// had been recorded through a single session.
    ///
    /// # Examples
    ///
    /// ```
    /// use alloc_tracker::{Allocator, Report, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// # fn main() {
    /// let session1 = Session::new();
    /// let session2 = Session::new();
    ///
    /// // Both sessions record the same operation name
    /// {
    ///     let op1 = session1.operation("common_work");
    ///     let _span1 = op1.measure_process();
    ///     let _data1 = vec![1, 2, 3]; // 3 elements
    /// }
    ///
    /// {
    ///     let op2 = session2.operation("common_work");
    ///     let _span2 = op2.measure_process();
    ///     let _data2 = vec![4, 5]; // 2 elements
    /// }
    ///
    /// let report1 = session1.to_report();
    /// let report2 = session2.to_report();
    ///
    /// // Merged report shows combined statistics (2 total iterations)
    /// let merged = Report::merge(&report1, &report2);
    /// # }
    /// ```
    #[must_use]
    pub fn merge(a: &Self, b: &Self) -> Self {
        let mut merged_operations = a.operations.clone();

        for (name, b_op) in &b.operations {
            merged_operations
                .entry(name.clone())
                .and_modify(|a_op| {
                    a_op.total_bytes_allocated = a_op
                        .total_bytes_allocated
                        .checked_add(b_op.total_bytes_allocated)
                        .expect("merging bytes allocated overflows u64 - this indicates an unrealistic scenario");

                    a_op.total_allocations_count = a_op
                        .total_allocations_count
                        .checked_add(b_op.total_allocations_count)
                        .expect("merging allocations count overflows u64 - this indicates an unrealistic scenario");

                    a_op.total_iterations = a_op
                        .total_iterations
                        .checked_add(b_op.total_iterations)
                        .expect("merging iteration counts overflows u64 - this indicates an unrealistic scenario");

                    // Merge bucket counts if present in either
                    match (&mut a_op.bucket_counts, &b_op.bucket_counts) {
                        (Some(a_buckets), Some(b_buckets)) => {
                            a_buckets.merge(b_buckets);
                        }
                        (None, Some(b_buckets)) => {
                            a_op.bucket_counts = Some(*b_buckets);
                        }
                        _ => {}
                    }
                })
                .or_insert_with(|| b_op.clone());
        }

        Self {
            operations: merged_operations,
        }
    }

    /// Prints the memory allocation statistics to stdout.
    ///
    /// Prints nothing if no operations were captured. This may indicate that the session
    /// was part of a "list available benchmarks" probe run instead of some real activity,
    /// in which case printing anything might violate the output protocol the tool is speaking.
    #[cfg_attr(test, mutants::skip)] // Too difficult to test stdout output reliably - manually tested.
    pub fn print_to_stdout(&self) {
        if self.is_empty() {
            return;
        }
        println!("{self}");
    }

    /// Whether there is any recorded activity in this report.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty() || self.operations.values().all(|op| op.total_iterations == 0)
    }

    /// Returns an iterator over the operation names and their statistics.
    ///
    /// This allows programmatic access to the same data that would be printed by
    /// [`print_to_stdout()`](Self::print_to_stdout).
    ///
    /// # Examples
    ///
    /// ```
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// # fn main() {
    /// let session = Session::new();
    /// {
    ///     let operation = session.operation("test_work");
    ///     let _span = operation.measure_process();
    ///     let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
    /// }
    ///
    /// let report = session.to_report();
    /// for (name, op) in report.operations() {
    ///     println!(
    ///         "Operation '{}' had {} iterations",
    ///         name,
    ///         op.total_iterations()
    ///     );
    ///     println!("Mean bytes per iteration: {}", op.mean());
    ///     println!("Total bytes: {}", op.total_bytes_allocated());
    /// }
    /// # }
    /// ```
    pub fn operations(&self) -> impl Iterator<Item = (&str, &ReportOperation)> {
        self.operations.iter().map(|(name, op)| (name.as_str(), op))
    }
}

impl ReportOperation {
    /// Returns the total bytes allocated across all iterations for this operation.
    #[must_use]
    pub fn total_bytes_allocated(&self) -> u64 {
        self.total_bytes_allocated
    }

    /// Returns the total number of allocations across all iterations for this operation.
    #[must_use]
    pub fn total_allocations_count(&self) -> u64 {
        self.total_allocations_count
    }

    /// Returns the total number of iterations recorded for this operation.
    #[must_use]
    pub fn total_iterations(&self) -> u64 {
        self.total_iterations
    }

    /// Calculates the mean bytes allocated per iteration.
    #[expect(
        clippy::integer_division,
        reason = "we accept loss of precision for mean calculation"
    )]
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "division by zero is guarded by if-else"
    )]
    #[must_use]
    pub fn mean_bytes(&self) -> u64 {
        if self.total_iterations == 0 {
            0
        } else {
            self.total_bytes_allocated / self.total_iterations
        }
    }

    /// Calculates the mean number of allocations per iteration.
    #[expect(
        clippy::integer_division,
        reason = "we accept loss of precision for mean calculation"
    )]
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "division by zero is guarded by if-else"
    )]
    #[must_use]
    pub fn mean_allocations(&self) -> u64 {
        if self.total_iterations == 0 {
            0
        } else {
            self.total_allocations_count / self.total_iterations
        }
    }

    /// Calculates the mean bytes allocated per iteration.
    ///
    /// This is an alias for [`mean_bytes`](Self::mean_bytes) to maintain backward compatibility.
    #[must_use]
    pub fn mean(&self) -> u64 {
        self.mean_bytes()
    }

    /// Returns an iterator over allocation buckets from smallest to largest.
    ///
    /// Each bucket contains the total number of allocations that fell within that size range
    /// across all iterations. Returns an empty iterator if bucket tracking was not enabled
    /// for this operation's session.
    ///
    /// # Examples
    ///
    /// ```
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new().enable_allocation_buckets();
    /// {
    ///     let operation = session.operation("work");
    ///     let _span = operation.measure_process();
    ///     let _data = vec![0u8; 100]; // Allocates in 64B-256B bucket
    /// }
    ///
    /// let report = session.to_report();
    /// for (_, op) in report.operations() {
    ///     for bucket in op.allocation_buckets() {
    ///         if bucket.allocations() > 0 {
    ///             println!("{}: {} allocations", bucket.label(), bucket.allocations());
    ///         }
    ///     }
    /// }
    /// ```
    pub fn allocation_buckets(&self) -> impl Iterator<Item = AllocationBucket> {
        let buckets: Vec<AllocationBucket> = match &self.bucket_counts {
            Some(counts) => counts.iter().collect(),
            None => Vec::new(),
        };
        buckets.into_iter()
    }

    /// Returns an iterator over mean allocation buckets from smallest to largest.
    ///
    /// Each bucket contains the mean number of allocations per iteration that fell within
    /// that size range. Returns an empty iterator if bucket tracking was not enabled
    /// for this operation's session.
    ///
    /// # Examples
    ///
    /// ```
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new().enable_allocation_buckets();
    /// {
    ///     let operation = session.operation("work");
    ///     let _span = operation.measure_process();
    ///     let _data = vec![0u8; 100]; // Allocates in 64B-256B bucket
    /// }
    ///
    /// let report = session.to_report();
    /// for (_, op) in report.operations() {
    ///     for bucket in op.mean_allocation_buckets() {
    ///         if bucket.allocations() > 0 {
    ///             println!("{}: {} allocations/iter", bucket.label(), bucket.allocations());
    ///         }
    ///     }
    /// }
    /// ```
    pub fn mean_allocation_buckets(&self) -> impl Iterator<Item = AllocationBucket> {
        let buckets: Vec<AllocationBucket> = match &self.bucket_counts {
            Some(counts) => counts.mean(self.total_iterations).iter().collect(),
            None => Vec::new(),
        };
        buckets.into_iter()
    }
}

impl fmt::Display for ReportOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} bytes (mean)", self.mean_bytes())
    }
}

// No API contract to test - output format is not guaranteed.
#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.operations.values().all(|op| op.total_iterations == 0) {
            writeln!(f, "No allocation statistics captured.")?;
        } else {
            writeln!(f, "Allocation statistics:")?;
            writeln!(f)?;

            // Sort operations by name for consistent output
            let mut sorted_ops: Vec<_> = self.operations.iter().collect();
            sorted_ops.sort_by_key(|(name, _)| *name);

            // Calculate column widths
            let max_name_width = sorted_ops
                .iter()
                .map(|(name, _)| name.len())
                .max()
                .unwrap_or(0)
                .max("Operation".len());

            let max_bytes_width = sorted_ops
                .iter()
                .map(|(_, operation)| operation.mean_bytes().to_string().len())
                .max()
                .unwrap_or(0)
                .max("Mean bytes".len());

            let max_count_width = sorted_ops
                .iter()
                .map(|(_, operation)| operation.mean_allocations().to_string().len())
                .max()
                .unwrap_or(0)
                .max("Mean count".len());

            // Print table header
            writeln!(
                f,
                "| {:<name_width$} | {:>bytes_width$} | {:>count_width$} |",
                "Operation",
                "Mean bytes",
                "Mean count",
                name_width = max_name_width,
                bytes_width = max_bytes_width,
                count_width = max_count_width
            )?;

            // Print separator
            let separator_name_width = max_name_width
                .checked_add(2)
                .expect("operation name width fits in memory, adding 2 cannot overflow");
            let separator_bytes_width = max_bytes_width
                .checked_add(2)
                .expect("bytes width fits in memory, adding 2 cannot overflow");
            let separator_count_width = max_count_width
                .checked_add(2)
                .expect("count width fits in memory, adding 2 cannot overflow");
            writeln!(
                f,
                "|{:-<name_width$}|{:-<bytes_width$}|{:-<count_width$}|",
                "",
                "",
                "",
                name_width = separator_name_width,
                bytes_width = separator_bytes_width,
                count_width = separator_count_width
            )?;

            // Print table rows
            for (name, operation) in &sorted_ops {
                writeln!(
                    f,
                    "| {:<name_width$} | {:>bytes_width$} | {:>count_width$} |",
                    name,
                    operation.mean_bytes(),
                    operation.mean_allocations(),
                    name_width = max_name_width,
                    bytes_width = max_bytes_width,
                    count_width = max_count_width
                )?;
            }

            // Print allocation buckets if any operation has them enabled
            let has_buckets = sorted_ops.iter().any(|(_, op)| op.bucket_counts.is_some());
            if has_buckets {
                writeln!(f)?;
                writeln!(f, "Allocation buckets:")?;
                writeln!(f)?;

                for (name, operation) in &sorted_ops {
                    if let Some(ref counts) = operation.bucket_counts {
                        writeln!(f, "  {name}:")?;
                        for bucket in counts.iter() {
                            if bucket.allocations() > 0 {
                                writeln!(
                                    f,
                                    "    {:>14}: {}",
                                    bucket.label(),
                                    bucket.allocations()
                                )?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::indexing_slicing, reason = "panic on invalid index is acceptable in tests")]
mod tests {
    use super::*;
    use crate::Session;
    use crate::allocator::register_fake_allocation;

    #[test]
    fn new_report_is_empty() {
        let report = Report::new();
        assert!(report.is_empty());
    }

    #[test]
    fn report_from_empty_session_is_empty() {
        let session = Session::new();
        let report = session.to_report();
        assert!(report.is_empty());
    }

    #[test]
    fn report_from_session_with_operations_is_not_empty() {
        let session = Session::new();
        {
            let operation = session.operation("test");
            let _span = operation.measure_thread();
            register_fake_allocation(100, 1);
        } // Span drops here, releasing the mutable borrow

        let report = session.to_report();
        assert!(!report.is_empty());
    }

    #[test]
    fn merge_empty_reports() {
        let report1 = Report::new();
        let report2 = Report::new();
        let merged = Report::merge(&report1, &report2);
        assert!(merged.is_empty());
    }

    #[test]
    fn merge_empty_with_non_empty() {
        let session = Session::new();
        {
            let operation = session.operation("test");
            let _span = operation.measure_thread();
            register_fake_allocation(100, 1);
        } // Span drops here

        let report1 = Report::new();
        let report2 = session.to_report();

        let merged1 = Report::merge(&report1, &report2);
        let merged2 = Report::merge(&report2, &report1);

        assert!(!merged1.is_empty());
        assert!(!merged2.is_empty());
    }

    #[test]
    fn merge_different_operations() {
        let session1 = Session::new();
        let session2 = Session::new();

        {
            let op1 = session1.operation("test1");
            let _span1 = op1.measure_thread();
            register_fake_allocation(100, 1);
        } // Span drops here

        {
            let op2 = session2.operation("test2");
            let _span2 = op2.measure_thread();
            register_fake_allocation(200, 2);
        } // Span drops here

        let report1 = session1.to_report();
        let report2 = session2.to_report();
        let merged = Report::merge(&report1, &report2);

        assert_eq!(merged.operations.len(), 2);
        assert!(merged.operations.contains_key("test1"));
        assert!(merged.operations.contains_key("test2"));
    }

    #[test]
    fn merge_same_operations() {
        let session1 = Session::new();
        let session2 = Session::new();

        {
            let op1 = session1.operation("test");
            let _span1 = op1.measure_thread();
            register_fake_allocation(100, 1);
        } // Span drops here

        {
            let op2 = session2.operation("test");
            let _span2 = op2.measure_thread();
            register_fake_allocation(200, 2);
        } // Span drops here

        let report1 = session1.to_report();
        let report2 = session2.to_report();
        let merged = Report::merge(&report1, &report2);

        assert_eq!(merged.operations.len(), 1);
        let merged_op = merged.operations.get("test").unwrap();
        assert_eq!(merged_op.total_iterations, 2); // 1 + 1
        assert_eq!(merged_op.total_bytes_allocated, 300); // 100 + 200
        assert_eq!(merged_op.total_allocations_count, 3); // 1 + 2
    }

    #[test]
    fn report_clone() {
        let session = Session::new();
        {
            let operation = session.operation("test");
            let _span = operation.measure_thread();
            register_fake_allocation(100, 1);
        } // Span drops here

        let report1 = session.to_report();
        let report2 = report1.clone();

        assert_eq!(report1.operations.len(), report2.operations.len());
    }

    #[test]
    fn report_operation_total_allocations_count_zero() {
        let operation = ReportOperation {
            total_bytes_allocated: 0,
            total_allocations_count: 0,
            total_iterations: 1,
            bucket_counts: None,
        };

        assert_eq!(operation.total_allocations_count(), 0);
    }

    #[test]
    fn report_operation_total_allocations_count_multiple() {
        let operation = ReportOperation {
            total_bytes_allocated: 500,
            total_allocations_count: 25,
            total_iterations: 5,
            bucket_counts: None,
        };

        assert_eq!(operation.total_allocations_count(), 25);
    }

    #[test]
    fn report_operation_total_allocations_count_consistency_with_session() {
        let session = Session::new();
        {
            let operation = session.operation("test_consistency");
            let _span = operation.measure_thread();
            // Simulate 3 allocations
            register_fake_allocation(300, 3);
        } // Span drops here

        let report = session.to_report();
        let operations: Vec<_> = report.operations().collect();
        assert_eq!(operations.len(), 1);

        let (_name, report_op) = operations.first().unwrap();
        assert_eq!(report_op.total_allocations_count(), 3);
        assert_eq!(report_op.total_bytes_allocated(), 300);
        assert_eq!(report_op.total_iterations(), 1);
    }

    // Static assertions for thread safety
    static_assertions::assert_impl_all!(Report: Send, Sync);
    static_assertions::assert_impl_all!(ReportOperation: Send, Sync);

    #[test]
    fn report_operation_display_shows_mean_bytes() {
        let operation = ReportOperation {
            total_bytes_allocated: 1000,
            total_allocations_count: 10,
            total_iterations: 4,
            bucket_counts: None,
        };

        let display_output = operation.to_string();
        assert!(display_output.contains("bytes (mean)"));
        assert!(display_output.contains("250")); // 1000 / 4 = 250 mean bytes
    }

    #[test]
    fn empty_report_display_shows_no_statistics_message() {
        let report = Report::new();
        let display_output = report.to_string();
        assert!(display_output.contains("No allocation statistics captured."));
    }

    #[test]
    fn allocation_buckets_returns_empty_when_disabled() {
        let session = Session::new(); // Buckets not enabled
        {
            let operation = session.operation("test");
            let _span = operation.measure_thread();
            register_fake_allocation(100, 1);
        }

        let report = session.to_report();
        let (_, op) = report.operations().next().unwrap();
        assert_eq!(op.allocation_buckets().count(), 0);
    }

    #[test]
    fn allocation_buckets_returns_buckets_when_enabled() {
        let session = Session::new().enable_allocation_buckets();
        {
            let operation = session.operation("test");
            let _span = operation.measure_thread();
            register_fake_allocation(100, 1);
        }

        let report = session.to_report();
        let (_, op) = report.operations().next().unwrap();
        let buckets: Vec<_> = op.allocation_buckets().collect();
        assert_eq!(buckets.len(), 9); // All 9 buckets
        assert_eq!(buckets[0].label(), "< 64B");
        assert_eq!(buckets[8].label(), ">= 1MB");
    }

    #[test]
    fn mean_allocation_buckets_returns_empty_when_disabled() {
        let session = Session::new(); // Buckets not enabled
        {
            let operation = session.operation("test");
            let _span = operation.measure_thread();
            register_fake_allocation(100, 1);
        }

        let report = session.to_report();
        let (_, op) = report.operations().next().unwrap();
        assert_eq!(op.mean_allocation_buckets().count(), 0);
    }

    #[test]
    fn mean_allocation_buckets_divides_by_iterations() {
        use crate::buckets::BucketCounts;

        let operation = ReportOperation {
            total_bytes_allocated: 1000,
            total_allocations_count: 20,
            total_iterations: 4,
            bucket_counts: Some(BucketCounts::from_array([40, 20, 12, 8, 4, 0, 0, 0, 0])),
        };

        let buckets: Vec<_> = operation.mean_allocation_buckets().collect();
        assert_eq!(buckets.len(), 9);
        // 40/4=10, 20/4=5, 12/4=3, 8/4=2, 4/4=1
        assert_eq!(buckets[0].allocations(), 10);
        assert_eq!(buckets[1].allocations(), 5);
        assert_eq!(buckets[2].allocations(), 3);
        assert_eq!(buckets[3].allocations(), 2);
        assert_eq!(buckets[4].allocations(), 1);
        assert_eq!(buckets[5].allocations(), 0);
    }

    #[test]
    fn mean_allocation_buckets_zero_iterations() {
        use crate::buckets::BucketCounts;

        let operation = ReportOperation {
            total_bytes_allocated: 0,
            total_allocations_count: 0,
            total_iterations: 0,
            bucket_counts: Some(BucketCounts::from_array([10, 20, 30, 0, 0, 0, 0, 0, 0])),
        };

        let buckets: Vec<_> = operation.mean_allocation_buckets().collect();
        assert_eq!(buckets.len(), 9);
        // All should be zero when iterations is zero
        for bucket in buckets {
            assert_eq!(bucket.allocations(), 0);
        }
    }
}
