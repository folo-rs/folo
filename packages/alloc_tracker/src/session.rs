//! Session management for allocation tracking.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::{Operation, Report};

/// Metrics tracked for each operation in the session.
#[derive(Clone, Debug, Default)]
pub(crate) struct OperationMetrics {
    pub(crate) total_bytes_allocated: u64,
    pub(crate) total_iterations: u64,
}

/// Manages allocation tracking session state and contains operations.
///
/// This type ensures that allocation tracking is properly enabled and disabled,
/// and prevents multiple concurrent tracking sessions which would interfere with
/// each other. It also serves as a container for tracking operations.
///
/// While `Session` is single-threaded, reports from sessions can be converted to
/// thread-safe [`Report`](crate::Report) instances using [`to_report()`](Self::to_report)
/// and sent to other threads for processing.
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
///     let _span = string_op.iterations(3).measure_process();
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
    operations: Rc<RefCell<HashMap<String, Rc<RefCell<OperationMetrics>>>>>,
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
            operations: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    /// Creates or retrieves an operation with the given name.
    ///
    /// This method exclusively borrows the session, ensuring that operations
    /// cannot be created concurrently. If an operation with the given name
    /// already exists, its existing statistics are preserved.
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
    ///     let _span = string_op.iterations(3).measure_process();
    ///     for _ in 0..3 {
    ///         let _s = String::from("test"); // This allocation will be tracked
    ///     }
    /// }
    /// ```
    pub fn operation(&self, name: impl Into<String>) -> Operation {
        let name = name.into();

        // Ensure the operation exists in the shared data
        let operation_data = {
            let mut operations = self.operations.borrow_mut();
            Rc::clone(
                operations
                    .entry(name.clone())
                    .or_insert_with(|| Rc::new(RefCell::new(OperationMetrics::default()))),
            )
        };

        Operation::new(name, operation_data)
    }

    /// Creates a thread-safe report from this session.
    ///
    /// The report contains a snapshot of all memory allocation statistics captured by this session.
    /// Unlike the single-threaded `Session`, reports can be safely sent to other threads for
    /// processing and can be merged with other reports.
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
    ///     let _span = operation.iterations(1).measure_process();
    ///     let _data = vec![1, 2, 3]; // This allocates memory
    /// }
    ///
    /// let report = session.to_report();
    /// // Report can now be sent to another thread
    /// report.print_to_stdout();
    /// ```
    #[must_use]
    pub fn to_report(&self) -> Report {
        let operations = self.operations.borrow();
        let operation_data: HashMap<String, OperationMetrics> = operations
            .iter()
            .map(|(name, data_ref)| (name.clone(), (*data_ref.borrow()).clone()))
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
        let operations = self.operations.borrow();
        operations.is_empty()
            || operations
                .values()
                .all(|op| op.borrow().total_iterations == 0)
    }
}

impl fmt::Display for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Delegate to Report's Display implementation for consistency
        write!(f, "{}", self.to_report())
    }
}
