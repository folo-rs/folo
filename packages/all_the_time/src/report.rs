//! Processor time tracking reports.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use crate::statistics::{format_slope_nanos, nanos_to_duration};
use crate::{FinalizedOperation, FinalizedReport, OperationMetrics, OperationStatistics};

/// Thread-safe processor time tracking report.
///
/// A `Report` contains the captured processor time statistics from a [`Session`](crate::Session)
/// and can be safely sent to other threads for processing. Reports can be merged together
/// and processed independently.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
///
/// use all_the_time::Session;
///
/// # fn main() {
/// let session = Session::new();
/// # let session = session.no_stdout().no_file();
/// let operation = session.operation("test_work");
/// let _span = operation.measure_thread().iterations(100);
/// for _ in 0..100 {
///     std::hint::black_box(42 * 2);
/// }
///
/// let report = session.to_report();
///
/// // A report exposes each operation's statistics for programmatic use.
/// let total_time: Duration = report
///     .operations()
///     .map(|(_, op)| op.total_processor_time())
///     .sum();
/// println!("Total processor time: {total_time:?}");
/// # }
/// ```
///
/// # Merging reports
///
/// ```
/// use std::time::Duration;
///
/// use all_the_time::{Report, Session};
///
/// # fn main() {
/// // Create two separate sessions
/// let session1 = Session::new();
/// # let session1 = session1.no_stdout().no_file();
/// let session2 = Session::new();
/// # let session2 = session2.no_stdout().no_file();
///
/// // Record some work in each
/// let op1 = session1.operation("work");
/// let _span1 = op1.measure_thread().iterations(1);
/// std::hint::black_box(42);
///
/// let op2 = session2.operation("work");
/// let _span2 = op2.measure_thread().iterations(1);
/// std::hint::black_box(42);
///
/// // Convert to reports and merge
/// let report1 = session1.to_report();
/// let report2 = session2.to_report();
/// let merged = Report::merge(&report1, &report2);
///
/// // The merged report exposes the combined statistics for programmatic use.
/// let total_time: Duration = merged
///     .operations()
///     .map(|(_, op)| op.total_processor_time())
///     .sum();
/// println!("Merged report total processor time: {total_time:?}");
/// # }
/// ```
#[derive(Clone, Debug, Default)]
pub struct Report {
    operations: HashMap<String, ReportOperation>,
}

/// Processor time statistics for a single operation in a report.
#[derive(Clone, Debug)]
pub struct ReportOperation {
    metrics: OperationMetrics,
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
            .map(|(name, data)| {
                (
                    name.clone(),
                    ReportOperation {
                        metrics: data.clone(),
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
    /// use all_the_time::{Report, Session};
    ///
    /// # fn main() {
    /// let session1 = Session::new();
    /// # let session1 = session1.no_stdout().no_file();
    /// let session2 = Session::new();
    /// # let session2 = session2.no_stdout().no_file();
    ///
    /// // Both sessions record the same operation name
    /// let op1 = session1.operation("common_work");
    /// let _span1 = op1.measure_thread().iterations(5);
    /// for _ in 0..5 {
    ///     std::hint::black_box(42);
    /// }
    ///
    /// let op2 = session2.operation("common_work");
    /// let _span2 = op2.measure_thread().iterations(3);
    /// for _ in 0..3 {
    ///     std::hint::black_box(42);
    /// }
    ///
    /// let report1 = session1.to_report();
    /// let report2 = session2.to_report();
    ///
    /// // Merged report shows combined statistics (8 total iterations)
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
                    operation.total_processor_time(),
                    operation.metrics.mean(),
                    operation.statistics(),
                )
            })
            .collect();

        FinalizedReport::new(operations)
    }

    /// Prints the processor time statistics to stdout.
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
    /// use std::time::Duration;
    ///
    /// use all_the_time::Session;
    ///
    /// # fn main() {
    /// let session = Session::new();
    /// # let session = session.no_stdout().no_file();
    /// let operation = session.operation("test_work");
    /// let _span = operation.measure_thread().iterations(100);
    /// for _ in 0..100 {
    ///     std::hint::black_box(42 * 2);
    /// }
    ///
    /// let report = session.to_report();
    /// for (name, op) in report.operations() {
    ///     println!(
    ///         "Operation '{}' had {} iterations",
    ///         name,
    ///         op.total_iterations()
    ///     );
    ///     println!("Processor time per iteration: {:?}", op.processor_time());
    ///     println!("Total time: {:?}", op.total_processor_time());
    /// }
    /// # }
    /// ```
    pub fn operations(&self) -> impl Iterator<Item = (&str, &ReportOperation)> {
        self.operations.iter().map(|(name, op)| (name.as_str(), op))
    }
}

impl ReportOperation {
    /// Returns the total processor time across all iterations for this operation.
    #[must_use]
    pub fn total_processor_time(&self) -> Duration {
        self.metrics.total_processor_time()
    }

    /// Returns the total number of iterations recorded for this operation.
    #[must_use]
    pub fn total_iterations(&self) -> u64 {
        self.metrics.total_iterations()
    }

    /// Returns the per-iteration processor time — the primary metric for this
    /// operation.
    ///
    /// Returns `None` when there is not enough data to estimate it.
    #[must_use]
    pub fn processor_time(&self) -> Option<Duration> {
        self.metrics
            .slope_nanos()
            .filter(|slope| slope.is_finite())
            .map(nanos_to_duration)
    }

    /// Computes per-iteration statistics over the recorded spans.
    ///
    /// Returns `None` when no spans were recorded. The returned
    /// [`OperationStatistics`] carries the same figures written to the
    /// machine-readable JSON output.
    ///
    /// # Examples
    ///
    /// ```
    /// use all_the_time::Session;
    ///
    /// # fn main() {
    /// let session = Session::new();
    /// # let session = session.no_stdout().no_file();
    /// let operation = session.operation("work");
    /// let _span = operation.measure_thread().iterations(100);
    /// for _ in 0..100 {
    ///     std::hint::black_box(42 * 2);
    /// }
    /// drop(_span);
    ///
    /// let report = session.to_report();
    /// for (_name, op) in report.operations() {
    ///     if let Some(stats) = op.statistics() {
    ///         println!(
    ///             "slope: {} ns/iter over {} spans",
    ///             stats.slope_nanos, stats.span_count
    ///         );
    ///     }
    /// }
    /// # }
    /// ```
    #[must_use]
    pub fn statistics(&self) -> Option<OperationStatistics> {
        self.metrics.statistics()
    }
}

impl fmt::Display for ReportOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The summary shows only the slope, so take the cheap slope-only path.
        match self.metrics.slope_nanos() {
            Some(slope_nanos) => {
                write!(f, "{} per iteration", format_slope_nanos(slope_nanos))
            }
            None => write!(f, "no measurements"),
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
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use super::*;
    use crate::Session;

    fn create_test_session() -> Session {
        use crate::pal::{FakePlatform, PlatformFacade};
        let fake_platform = FakePlatform::new();
        let platform_facade = PlatformFacade::fake(fake_platform);
        Session::with_platform(platform_facade)
    }

    #[test]
    fn new_report_is_empty() {
        let report = Report::new();
        assert!(report.is_empty());
    }

    #[test]
    fn report_from_empty_session_is_empty() {
        let session = create_test_session();
        let report = session.to_report();
        assert!(report.is_empty());
    }

    #[test]
    fn report_from_session_with_operations_is_not_empty() {
        let session = create_test_session();
        {
            let operation = session.operation("test");
            let _span = operation.measure_thread().iterations(1);
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
        let session = create_test_session();
        {
            let operation = session.operation("test");
            let _span = operation.measure_thread().iterations(1);
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
        let session1 = create_test_session();
        let session2 = create_test_session();

        {
            let op1 = session1.operation("test1");
            let _span1 = op1.measure_thread().iterations(1);
        } // Span drops here

        {
            let op2 = session2.operation("test2");
            let _span2 = op2.measure_thread().iterations(1);
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
        let session1 = create_test_session();
        let session2 = create_test_session();

        {
            let op1 = session1.operation("test");
            let _span1 = op1.measure_thread().iterations(5);
        } // Span drops here

        {
            let op2 = session2.operation("test");
            let _span2 = op2.measure_thread().iterations(3);
        } // Span drops here

        let report1 = session1.to_report();
        let report2 = session2.to_report();
        let merged = Report::merge(&report1, &report2);

        assert_eq!(merged.operations.len(), 1);
        let merged_op = merged.operations.get("test").unwrap();
        assert_eq!(merged_op.total_iterations(), 8); // 5 + 3
    }

    #[test]
    fn report_clone() {
        let session = create_test_session();
        {
            let operation = session.operation("test");
            let _span = operation.measure_thread().iterations(1);
        } // Span drops here

        let report1 = session.to_report();
        let report2 = report1.clone();

        assert_eq!(report1.operations.len(), report2.operations.len());
    }

    #[test]
    fn report_mean_with_fake_platform() {
        use crate::pal::{FakePlatform, PlatformFacade};

        // Create fake platform that we can modify during the test
        let fake_platform = FakePlatform::new();
        let platform_facade = PlatformFacade::fake(fake_platform.clone());
        let session = Session::with_platform(platform_facade);

        // First operation: start at 10ms, end at 50ms = 40ms duration
        fake_platform.set_thread_time(Duration::from_millis(10));
        {
            let operation = session.operation("test_operation");
            let _span = operation.measure_thread().iterations(4);
            // Progress time during the operation to demonstrate time passing
            fake_platform.set_thread_time(Duration::from_millis(50));
        } // Operation is dropped here, recording 40ms total for 4 iterations

        // Second operation: start at 50ms, end at 90ms = 40ms duration
        {
            let operation = session.operation("test_operation");
            let _span = operation.measure_thread().iterations(2);
            // Progress time during this operation too
            fake_platform.set_thread_time(Duration::from_millis(90));
        } // Operation is dropped here, recording another 40ms total for 2 iterations

        let report = session.to_report();
        let operations: Vec<_> = report.operations().collect();
        assert_eq!(operations.len(), 1);

        let (_name, op) = operations.first().unwrap();

        // Verify the operation recorded meaningful durations
        // The same operation name means they merge:
        // First operation: 40ms total / 4 iterations
        // Second operation: 40ms total / 2 iterations
        // Combined: (40ms + 40ms) total / (4 + 2) iterations = 80ms / 6 = ~13.33ms mean
        let expected_mean = Duration::from_nanos(13_333_333); // 80ms / 6 iterations
        assert_eq!(op.metrics.mean(), expected_mean);

        // Verify total duration and iteration count
        assert_eq!(op.total_processor_time(), Duration::from_millis(80));
        assert_eq!(op.total_iterations(), 6);
    }

    // Static assertions for thread safety.
    static_assertions::assert_impl_all!(Report: Send, Sync);
    static_assertions::assert_impl_all!(ReportOperation: Send, Sync);

    // Static assertions for unwind safety.
    static_assertions::assert_impl_all!(Report: UnwindSafe, RefUnwindSafe);
    static_assertions::assert_impl_all!(
        ReportOperation: UnwindSafe, RefUnwindSafe
    );

    #[test]
    fn report_operation_display_shows_robust_per_iteration_estimate() {
        use crate::pal::{FakePlatform, PlatformFacade};

        let fake_platform = FakePlatform::new();
        let platform_facade = PlatformFacade::fake(fake_platform.clone());
        let session = Session::with_platform(platform_facade);

        // 100ms total for 2 iterations: the through-origin slope recovers 50ms.
        fake_platform.set_thread_time(Duration::from_millis(0));
        {
            let operation = session.operation("test_op");
            let _span = operation.measure_thread().iterations(2);
            fake_platform.set_thread_time(Duration::from_millis(100));
        }

        let report = session.to_report();
        let (_name, op) = report.operations().next().unwrap();

        let display = op.to_string();
        assert!(
            display.contains("per iteration"),
            "Display should report the per-iteration estimate: got {display}"
        );
        // Duration debug format varies, but should contain '50' for the 50ms slope.
        assert!(
            display.contains("50"),
            "Display should show the 50ms per-iteration slope: got {display}"
        );
    }

    #[test]
    fn report_operation_display_reports_no_measurements_when_empty() {
        // A report operation whose metrics recorded no spans has no statistics, so
        // its Display takes the `None` leg.
        let op = ReportOperation {
            metrics: OperationMetrics::default(),
        };
        assert_eq!(op.to_string(), "no measurements");
    }
}
