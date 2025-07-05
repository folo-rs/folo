//! Session management for allocation tracking.

use std::collections::HashMap;
use std::fmt;

use crate::Operation;

/// Manages allocation tracking session state and contains operations.
///
/// This type ensures that allocation tracking is properly enabled and disabled,
/// and prevents multiple concurrent tracking sessions which would interfere with
/// each other. It also serves as a container for tracking operations.
///
/// # Examples
///
/// ```rust
/// use alloc_tracker::{Allocator, Session};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
///
/// let mut session = Session::new();
/// let mut string_op = session.operation("do_stuff_with_strings");
///
/// for _ in 0..3 {
///     let _span = string_op.measure_process();
///     let _data = String::from("example string allocation");
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
    /// let mut session = Session::new();
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
    /// ```rust
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let mut session = Session::new();
    /// let mut string_op = session.operation("string_operations");
    ///
    /// for _ in 0..3 {
    ///     let _span = string_op.measure_process();
    ///     let _s = String::from("test"); // This allocation will be tracked
    /// }
    /// ```
    pub fn operation(&mut self, name: impl Into<String>) -> &mut Operation {
        let name = name.into();

        // Get or create the operation
        self.operations
            .entry(name)
            .or_insert_with_key(|name| Operation::new(name.clone()))
    }

    /// Prints the allocation statistics of all operations to stdout.
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
            writeln!(f, "No allocation statistics captured.")?;
        } else {
            writeln!(f, "Allocation statistics:")?;

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
