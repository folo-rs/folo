//! Memory allocation tracking reports.

use std::collections::HashMap;
use std::fmt;

use crate::{FinalizedOperation, FinalizedReport, OperationMetrics};

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
/// # let session = session.no_stdout().no_file();
/// {
///     let operation = session.operation("test_work");
///     let _span = operation.measure_process().iterations(1);
///     let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
/// }
///
/// let report = session.to_report();
///
/// // A report exposes each operation's statistics for programmatic use.
/// let total_bytes: u64 = report
///     .operations()
///     .map(|(_, op)| op.total_bytes_allocated())
///     .sum();
/// println!("Captured {total_bytes} bytes across all operations");
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
/// # let session1 = session1.no_stdout().no_file();
/// let session2 = Session::new();
/// # let session2 = session2.no_stdout().no_file();
///
/// // Record some work in each
/// {
///     let op1 = session1.operation("work");
///     let _span1 = op1.measure_process().iterations(1);
///     let _data1 = vec![1, 2, 3]; // This allocates memory
/// }
///
/// {
///     let op2 = session2.operation("work");
///     let _span2 = op2.measure_process().iterations(1);
///     let _data2 = vec![4, 5, 6, 7]; // This allocates more memory
/// }
///
/// // Convert to reports and merge
/// let report1 = session1.to_report();
/// let report2 = session2.to_report();
/// let merged = Report::merge(&report1, &report2);
///
/// // The merged report exposes the combined statistics for programmatic use.
/// let total_bytes: u64 = merged
///     .operations()
///     .map(|(_, op)| op.total_bytes_allocated())
///     .sum();
/// println!("Merged report captured {total_bytes} bytes across all operations");
/// # }
/// ```
#[derive(Clone, Debug, Default)]
pub struct Report {
    operations: HashMap<String, ReportOperation>,
}

/// Memory allocation statistics for a single operation in a report.
#[derive(Clone, Debug)]
pub struct ReportOperation {
    metrics: OperationMetrics,
}

/// Per-iteration statistics for a single allocation metric.
///
/// Every value is expressed in the metric's own per-iteration unit (bytes, or a
/// count of allocations). [`slope`](Self::slope) is the per-iteration value and
/// [`interval`](Self::interval) its 95% confidence bounds, or `None` when there
/// is not enough data to estimate them.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct MetricStatistics {
    /// The per-iteration value.
    pub slope: f64,

    /// Confidence interval `(low, high)` for [`slope`](Self::slope), or `None`
    /// when it cannot be estimated.
    pub interval: Option<(f64, f64)>,
}

/// Statistics for one operation across both allocation metrics.
///
/// Exposed through [`ReportOperation::statistics`] so callers can consume the
/// same figures that are written to the machine-readable JSON output.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct OperationStatistics {
    /// Number of spans the statistics were derived from (distinct from the total
    /// iteration count).
    pub span_count: u64,

    /// Per-iteration byte-count statistics.
    pub bytes: MetricStatistics,

    /// Per-iteration allocation-count statistics.
    pub allocations: MetricStatistics,
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
    pub(crate) fn from_operation_data(operation_data: &HashMap<String, OperationMetrics>) -> Self {
        let report_operations = operation_data
            .iter()
            .map(|(name, metrics)| {
                (
                    name.clone(),
                    ReportOperation {
                        metrics: metrics.clone(),
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
    /// Operations with the same name have their spans concatenated as if all spans
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
    /// # let session1 = session1.no_stdout().no_file();
    /// let session2 = Session::new();
    /// # let session2 = session2.no_stdout().no_file();
    ///
    /// // Both sessions record the same operation name
    /// {
    ///     let op1 = session1.operation("common_work");
    ///     let _span1 = op1.measure_process().iterations(1);
    ///     let _data1 = vec![1, 2, 3]; // 3 elements
    /// }
    ///
    /// {
    ///     let op2 = session2.operation("common_work");
    ///     let _span2 = op2.measure_process().iterations(1);
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
                .and_modify(|a_op| a_op.metrics.merge(&b_op.metrics))
                .or_insert_with(|| b_op.clone());
        }

        Self {
            operations: merged_operations,
        }
    }

    /// Fully computes this report into a [`FinalizedReport`].
    ///
    /// Both the stdout summary and the machine-readable JSON output are rendered
    /// from the returned value.
    #[must_use]
    pub fn finalize(&self) -> FinalizedReport {
        let operations = self
            .operations
            .iter()
            .map(|(name, operation)| {
                FinalizedOperation::new(
                    name.clone(),
                    operation.total_iterations(),
                    operation.total_bytes_allocated(),
                    operation.total_allocations_count(),
                    operation.mean_bytes(),
                    operation.mean_allocations(),
                    operation.statistics(),
                )
            })
            .collect();

        FinalizedReport::new(operations)
    }

    /// Prints the memory allocation statistics to stdout.
    ///
    /// Finalizes the report and prints the resulting summary. Prints nothing if
    /// no operations were captured. This may indicate that the session was part
    /// of a "list available benchmarks" probe run instead of some real activity,
    /// in which case printing anything might violate the output protocol the tool
    /// is speaking.
    // Excluded from coverage as an un-assertable stdout side effect, matching the
    // sibling `Display` impls. The `finalize()` computation it performs is covered
    // independently — `Session::drop` finalizes on the measured path and
    // `FinalizedReport` is unit-tested — so nothing computational is hidden here;
    // only the stdout emission, which cannot be meaningfully asserted, is skipped.
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)] // Too difficult to test stdout output reliably - manually tested.
    pub fn print_to_stdout(&self) {
        self.finalize().print_to_stdout();
    }

    /// Whether there is any recorded activity in this report.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty() || self.operations.values().all(|op| op.metrics.is_empty())
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
    /// # let session = session.no_stdout().no_file();
    /// {
    ///     let operation = session.operation("test_work");
    ///     let _span = operation.measure_process().iterations(1);
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
        self.metrics.total_bytes_allocated()
    }

    /// Returns the total number of allocations across all iterations for this operation.
    #[must_use]
    pub fn total_allocations_count(&self) -> u64 {
        self.metrics.total_allocations_count()
    }

    /// Returns the total number of iterations recorded for this operation.
    #[must_use]
    pub fn total_iterations(&self) -> u64 {
        self.metrics.total_iterations()
    }

    /// Calculates the mean bytes allocated per iteration.
    #[must_use]
    pub fn mean_bytes(&self) -> u64 {
        self.metrics.mean_bytes()
    }

    /// Calculates the mean number of allocations per iteration.
    #[must_use]
    pub fn mean_allocations(&self) -> u64 {
        self.metrics.mean_allocations()
    }

    /// Calculates the mean bytes allocated per iteration.
    ///
    /// This is an alias for [`mean_bytes`](Self::mean_bytes) to maintain backward compatibility.
    #[must_use]
    pub fn mean(&self) -> u64 {
        self.mean_bytes()
    }

    /// Computes per-iteration statistics over the recorded spans.
    ///
    /// Returns `None` when no spans were recorded. The returned
    /// [`OperationStatistics`] carries the per-iteration value and its confidence
    /// interval for both the byte and allocation-count metrics — the same figures
    /// written to the machine-readable JSON output.
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
    /// # let session = session.no_stdout().no_file();
    /// {
    ///     let operation = session.operation("test_work");
    ///     let _span = operation.measure_process().iterations(1);
    ///     let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
    /// }
    ///
    /// let report = session.to_report();
    /// for (_name, op) in report.operations() {
    ///     if let Some(stats) = op.statistics() {
    ///         println!(
    ///             "slope: {} bytes/iter over {} spans",
    ///             stats.bytes.slope, stats.span_count
    ///         );
    ///     }
    /// }
    /// # }
    /// ```
    #[must_use]
    pub fn statistics(&self) -> Option<OperationStatistics> {
        if self.metrics.span_count() == 0 {
            return None;
        }
        Some(OperationStatistics {
            span_count: self.metrics.span_count(),
            bytes: MetricStatistics {
                slope: self.metrics.bytes_slope()?,
                interval: self.metrics.bytes_interval(),
            },
            allocations: MetricStatistics {
                slope: self.metrics.allocations_slope()?,
                interval: self.metrics.allocations_interval(),
            },
        })
    }
}

/// Formats a per-iteration count for human-readable output.
///
/// Counts are conceptually integers but the warmup-robust slope is a real number
/// (a fitted per-iteration rate), so this rounds to two decimals and trims any
/// trailing zeros: `200.0` renders as `200` and `199.5` as `199.5`.
pub(crate) fn format_count(value: f64) -> String {
    let rounded = (value.max(0.0) * 100.0).round() / 100.0;
    let mut rendered = format!("{rounded:.2}");
    if rendered.contains('.') {
        rendered = rendered
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string();
    }
    rendered
}

impl fmt::Display for ReportOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The summary shows only the per-iteration slopes, not their intervals.
        match (self.metrics.bytes_slope(), self.metrics.allocations_slope()) {
            (Some(bytes), Some(allocations)) => write!(
                f,
                "{} bytes/iter, {} allocations/iter",
                format_count(bytes),
                format_count(allocations),
            ),
            _ => write!(f, "no measurements"),
        }
    }
}

// No API contract to test - output format is not guaranteed, and the rendering
// itself is exercised through `FinalizedReport`.
#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // There is one report and it is always fully calculated: rendering the
        // human summary goes through the same finalized value the JSON output
        // uses, rather than a cheaper slope-only path.
        write!(f, "{}", self.finalize())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "allocation statistics are exact integer-derived values in these fixtures"
    )]

    use std::panic::{RefUnwindSafe, UnwindSafe};

    use super::*;
    use crate::Session;
    use crate::allocator::register_fake_allocation;

    /// Builds a detached [`ReportOperation`] from per-iteration deltas for tests
    /// that assert directly on the report surface without a live session.
    fn report_operation(bytes_delta: u64, count_delta: u64, iterations: u64) -> ReportOperation {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(bytes_delta, count_delta, iterations);
        ReportOperation { metrics }
    }

    #[test]
    fn new_report_is_empty() {
        let report = Report::new();
        assert!(report.is_empty());
    }

    #[test]
    fn report_from_empty_session_is_empty() {
        let session = Session::new().no_stdout().no_file();
        let report = session.to_report();
        assert!(report.is_empty());
    }

    #[test]
    fn report_from_session_with_operations_is_not_empty() {
        let session = Session::new().no_stdout().no_file();
        {
            let operation = session.operation("test");
            let _span = operation.measure_thread().iterations(1);
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
        let session = Session::new().no_stdout().no_file();
        {
            let operation = session.operation("test");
            let _span = operation.measure_thread().iterations(1);
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
        let session1 = Session::new().no_stdout().no_file();
        let session2 = Session::new().no_stdout().no_file();

        {
            let op1 = session1.operation("test1");
            let _span1 = op1.measure_thread().iterations(1);
            register_fake_allocation(100, 1);
        } // Span drops here

        {
            let op2 = session2.operation("test2");
            let _span2 = op2.measure_thread().iterations(1);
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
        let session1 = Session::new().no_stdout().no_file();
        let session2 = Session::new().no_stdout().no_file();

        {
            let op1 = session1.operation("test");
            let _span1 = op1.measure_thread().iterations(1);
            register_fake_allocation(100, 1);
        } // Span drops here

        {
            let op2 = session2.operation("test");
            let _span2 = op2.measure_thread().iterations(1);
            register_fake_allocation(200, 2);
        } // Span drops here

        let report1 = session1.to_report();
        let report2 = session2.to_report();
        let merged = Report::merge(&report1, &report2);

        assert_eq!(merged.operations.len(), 1);
        let merged_op = merged.operations.get("test").unwrap();
        assert_eq!(merged_op.total_iterations(), 2); // 1 + 1
        assert_eq!(merged_op.total_bytes_allocated(), 300); // 100 + 200
        assert_eq!(merged_op.total_allocations_count(), 3); // 1 + 2
    }

    #[test]
    fn report_clone() {
        let session = Session::new().no_stdout().no_file();
        {
            let operation = session.operation("test");
            let _span = operation.measure_thread().iterations(1);
            register_fake_allocation(100, 1);
        } // Span drops here

        let report1 = session.to_report();
        let report2 = report1.clone();

        assert_eq!(report1.operations.len(), report2.operations.len());
    }

    #[test]
    fn report_operation_total_allocations_count_zero() {
        let operation = report_operation(0, 0, 1);
        assert_eq!(operation.total_allocations_count(), 0);
    }

    #[test]
    fn report_operation_total_allocations_count_multiple() {
        // 100 bytes and 5 allocations per iteration over 5 iterations.
        let operation = report_operation(100, 5, 5);
        assert_eq!(operation.total_allocations_count(), 25);
    }

    #[test]
    fn report_operation_total_allocations_count_consistency_with_session() {
        let session = Session::new().no_stdout().no_file();
        {
            let operation = session.operation("test_consistency");
            let _span = operation.measure_thread().iterations(1);
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

    #[test]
    fn statistics_are_none_without_spans() {
        let session = Session::new().no_stdout().no_file();
        let report = session.to_report();
        assert!(report.operations().next().is_none());
    }

    #[test]
    fn statistics_expose_both_metric_estimates() {
        // A single recorded span yields a span count of one and a slope equal to
        // the per-iteration mean, but carries no dispersion information, so the
        // interval is withheld.
        let operation = report_operation(200, 2, 4);
        let stats = operation.statistics().unwrap();
        assert_eq!(stats.span_count, 1);
        assert_eq!(stats.bytes.slope, 200.0);
        assert_eq!(stats.bytes.interval, None);
        assert_eq!(stats.allocations.slope, 2.0);
        assert_eq!(stats.allocations.interval, None);
    }

    #[test]
    fn repeated_identical_spans_collapse_the_interval_onto_the_slope() {
        // Two identical spans clear the two-span threshold with zero residual
        // dispersion, so the interval collapses onto the slope.
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(200, 2, 4);
        metrics.add_iterations(200, 2, 4);
        let operation = ReportOperation { metrics };

        let stats = operation.statistics().unwrap();
        assert_eq!(stats.span_count, 2);
        assert_eq!(stats.bytes.slope, 200.0);
        assert_eq!(stats.bytes.interval, Some((200.0, 200.0)));
    }

    // Static assertions for thread safety.
    static_assertions::assert_impl_all!(Report: Send, Sync);
    static_assertions::assert_impl_all!(ReportOperation: Send, Sync);
    static_assertions::assert_impl_all!(OperationStatistics: Send, Sync);
    static_assertions::assert_impl_all!(MetricStatistics: Send, Sync);

    // Static assertions for unwind safety.
    static_assertions::assert_impl_all!(Report: UnwindSafe, RefUnwindSafe);
    static_assertions::assert_impl_all!(
        ReportOperation: UnwindSafe, RefUnwindSafe
    );

    #[test]
    fn report_operation_display_shows_robust_per_iteration_estimate() {
        // 250 bytes/iter over 4 iterations → a single-span slope of 250 with the
        // interval collapsed onto it.
        let operation = report_operation(250, 3, 4);
        let display_output = operation.to_string();
        assert!(
            display_output.contains("bytes/iter"),
            "got {display_output}"
        );
        assert!(display_output.contains("250"), "got {display_output}");
    }

    #[test]
    fn report_operation_display_reports_no_measurements_when_empty() {
        // A report operation whose metrics recorded no spans has no statistics, so
        // its Display takes the `None` leg.
        let operation = ReportOperation {
            metrics: OperationMetrics::default(),
        };
        assert_eq!(operation.to_string(), "no measurements");
    }

    #[test]
    fn empty_report_display_shows_no_statistics_message() {
        let report = Report::new();
        let display_output = report.to_string();
        assert!(display_output.contains("No allocation statistics captured."));
    }
}
