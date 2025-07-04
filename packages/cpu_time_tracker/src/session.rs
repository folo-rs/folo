//! Session management for CPU time tracking.

use std::collections::HashMap;
use std::fmt;

use crate::Operation;

/// Manages CPU time tracking session state and contains operations.
///
/// This type serves as a container for tracking operations and provides
/// methods to analyze CPU time usage patterns across different operations.
///
/// # Examples
///
/// ```
/// use cpu_time_tracker::Session;
///
/// let mut session = Session::new();
/// let mut cpu_op = session.operation("cpu_intensive_work");
///
/// for _ in 0..3 {
///     let _span = cpu_op.measure_thread();
///     // Perform some CPU-intensive work
///     let mut sum = 0;
///     for i in 0..1000 {
///         sum += i;
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
    operations: HashMap<String, Operation>,
}

impl Session {
    /// Creates a new CPU time tracking session.
    ///
    /// # Examples
    ///
    /// ```
    /// use cpu_time_tracker::Session;
    ///
    /// let mut session = Session::new();
    /// // CPU time tracking is now enabled
    /// // Session will disable tracking when dropped
    /// ```
    #[expect(
        clippy::new_without_default,
        reason = "to avoid ambiguity with the notion of a 'default session' that is not actually a default session"
    )]
    #[must_use]
    pub fn new() -> Self {
        Self {
            operations: HashMap::new(),
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
    /// ```
    /// use cpu_time_tracker::Session;
    ///
    /// let mut session = Session::new();
    /// let mut cpu_op = session.operation("cpu_operations");
    ///
    /// for _ in 0..3 {
    ///     let _span = cpu_op.measure_thread();
    ///     // Perform some CPU-intensive work
    ///     let mut sum = 0;
    ///     for i in 0..1000 {
    ///         sum += i; // This CPU time will be tracked
    ///     }
    /// }
    /// ```
    pub fn operation(&mut self, name: impl Into<String>) -> &mut Operation {
        let name = name.into();

        // Get or create the operation
        self.operations
            .entry(name)
            .or_insert_with_key(|name| Operation::new(name.clone()))
    }

    /// Prints the CPU time statistics of all operations to stdout.
    ///
    /// Prints nothing if no spans were captured. This may indicate that the session
    /// was part of a "list available benchmarks" probe run instead of some real activity,
    /// in which case printing anything might violate the output protocol the tool is speaking.
    #[cfg_attr(test, mutants::skip)] // Too difficult to test stdout output reliably - manually tested.
    pub fn print_to_stdout(&self) {
        if self.is_empty() {
            return;
        }

        println!("{self}");
    }

    /// Whether there is any recorded activity in this session.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty() || self.operations.values().all(|op| op.spans() == 0)
    }
}

impl fmt::Display for Session {
    #[cfg_attr(test, mutants::skip)] // No API contract.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.operations.is_empty() || self.operations.values().all(|op| op.spans() == 0) {
            writeln!(f, "No CPU time statistics captured.")?;
        } else {
            writeln!(f, "CPU time statistics:")?;

            // Sort operations by name for consistent output
            let mut sorted_ops: Vec<_> = self.operations.iter().collect();
            sorted_ops.sort_by_key(|(name, _)| *name);

            for (_, operation) in sorted_ops {
                writeln!(f, "  {operation}")?;
            }
        }

        Ok(())
    }
}
