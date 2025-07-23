//! Memory allocation tracking reports.

use std::collections::HashMap;
use std::fmt;

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
///     let _span = operation.iterations(1).measure_process();
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
///     let _span1 = op1.iterations(1).measure_process();
///     let _data1 = vec![1, 2, 3]; // This allocates memory
/// }
///
/// {
///     let op2 = session2.operation("work");
///     let _span2 = op2.iterations(1).measure_process();
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
#[derive(Clone, Debug)]
pub struct Report {
    operations: HashMap<String, ReportOperation>,
}

/// Memory allocation statistics for a single operation in a report.
#[derive(Clone, Debug)]
pub struct ReportOperation {
    total_bytes_allocated: u64,
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
            .map(|(name, op_data)| {
                (
                    name.clone(),
                    ReportOperation {
                        total_bytes_allocated: op_data.total_bytes_allocated,
                        total_iterations: op_data.total_iterations,
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
    ///     let _span1 = op1.iterations(1).measure_process();
    ///     let _data1 = vec![1, 2, 3]; // 3 elements
    /// }
    ///
    /// {
    ///     let op2 = session2.operation("common_work");
    ///     let _span2 = op2.iterations(1).measure_process();
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
    ///     let _span = operation.iterations(1).measure_process();
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

    /// Returns the total number of iterations recorded for this operation.
    #[must_use]
    pub fn total_iterations(&self) -> u64 {
        self.total_iterations
    }

    /// Calculates the mean bytes allocated per iteration.
    #[expect(clippy::integer_division, reason = "we accept loss of precision")]
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "division by zero excluded via if-else"
    )]
    #[must_use]
    pub fn mean(&self) -> u64 {
        if self.total_iterations == 0 {
            0
        } else {
            self.total_bytes_allocated / self.total_iterations
        }
    }
}

impl fmt::Display for ReportOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} bytes (mean)", self.mean())
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.operations.is_empty() || self.operations.values().all(|op| op.total_iterations == 0)
        {
            writeln!(f, "No allocation statistics captured.")?;
        } else {
            writeln!(f, "Allocation statistics:")?;

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
    use std::sync::atomic;

    use super::*;
    use crate::Session;
    use crate::allocator::TOTAL_BYTES_ALLOCATED;

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
            let _span = operation.iterations(1).measure_process();
            // Simulate allocation
            TOTAL_BYTES_ALLOCATED.fetch_add(100, atomic::Ordering::Relaxed);
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
            let _span = operation.iterations(1).measure_process();
            TOTAL_BYTES_ALLOCATED.fetch_add(100, atomic::Ordering::Relaxed);
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
            let _span1 = op1.iterations(1).measure_process();
            TOTAL_BYTES_ALLOCATED.fetch_add(100, atomic::Ordering::Relaxed);
        } // Span drops here

        {
            let op2 = session2.operation("test2");
            let _span2 = op2.iterations(1).measure_process();
            TOTAL_BYTES_ALLOCATED.fetch_add(200, atomic::Ordering::Relaxed);
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
            let _span1 = op1.iterations(1).measure_process();
            TOTAL_BYTES_ALLOCATED.fetch_add(100, atomic::Ordering::Relaxed);
        } // Span drops here

        {
            let op2 = session2.operation("test");
            let _span2 = op2.iterations(1).measure_process();
            TOTAL_BYTES_ALLOCATED.fetch_add(200, atomic::Ordering::Relaxed);
        } // Span drops here

        let report1 = session1.to_report();
        let report2 = session2.to_report();
        let merged = Report::merge(&report1, &report2);

        assert_eq!(merged.operations.len(), 1);
        let merged_op = merged.operations.get("test").unwrap();
        assert_eq!(merged_op.total_iterations, 2); // 1 + 1
        assert_eq!(merged_op.total_bytes_allocated, 300); // 100 + 200
    }

    #[test]
    fn report_clone() {
        let session = Session::new();
        {
            let operation = session.operation("test");
            let _span = operation.iterations(1).measure_process();
            TOTAL_BYTES_ALLOCATED.fetch_add(100, atomic::Ordering::Relaxed);
        } // Span drops here

        let report1 = session.to_report();
        let report2 = report1.clone();

        assert_eq!(report1.operations.len(), report2.operations.len());
    }

    // Static assertions for thread safety
    static_assertions::assert_impl_all!(Report: Send, Sync);
    static_assertions::assert_impl_all!(ReportOperation: Send, Sync);
}
