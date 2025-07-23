//! Processor time tracking reports.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

/// Thread-safe processor time tracking report.
///
/// A `Report` contains the captured processor time statistics from a [`Session`](crate::Session)
/// and can be safely sent to other threads for processing. Reports can be merged together
/// and processed independently.
///
/// # Examples
///
/// ```
/// use all_the_time::Session;
///
/// # fn main() {
/// let session = Session::new();
/// let operation = session.operation("test_work");
/// let _span = operation.iterations(100).measure_thread();
/// for _ in 0..100 {
///     std::hint::black_box(42 * 2);
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
/// use std::thread;
///
/// use all_the_time::{Report, Session};
///
/// # fn main() {
/// // Create two separate sessions
/// let session1 = Session::new();
/// let session2 = Session::new();
///
/// // Record some work in each
/// let op1 = session1.operation("work");
/// let _span1 = op1.iterations(1).measure_thread();
/// std::hint::black_box(42);
///
/// let op2 = session2.operation("work");
/// let _span2 = op2.iterations(1).measure_thread();
/// std::hint::black_box(42);
///
/// // Convert to reports and merge
/// let report1 = session1.to_report();
/// let report2 = session2.to_report();
/// let merged = Report::merge(&report1, &report2);
///
/// merged.print_to_stdout();
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct Report {
    operations: HashMap<String, ReportOperation>,
}

/// Processor time statistics for a single operation in a report.
#[derive(Clone, Debug)]
pub struct ReportOperation {
    total_processor_time: Duration,
    total_iterations: u64,
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
        operation_data: &HashMap<String, crate::session::OperationMetrics>,
    ) -> Self {
        let report_operations = operation_data
            .iter()
            .map(|(name, data)| {
                (
                    name.clone(),
                    ReportOperation {
                        total_processor_time: data.total_processor_time,
                        total_iterations: data.total_iterations,
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
    /// let session2 = Session::new();
    ///
    /// // Both sessions record the same operation name
    /// let op1 = session1.operation("common_work");
    /// let _span1 = op1.iterations(5).measure_thread();
    /// for _ in 0..5 {
    ///     std::hint::black_box(42);
    /// }
    ///
    /// let op2 = session2.operation("common_work");
    /// let _span2 = op2.iterations(3).measure_thread();
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
                .and_modify(|a_op| {
                    a_op.total_processor_time = a_op
                        .total_processor_time
                        .checked_add(b_op.total_processor_time)
                        .expect("merging processor times overflows Duration - this indicates an unrealistic scenario");

                    a_op.total_iterations = a_op
                        .total_iterations
                        .checked_add(b_op.total_iterations)
                        .expect("merging iteration counts overflows u64 - this indicates an unrealistic scenario");
                })
                .or_insert_with(|| b_op.clone());
        }

        Self {
            operations: merged_operations,
        }
    }

    /// Prints the processor time statistics to stdout.
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
    /// use std::time::Duration;
    ///
    /// use all_the_time::Session;
    ///
    /// # fn main() {
    /// let session = Session::new();
    /// let operation = session.operation("test_work");
    /// let _span = operation.iterations(100).measure_thread();
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
    ///     println!("Mean time per iteration: {:?}", op.mean());
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
        self.total_processor_time
    }

    /// Returns the total number of iterations recorded for this operation.
    #[must_use]
    pub fn total_iterations(&self) -> u64 {
        self.total_iterations
    }

    /// Calculates the mean processor time per iteration.
    #[must_use]
    pub fn mean(&self) -> Duration {
        if self.total_iterations == 0 {
            Duration::ZERO
        } else {
            Duration::from_nanos(
                self.total_processor_time
                    .as_nanos()
                    .checked_div(u128::from(self.total_iterations))
                    .expect("guarded by if condition")
                    .try_into()
                    .expect("all realistic values fit in u64"),
            )
        }
    }
}

impl fmt::Display for ReportOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} (mean)", self.mean())
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.operations.is_empty() || self.operations.values().all(|op| op.total_iterations == 0)
        {
            writeln!(f, "No processor time statistics captured.")?;
        } else {
            writeln!(f, "Processor time statistics:")?;
            // Sort operations by name for consistent output
            let mut sorted_ops: Vec<_> = self.operations.iter().collect();
            sorted_ops.sort_by_key(|(name, _)| *name);
            for (name, operation) in sorted_ops {
                writeln!(f, "  {name}: {operation}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
            let _span = operation.iterations(1).measure_thread();
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
            let _span = operation.iterations(1).measure_thread();
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
            let _span1 = op1.iterations(1).measure_thread();
        } // Span drops here

        {
            let op2 = session2.operation("test2");
            let _span2 = op2.iterations(1).measure_thread();
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
            let _span1 = op1.iterations(5).measure_thread();
        } // Span drops here

        {
            let op2 = session2.operation("test");
            let _span2 = op2.iterations(3).measure_thread();
        } // Span drops here

        let report1 = session1.to_report();
        let report2 = session2.to_report();
        let merged = Report::merge(&report1, &report2);

        assert_eq!(merged.operations.len(), 1);
        let merged_op = merged.operations.get("test").unwrap();
        assert_eq!(merged_op.total_iterations, 8); // 5 + 3
    }

    #[test]
    fn report_clone() {
        let session = create_test_session();
        {
            let operation = session.operation("test");
            let _span = operation.iterations(1).measure_thread();
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
            let _span = operation.iterations(4).measure_thread();
            // Progress time during the operation to demonstrate time passing
            fake_platform.set_thread_time(Duration::from_millis(50));
        } // Operation is dropped here, recording 40ms total for 4 iterations

        // Second operation: start at 50ms, end at 90ms = 40ms duration
        {
            let operation = session.operation("test_operation");
            let _span = operation.iterations(2).measure_thread();
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
        assert_eq!(op.mean(), expected_mean);

        // Verify total duration and iteration count
        assert_eq!(op.total_processor_time(), Duration::from_millis(80));
        assert_eq!(op.total_iterations(), 6);
    }

    // Static assertions for thread safety
    static_assertions::assert_impl_all!(Report: Send, Sync);
    static_assertions::assert_impl_all!(ReportOperation: Send, Sync);
}
