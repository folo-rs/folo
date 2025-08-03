use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::pal::PlatformFacade;
use crate::{ERR_POISONED_LOCK, Operation, OperationMetrics, Report};

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
/// let session = Session::new();
/// let processor_op = session.operation("processor_intensive_work");
///
/// for _ in 0..3 {
///     let _span = processor_op.measure_thread();
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
    operations: Arc<Mutex<HashMap<String, Arc<Mutex<OperationMetrics>>>>>,
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
    /// let session = Session::new();
    /// // Processor time tracking is enabled
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
            operations: Arc::new(Mutex::new(HashMap::new())),
            platform,
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
    /// ```
    /// use all_the_time::Session;
    ///
    /// let session = Session::new();
    /// let processor_op = session.operation("processor_operations");
    ///
    /// for _ in 0..3 {
    ///     let _span = processor_op.measure_thread();
    ///     // Perform some processor-intensive work
    ///     let mut sum = 0;
    ///     for i in 0..1000 {
    ///         sum += i; // This processor time will be tracked
    ///     }
    /// }
    /// ```
    pub fn operation(&self, name: impl Into<String>) -> Operation {
        let name = name.into();

        // Get or create operation data
        let operation_data = {
            let mut operations = self.operations.lock().expect(ERR_POISONED_LOCK);
            Arc::clone(
                operations
                    .entry(name.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(OperationMetrics::default()))),
            )
        };

        Operation::new(name, operation_data, self.platform.clone())
    }

    /// Creates a thread-safe report from this session.
    ///
    /// The report contains a snapshot of all processor time statistics captured by this session.
    /// Reports can be safely sent to other threads for processing and can be merged with other reports.
    ///
    /// # Examples
    ///
    /// ```
    /// use all_the_time::Session;
    ///
    /// let session = Session::new();
    /// let operation = session.operation("test_work");
    /// let _span = operation.measure_thread();
    /// // Work happens here
    ///
    /// let report = session.to_report();
    /// // Report can be sent to another thread
    /// report.print_to_stdout();
    /// ```
    #[must_use]
    pub fn to_report(&self) -> Report {
        // Convert Arc<Mutex<OperationMetrics>> back to plain OperationMetrics for the report
        let operations_snapshot: HashMap<String, OperationMetrics> = self
            .operations
            .lock()
            .expect(ERR_POISONED_LOCK)
            .iter()
            .map(|(name, data_ref)| {
                (
                    name.clone(),
                    data_ref.lock().expect(ERR_POISONED_LOCK).clone(),
                )
            })
            .collect();

        Report::from_operation_data(&operations_snapshot)
    }

    /// Prints the processor time statistics of all operations to stdout.
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
                .all(|data| data.lock().expect(ERR_POISONED_LOCK).total_iterations == 0)
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
        let session = create_test_session();

        // Create operations but don't record any spans
        let _operation1 = session.operation("test1");
        let _operation2 = session.operation("test2");

        assert!(session.is_empty());
    }

    #[test]
    fn is_empty_returns_false_for_operations_with_spans() {
        let session = create_test_session();

        {
            let operation = session.operation("test");

            // Record a span
            {
                let _span = operation.measure_thread();
            }
        } // operation is dropped here, merging data to session

        assert!(!session.is_empty());
    }

    #[test]
    fn is_empty_mixed_operations_some_with_spans() {
        let session = create_test_session();

        // Create some operations without spans
        let _operation1 = session.operation("no_spans1");
        let _operation2 = session.operation("no_spans2");

        // Create an operation with spans
        {
            let operation_with_spans = session.operation("with_spans");
            {
                let _span = operation_with_spans.measure_process();
            }
        } // operation_with_spans is dropped here, merging data to session

        // Session should not be empty because at least one operation has spans
        assert!(!session.is_empty());
    }

    #[test]
    fn is_empty_multiple_operations_all_empty() {
        let session = create_test_session();

        // Create multiple operations but don't record spans
        for i in 0..5 {
            let _operation = session.operation(format!("test_{i}"));
        }

        assert!(session.is_empty());
    }

    #[test]
    fn is_empty_multiple_operations_all_with_spans() {
        let session = create_test_session();

        // Create multiple operations with spans
        for i in 0..3 {
            let operation = session.operation(format!("test_{i}"));
            let _span = operation.measure_thread();
        }

        assert!(!session.is_empty());
    }

    #[test]
    fn report_is_empty_matches_session_is_empty() {
        let session = create_test_session();

        // Test 1: Both empty initially
        let report = session.to_report();
        assert_eq!(session.is_empty(), report.is_empty());
        assert!(session.is_empty());
        assert!(report.is_empty());

        // Test 2: Create operation without spans - both should still be empty
        let _operation = session.operation("test");
        let report = session.to_report();
        assert_eq!(session.is_empty(), report.is_empty());
        assert!(session.is_empty());
        assert!(report.is_empty());

        // Test 3: Add spans - both should be non-empty
        {
            let operation = session.operation("test_with_spans");
            let _span = operation.measure_thread();
        } // Operation is dropped here, merging data to session

        let report = session.to_report();
        assert_eq!(session.is_empty(), report.is_empty());
        assert!(!session.is_empty());
        assert!(!report.is_empty());
    }

    // The type is thread-safe.
    static_assertions::assert_impl_all!(Session: Send, Sync);
}
