//! Session management for processor time tracking.
use std::collections::HashMap;
use std::fmt;

use crate::Operation;
use crate::pal::PlatformFacade;
/// Manages processor time tracking session state and contains operations.
///
/// This type serves as a container for tracking operations and provides
/// methods to analyze processor time usage patterns across different operations.
///
/// # Examples
///
/// ```
/// use all_the_time::Session;
///
/// let mut session = Session::new();
/// let mut processor_op = session.operation("processor_intensive_work");
///
/// for _ in 0..3 {
///     let _span = processor_op.iterations(1).measure_thread();
///     // Perform some processor-intensive work
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
    platform: PlatformFacade,
}
impl Session {
    /// Creates a new processor time tracking session.
    ///
    /// # Examples
    ///
    /// ```
    /// use all_the_time::Session;
    ///
    /// let mut session = Session::new();
    /// // Processor time tracking is now enabled
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
            platform: PlatformFacade::real(),
        }
    }

    /// Creates a new processor time tracking session with a specific platform.
    ///
    /// This method is primarily used for testing purposes to inject a fake platform
    /// that doesn't rely on actual system calls.
    #[cfg(test)]
    pub(crate) fn with_platform(platform: PlatformFacade) -> Self {
        Self {
            operations: HashMap::new(),
            platform,
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
    /// use all_the_time::Session;
    ///
    /// let mut session = Session::new();
    /// let mut processor_op = session.operation("processor_operations");
    ///
    /// for _ in 0..3 {
    ///     let _span = processor_op.iterations(1).measure_thread();
    ///     // Perform some processor-intensive work
    ///     let mut sum = 0;
    ///     for i in 0..1000 {
    ///         sum += i; // This processor time will be tracked
    ///     }
    /// }
    /// ```
    pub fn operation(&mut self, name: impl Into<String>) -> &mut Operation {
        let name = name.into();
        // Get or create the operation
        self.operations
            .entry(name)
            .or_insert_with(|| Operation::new(self.platform.clone()))
    }

    /// Prints the processor time statistics of all operations to stdout.
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
        self.operations.is_empty()
            || self
                .operations
                .values()
                .all(|op| op.total_iterations() == 0)
    }
}

impl fmt::Display for Session {
    #[cfg_attr(test, mutants::skip)] // No API contract.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.operations.is_empty()
            || self
                .operations
                .values()
                .all(|op| op.total_iterations() == 0)
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
    use crate::pal::{FakePlatform, PlatformFacade};

    fn create_test_session() -> Session {
        let fake_platform = FakePlatform::new();
        let platform_facade = PlatformFacade::fake(fake_platform);
        Session::with_platform(platform_facade)
    }

    #[test]
    fn is_empty_returns_true_for_no_operations() {
        let session = create_test_session();
        assert!(session.is_empty());
    }

    #[test]
    fn is_empty_returns_true_for_operations_with_no_spans() {
        let mut session = create_test_session();

        // Create operations but don't record any spans
        let _operation1 = session.operation("test1");
        let _operation2 = session.operation("test2");

        assert!(session.is_empty());
    }

    #[test]
    fn is_empty_returns_false_for_operations_with_spans() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        // Record a span
        {
            let _span = operation.iterations(1).measure_thread();
        }

        assert!(!session.is_empty());
    }

    #[test]
    fn is_empty_mixed_operations_some_with_spans() {
        let mut session = create_test_session();

        // Create some operations without spans
        let _operation1 = session.operation("no_spans1");
        let _operation2 = session.operation("no_spans2");

        // Create an operation with spans
        let operation_with_spans = session.operation("with_spans");
        {
            let _span = operation_with_spans.iterations(1).measure_process();
        }

        // Session should not be empty because at least one operation has spans
        assert!(!session.is_empty());
    }

    #[test]
    fn is_empty_multiple_operations_all_empty() {
        let mut session = create_test_session();

        // Create multiple operations but don't record spans
        for i in 0..5 {
            let _operation = session.operation(format!("test_{i}"));
        }

        assert!(session.is_empty());
    }

    #[test]
    fn is_empty_multiple_operations_all_with_spans() {
        let mut session = create_test_session();

        // Create multiple operations with spans
        for i in 0..3 {
            let operation = session.operation(format!("test_{i}"));
            let _span = operation.iterations(1).measure_thread();
        }

        assert!(!session.is_empty());
    }
}
