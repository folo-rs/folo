use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::constants::ERR_POISONED_LOCK;
use crate::{Operation, Report};

/// Metrics tracked for each operation in the session.
#[derive(Clone, Debug, Default)]
pub(crate) struct OperationMetrics {
    pub(crate) total_bytes_allocated: u64,
    pub(crate) total_iterations: u64,
}

impl OperationMetrics {
    /// Adds multiple iterations of the same allocation to the metrics.
    ///
    /// This is a more efficient version of calling individual add operations multiple times with the same delta.
    /// This method is used by operation and span types when they measure multiple iterations.
    pub(crate) fn add_iterations(&mut self, delta: u64, iterations: u64) {
        let total_bytes = delta
            .checked_mul(iterations)
            .expect("bytes * iterations overflows u64 - this indicates an unrealistic scenario");

        self.total_bytes_allocated = self
            .total_bytes_allocated
            .checked_add(total_bytes)
            .expect("total bytes allocated overflows u64 - this indicates an unrealistic scenario");

        self.total_iterations = self.total_iterations.checked_add(iterations).expect(
            "total iterations count overflows u64 - this indicates an unrealistic scenario",
        );
    }
}

/// Manages allocation tracking session state and contains operations.
///
/// # Examples
///
/// ```rust
/// use alloc_tracker::{Allocator, Session};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
///
/// let session = Session::new();
/// let string_op = session.operation("do_stuff_with_strings");
///
/// {
///     let _span = string_op.measure_process().iterations(3);
///     for _ in 0..3 {
///         let _data = String::from("example string allocation");
///     }
/// }
///
/// // Output statistics of all operations to console.
/// // Using print_to_stdout() here is important in benchmarks because it will
/// // print nothing if no spans were recorded, not even an empty line, which can
/// // be functionally critical for benchmark harness behavior.
/// session.print_to_stdout();
/// ```
#[derive(Debug)]
pub struct Session {
    operations: Arc<Mutex<HashMap<String, Arc<Mutex<OperationMetrics>>>>>,
}

impl Session {
    /// Creates a new allocation tracking session.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new();
    /// // Allocation tracking is now enabled
    /// // Session will disable tracking when dropped
    /// ```
    #[expect(
        clippy::new_without_default,
        reason = "to avoid ambiguity with the notion of a 'default session' that is not actually a default session"
    )]
    #[must_use]
    pub fn new() -> Self {
        Self {
            operations: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Creates or retrieves an operation with the given name.
    ///
    /// If an operation with the given name already exists, its existing statistics are preserved
    /// and any consecutive or concurrent use of multiple such `Operation` instances will merge
    /// the data sets.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new();
    /// let string_op = session.operation("string_operations");
    ///
    /// {
    ///     let _span = string_op.measure_process().iterations(3);
    ///     for _ in 0..3 {
    ///         let _s = String::from("test"); // This allocation will be tracked
    ///     }
    /// }
    /// ```
    pub fn operation(&self, name: impl Into<String>) -> Operation {
        let name = name.into();

        // Ensure the operation exists in the shared data
        let operation_data = {
            let mut operations = self.operations.lock().expect(ERR_POISONED_LOCK);
            Arc::clone(
                operations
                    .entry(name.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(OperationMetrics::default()))),
            )
        };

        Operation::new(name, operation_data)
    }

    /// Creates a thread-safe report from this session.
    ///
    /// The report contains a snapshot of all memory allocation statistics captured by this session.
    ///
    /// # Examples
    ///
    /// ```
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new();
    /// let operation = session.operation("test_work");
    /// {
    ///     let _span = operation.measure_process();
    ///     let _data = vec![1, 2, 3]; // This allocates memory
    /// }
    ///
    /// let report = session.to_report();
    /// report.print_to_stdout();
    /// ```
    #[must_use]
    pub fn to_report(&self) -> Report {
        let operations = self.operations.lock().expect(ERR_POISONED_LOCK);
        let operation_data: HashMap<String, OperationMetrics> = operations
            .iter()
            .map(|(name, data_ref)| {
                (
                    name.clone(),
                    data_ref.lock().expect(ERR_POISONED_LOCK).clone(),
                )
            })
            .collect();

        Report::from_operation_data(&operation_data)
    }

    /// Prints the allocation statistics of all operations to stdout.
    ///
    /// This is a convenience method equivalent to `self.to_report().print_to_stdout()`.
    /// Prints nothing if no spans were captured. This may indicate that the session
    /// was part of a "list available benchmarks" probe run instead of some real activity,
    /// in which case printing anything might violate the output protocol the tool is speaking.
    #[cfg_attr(test, mutants::skip)] // Too difficult to test stdout output reliably - manually tested.
    pub fn print_to_stdout(&self) {
        self.to_report().print_to_stdout();
    }

    /// Whether there is any recorded activity in this session.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        let operations = self.operations.lock().expect(ERR_POISONED_LOCK);
        operations.is_empty()
            || operations
                .values()
                .all(|op| op.lock().expect(ERR_POISONED_LOCK).total_iterations == 0)
    }
}

impl fmt::Display for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Delegate to Report's Display implementation for consistency
        write!(f, "{}", self.to_report())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_metrics_default_values() {
        let metrics = OperationMetrics::default();
        assert_eq!(metrics.total_bytes_allocated, 0);
        assert_eq!(metrics.total_iterations, 0);
    }

    #[test]
    fn operation_metrics_add_iterations_basic() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(100, 5);

        assert_eq!(metrics.total_iterations, 5);
        assert_eq!(metrics.total_bytes_allocated, 500);
    }

    #[test]
    fn operation_metrics_add_iterations_zero_iterations() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(100, 0);

        assert_eq!(metrics.total_iterations, 0);
        assert_eq!(metrics.total_bytes_allocated, 0);
    }

    #[test]
    fn operation_metrics_add_iterations_zero_allocation() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(0, 1000);

        assert_eq!(metrics.total_iterations, 1000);
        assert_eq!(metrics.total_bytes_allocated, 0);
    }

    #[test]
    fn operation_metrics_add_iterations_accumulates() {
        let mut metrics = OperationMetrics::default();
        metrics.add_iterations(100, 2); // 200 bytes, 2 iterations
        metrics.add_iterations(200, 3); // 600 bytes, 3 iterations

        assert_eq!(metrics.total_iterations, 5);
        assert_eq!(metrics.total_bytes_allocated, 800);
    }

    // The type is thread-safe.
    static_assertions::assert_impl_all!(Session: Send, Sync);
}
