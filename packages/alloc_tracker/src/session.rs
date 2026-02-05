use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::{ERR_POISONED_LOCK, Operation, OperationMetrics, Report};

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
    allocation_buckets_enabled: bool,
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
            allocation_buckets_enabled: false,
        }
    }

    /// Enables detailed tracking of allocations by size bucket.
    ///
    /// When enabled, allocation counts are tracked separately for each size bucket,
    /// allowing analysis of allocation size distribution. This information is then
    /// available via [`ReportOperation::allocation_buckets()`](crate::ReportOperation::allocation_buckets).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new().enable_allocation_buckets();
    /// let operation = session.operation("work");
    /// {
    ///     let _span = operation.measure_process();
    ///     let _small = vec![0u8; 32];  // < 64B bucket
    ///     let _large = vec![0u8; 1024]; // 1KB - 4KB bucket
    /// }
    ///
    /// let report = session.to_report();
    /// for (name, op) in report.operations() {
    ///     for bucket in op.allocation_buckets() {
    ///         println!("{}: {} allocations", bucket.label(), bucket.allocations());
    ///     }
    /// }
    /// ```
    #[must_use]
    pub fn enable_allocation_buckets(mut self) -> Self {
        self.allocation_buckets_enabled = true;
        self
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

        Operation::new(name, operation_data, self.allocation_buckets_enabled)
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    // The type is thread-safe.
    static_assertions::assert_impl_all!(Session: Send, Sync);
}
