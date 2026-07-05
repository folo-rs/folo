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
/// # Output on drop
///
/// When a session is dropped it automatically emits its results:
///
/// * a human-readable summary is printed to stdout, and
/// * machine-readable JSON files (one per operation) are written into the Cargo
///   target directory at `target/all_the_time/<operation>.json`.
///
/// A session that recorded no measurable work emits nothing, so a "list
/// available benchmarks" probe run leaves no output behind. Either output can be
/// suppressed with [`no_stdout()`](Self::no_stdout) and
/// [`no_file()`](Self::no_file).
///
/// # Examples
///
/// ```
/// use all_the_time::Session;
///
/// let session = Session::new();
/// # let session = session.no_stdout().no_file();
/// let processor_op = session.operation("processor_intensive_work");
///
/// for _ in 0..3 {
///     let _span = processor_op.measure_thread().iterations(1);
///     // Perform some processor-intensive work
///     let mut sum = 0;
///     for i in 0..1000 {
///         sum += i;
///     }
/// }
///
/// // When `session` is dropped, the recorded statistics are printed to stdout
/// // and written to `target/all_the_time/` as machine-readable JSON.
/// ```
///
/// # Panics
///
/// Dropping a session panics if writing the JSON output files fails, since a
/// benchmark that cannot persist its results is not useful. To avoid masking an
/// in-flight panic, no output is emitted while the thread is already panicking.
#[derive(Debug)]
pub struct Session {
    operations: Arc<Mutex<HashMap<String, Arc<Mutex<OperationMetrics>>>>>,
    platform: PlatformFacade,
    emit_stdout: bool,
    emit_file: bool,
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
    /// // Processor time tracking is enabled. When the session is dropped, its
    /// // results are printed to stdout and written to the Cargo target directory.
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
            emit_stdout: true,
            emit_file: true,
        }
    }

    /// Creates a new processor time tracking session with a specific platform.
    ///
    /// This method is primarily used for testing purposes to inject a fake platform
    /// that does not rely on actual system calls. Automatic output on drop is
    /// disabled so that tests do not print to stdout or write to the target
    /// directory.
    #[cfg(test)]
    pub(crate) fn with_platform(platform: PlatformFacade) -> Self {
        Self {
            operations: Arc::new(Mutex::new(HashMap::new())),
            platform,
            emit_stdout: false,
            emit_file: false,
        }
    }

    /// Disables printing a human-readable summary to stdout when the session is
    /// dropped.
    ///
    /// JSON files are still written to the Cargo target directory unless
    /// [`no_file()`](Self::no_file) is also used.
    #[must_use]
    pub fn no_stdout(mut self) -> Self {
        self.emit_stdout = false;
        self
    }

    /// Disables writing machine-readable JSON files to the Cargo target
    /// directory when the session is dropped.
    ///
    /// A human-readable summary is still printed to stdout unless
    /// [`no_stdout()`](Self::no_stdout) is also used.
    #[must_use]
    pub fn no_file(mut self) -> Self {
        self.emit_file = false;
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
    /// ```
    /// use all_the_time::Session;
    ///
    /// let session = Session::new();
    /// # let session = session.no_stdout().no_file();
    /// let processor_op = session.operation("processor_operations");
    ///
    /// for _ in 0..3 {
    ///     let _span = processor_op.measure_thread().iterations(1);
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
    /// use std::time::Duration;
    ///
    /// use all_the_time::Session;
    ///
    /// let session = Session::new();
    /// # let session = session.no_stdout().no_file();
    /// let operation = session.operation("test_work");
    /// let _span = operation.measure_thread().iterations(1);
    /// // Work happens here
    ///
    /// let report = session.to_report();
    ///
    /// // A report exposes each operation's statistics for programmatic use.
    /// let total_time: Duration = report
    ///     .operations()
    ///     .map(|(_, op)| op.total_processor_time())
    ///     .sum();
    /// println!("Total processor time: {total_time:?}");
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

    /// Whether there is any recorded activity in this session.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        let operations = self.operations.lock().expect(ERR_POISONED_LOCK);
        operations.is_empty()
            || operations
                .values()
                .all(|data| data.lock().expect(ERR_POISONED_LOCK).is_empty())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Emitting output while the thread is already unwinding would risk a
        // double panic (which aborts the process) and would obscure the original
        // failure. See docs/error-handling.md.
        if std::thread::panicking() {
            return;
        }

        // With neither output enabled there is nothing to emit, so skip building
        // a report entirely.
        if !(self.emit_stdout || self.emit_file) {
            return;
        }

        // A session that recorded no measurable work emits nothing. Returning
        // early also avoids resolving the Cargo target directory (which may shell
        // out to `cargo metadata`) during "list available benchmarks" probe runs.
        if self.is_empty() {
            return;
        }

        let report = self.to_report();
        if self.emit_stdout {
            report.print_to_stdout();
        }
        if self.emit_file {
            report.write_to_target();
        }
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

        // Create operations but do not record any spans
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
                let _span = operation.measure_thread().iterations(1);
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
                let _span = operation_with_spans.measure_process().iterations(1);
            }
        } // operation_with_spans is dropped here, merging data to session

        // Session should not be empty because at least one operation has spans
        assert!(!session.is_empty());
    }

    #[test]
    fn is_empty_multiple_operations_all_empty() {
        let session = create_test_session();

        // Create multiple operations but do not record spans
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
            let _span = operation.measure_thread().iterations(1);
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
            let _span = operation.measure_thread().iterations(1);
        } // Operation is dropped here, merging data to session

        let report = session.to_report();
        assert_eq!(session.is_empty(), report.is_empty());
        assert!(!session.is_empty());
        assert!(!report.is_empty());
    }

    use std::panic::{RefUnwindSafe, UnwindSafe};

    // The type is thread-safe.
    static_assertions::assert_impl_all!(Session: Send, Sync);

    // Static assertions for unwind safety.
    static_assertions::assert_impl_all!(Session: UnwindSafe, RefUnwindSafe);

    #[test]
    fn session_display_includes_operation_name() {
        let session = create_test_session();

        {
            let operation = session.operation("display_test_operation");
            let _span = operation.measure_thread().iterations(1);
        }

        let display_output = session.to_string();
        assert!(display_output.contains("display_test_operation"));
    }
}
