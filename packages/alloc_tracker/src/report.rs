//! Memory allocation tracking reports.

use std::collections::HashMap;
use std::fmt;

use folo_utils::SpanStats;

use crate::OperationMetrics;

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
///     let _span = operation.measure_process();
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

/// Per-iteration dispersion statistics for a single allocation metric.
///
/// Every value is expressed in the metric's own per-iteration unit (bytes, or a
/// count of allocations). Allocation figures are not deterministic — first-run
/// allocations and buffer resizing jitter around the mean over a
/// Criterion-chosen iteration count — so the point estimate is a warmup-robust
/// through-origin slope and the interval is a bootstrap confidence interval of
/// that slope. When every span recorded the same per-iteration value the interval
/// collapses onto the point estimate.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct MetricStatistics {
    /// Through-origin OLS slope: the per-iteration point estimate.
    pub slope: f64,

    /// Sample standard deviation of the per-iteration values across spans.
    pub std_dev: f64,

    /// Lower bound of the slope's bootstrap confidence interval.
    pub interval_low: f64,

    /// Upper bound of the slope's bootstrap confidence interval.
    pub interval_high: f64,

    /// Smallest per-iteration value observed across spans.
    pub min: f64,

    /// Largest per-iteration value observed across spans.
    pub max: f64,
}

impl MetricStatistics {
    /// Re-labels a unit-agnostic [`SpanStats`] as this metric's per-iteration
    /// statistics.
    fn from_span_stats(stats: SpanStats) -> Self {
        Self {
            slope: stats.slope,
            std_dev: stats.std_dev,
            interval_low: stats.interval_low,
            interval_high: stats.interval_high,
            min: stats.min,
            max: stats.max,
        }
    }
}

/// Dispersion statistics for one operation across both allocation metrics.
///
/// Exposed through [`ReportOperation::statistics`] so callers can consume the
/// same warmup-robust dispersion that is written to the machine-readable JSON
/// output.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct OperationStatistics {
    /// Number of spans the statistics were derived from (distinct from the total
    /// iteration count).
    pub span_count: u64,

    /// Per-iteration byte-count dispersion.
    pub bytes: MetricStatistics,

    /// Per-iteration allocation-count dispersion.
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
                .and_modify(|a_op| a_op.metrics.extend_from(&b_op.metrics))
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

    /// Calculates the pooled mean bytes allocated per iteration.
    #[must_use]
    pub fn mean_bytes(&self) -> u64 {
        self.metrics.mean_bytes()
    }

    /// Calculates the pooled mean number of allocations per iteration.
    #[must_use]
    pub fn mean_allocations(&self) -> u64 {
        self.metrics.mean_allocations()
    }

    /// Calculates the pooled mean bytes allocated per iteration.
    ///
    /// This is an alias for [`mean_bytes`](Self::mean_bytes) to maintain backward compatibility.
    #[must_use]
    pub fn mean(&self) -> u64 {
        self.mean_bytes()
    }

    /// Computes warmup-robust dispersion statistics over the recorded spans.
    ///
    /// Returns `None` when no spans were recorded. The returned
    /// [`OperationStatistics`] carries the slope point estimate, bootstrap
    /// confidence interval, standard deviation and extremes for both the byte and
    /// allocation-count metrics — the same dispersion written to the
    /// machine-readable JSON output.
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
    ///     let _span = operation.measure_process();
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
        let bytes = self.metrics.bytes_stats()?;
        let allocations = self.metrics.allocations_stats()?;
        Some(OperationStatistics {
            span_count: bytes.span_count,
            bytes: MetricStatistics::from_span_stats(bytes),
            allocations: MetricStatistics::from_span_stats(allocations),
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

/// Formats a metric's warmup-robust point estimate and its bootstrap confidence
/// interval as `point [low, high]`.
pub(crate) fn format_metric(stats: &MetricStatistics) -> String {
    format!(
        "{} [{}, {}]",
        format_count(stats.slope),
        format_count(stats.interval_low),
        format_count(stats.interval_high),
    )
}

impl fmt::Display for ReportOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.statistics() {
            Some(stats) => write!(
                f,
                "{} bytes/iter [{}, {}], {} allocations/iter [{}, {}]",
                format_count(stats.bytes.slope),
                format_count(stats.bytes.interval_low),
                format_count(stats.bytes.interval_high),
                format_count(stats.allocations.slope),
                format_count(stats.allocations.interval_low),
                format_count(stats.allocations.interval_high),
            ),
            None => write!(f, "no measurements"),
        }
    }
}

// No API contract to test - output format is not guaranteed.
#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.operations.values().all(|op| op.metrics.is_empty()) {
            writeln!(f, "No allocation statistics captured.")?;
            return Ok(());
        }

        writeln!(f, "Allocation statistics:")?;
        writeln!(f)?;

        // Sort operations by name for consistent output.
        let mut sorted_ops: Vec<_> = self.operations.iter().collect();
        sorted_ops.sort_by_key(|(name, _)| *name);

        // Pre-render the warmup-robust per-iteration cells so the column widths
        // and the printed rows are computed from the exact same strings.
        let rows: Vec<(&str, String, String)> = sorted_ops
            .iter()
            .map(|(name, operation)| match operation.statistics() {
                Some(stats) => (
                    name.as_str(),
                    format_metric(&stats.bytes),
                    format_metric(&stats.allocations),
                ),
                None => (name.as_str(), "n/a".to_string(), "n/a".to_string()),
            })
            .collect();

        let name_header = "Operation";
        let bytes_header = "Bytes/iter";
        let count_header = "Allocations/iter";

        let max_name_width = rows
            .iter()
            .map(|(name, _, _)| name.len())
            .max()
            .unwrap_or(0)
            .max(name_header.len());
        let max_bytes_width = rows
            .iter()
            .map(|(_, bytes, _)| bytes.len())
            .max()
            .unwrap_or(0)
            .max(bytes_header.len());
        let max_count_width = rows
            .iter()
            .map(|(_, _, count)| count.len())
            .max()
            .unwrap_or(0)
            .max(count_header.len());

        // Print table header.
        writeln!(
            f,
            "| {name_header:<max_name_width$} | {bytes_header:>max_bytes_width$} | {count_header:>max_count_width$} |",
        )?;

        // Print separator.
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
            "|{:-<separator_name_width$}|{:-<separator_bytes_width$}|{:-<separator_count_width$}|",
            "", "", "",
        )?;

        // Print table rows.
        for (name, bytes, count) in rows {
            writeln!(
                f,
                "| {name:<max_name_width$} | {bytes:>max_bytes_width$} | {count:>max_count_width$} |",
            )?;
        }

        Ok(())
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
        let session = Session::new().no_stdout().no_file();
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
        let session1 = Session::new().no_stdout().no_file();
        let session2 = Session::new().no_stdout().no_file();

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
        let session1 = Session::new().no_stdout().no_file();
        let session2 = Session::new().no_stdout().no_file();

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
        assert_eq!(merged_op.total_iterations(), 2); // 1 + 1
        assert_eq!(merged_op.total_bytes_allocated(), 300); // 100 + 200
        assert_eq!(merged_op.total_allocations_count(), 3); // 1 + 2
    }

    #[test]
    fn report_clone() {
        let session = Session::new().no_stdout().no_file();
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

    #[test]
    fn statistics_are_none_without_spans() {
        let session = Session::new().no_stdout().no_file();
        let report = session.to_report();
        assert!(report.operations().next().is_none());
    }

    #[test]
    fn statistics_expose_both_metric_dispersions() {
        // A single recorded span yields a span count of one, a slope equal to the
        // per-iteration mean, and a degenerate interval that collapses onto it.
        let operation = report_operation(200, 2, 4);
        let stats = operation.statistics().unwrap();
        assert_eq!(stats.span_count, 1);
        assert_eq!(stats.bytes.slope, 200.0);
        assert_eq!(stats.bytes.interval_low, 200.0);
        assert_eq!(stats.bytes.interval_high, 200.0);
        assert_eq!(stats.allocations.slope, 2.0);
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
    fn empty_report_display_shows_no_statistics_message() {
        let report = Report::new();
        let display_output = report.to_string();
        assert!(display_output.contains("No allocation statistics captured."));
    }
}
