use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::{fmt, thread};

use crate::{ERR_POISONED_LOCK, Operation, OperationMetrics, Report};

/// Manages allocation tracking session state and contains operations.
///
/// # Output on drop
///
/// When a session is dropped it automatically emits its results:
///
/// * a human-readable summary is printed to stdout, and
/// * machine-readable JSON files (one per operation) are written into the Cargo
///   target directory at `target/alloc_tracker/<operation>.json`.
///
/// A session that recorded no measurable work emits nothing, so a "list
/// available benchmarks" probe run leaves no output behind. Either output can be
/// suppressed with [`no_stdout()`](Self::no_stdout) and
/// [`no_file()`](Self::no_file).
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
/// # let session = session.no_stdout().no_file();
/// let string_op = session.operation("do_stuff_with_strings");
///
/// {
///     let _span = string_op.measure_thread().iterations(3);
///     for _ in 0..3 {
///         let _data = String::from("example string allocation");
///     }
/// }
///
/// // When `session` is dropped, the recorded statistics are printed to stdout
/// // and written to `target/alloc_tracker/` as machine-readable JSON.
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
    emit_stdout: bool,
    emit_file: bool,
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
    /// // Allocation tracking is enabled. When the session is dropped, its
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
            emit_stdout: true,
            emit_file: true,
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
    /// ```rust
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new();
    /// # let session = session.no_stdout().no_file();
    /// let string_op = session.operation("string_operations");
    ///
    /// {
    ///     let _span = string_op.measure_thread().iterations(3);
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
    /// # let session = session.no_stdout().no_file();
    /// let operation = session.operation("test_work");
    /// {
    ///     let _span = operation.measure_thread().iterations(1);
    ///     let _data = vec![1, 2, 3]; // This allocates memory
    /// }
    ///
    /// let report = session.to_report();
    ///
    /// // A report exposes each operation's statistics for programmatic use.
    /// let total_bytes: u64 = report
    ///     .operations()
    ///     .map(|(_, op)| op.total_bytes_allocated())
    ///     .sum();
    /// println!("Captured {total_bytes} bytes across all operations");
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

    /// Whether there is any recorded activity in this session.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        let operations = self.operations.lock().expect(ERR_POISONED_LOCK);
        operations.is_empty()
            || operations
                .values()
                .all(|op| op.lock().expect(ERR_POISONED_LOCK).is_empty())
    }

    // Keep output policy independent of process-global destinations so unit tests can
    // observe it in memory. Ref: docs/implementation.md, "Reporting".
    fn emit_results(
        &self,
        panicking: bool,
        print: impl FnOnce(&Report),
        write: impl FnOnce(&Report),
    ) {
        // Emitting output while the thread is already unwinding would risk a
        // double panic (which aborts the process) and would obscure the original
        // failure. See docs/error-handling.md.
        if panicking {
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

        // Snapshot the report once and feed every enabled output from that single
        // value, so stdout and file outputs render identical figures.
        let report = self.to_report();
        if self.emit_stdout {
            print(&report);
        }
        if self.emit_file {
            write(&report);
        }
    }
}

impl Drop for Session {
    // This adapter only supplies thread state and real output destinations. The policy
    // stays mutation-tested in emit_results; integration tests exercise automatic output.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        self.emit_results(
            thread::panicking(),
            Report::print_to_stdout,
            Report::write_to_target,
        );
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
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use super::*;
    use crate::counters::register_fake_allocation;

    // Distinct totals reveal a missing or changed report without using the global allocator.
    const OPERATION: &str = "work";
    const ITERATIONS: u64 = 7;
    const BYTES: u64 = 91;
    const ALLOCATIONS: u64 = 13;

    // The type is thread-safe.
    static_assertions::assert_impl_all!(Session: Send, Sync);

    // Static assertions for unwind safety.
    static_assertions::assert_impl_all!(Session: UnwindSafe, RefUnwindSafe);

    #[test]
    fn empty_session_lifecycle() {
        let session = Session::new().no_stdout().no_file();
        assert!(session.is_empty());

        _ = session.operation("unmeasured");
        assert!(session.is_empty());

        // Allocator activity alone cannot define a rate when no iterations completed.
        record(&session, 0);
        assert!(session.is_empty());

        record(&session, ITERATIONS);
        assert!(!session.is_empty());

        _ = session.operation("also_unmeasured");
        assert!(!session.is_empty());
    }

    #[test]
    fn completed_iterations_without_allocations_are_not_empty() {
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation(OPERATION);
        drop(operation.measure_thread().iterations(ITERATIONS));

        assert!(!session.is_empty());
    }

    #[test]
    fn output_destinations_are_independent() {
        for (session, expected) in [
            (Session::new(), (true, true)),
            (Session::new().no_stdout(), (false, true)),
            (Session::new().no_file(), (true, false)),
            (Session::new().no_stdout().no_file(), (false, false)),
        ] {
            record(&session, ITERATIONS);
            let mut stdout = None;
            let mut file = None;
            session.emit_results(
                false,
                |report| stdout = Some(report.clone()),
                |report| file = Some(report.clone()),
            );
            // The policy has already been exercised with in-memory destinations.
            drop(session.no_stdout().no_file());

            assert_eq!((stdout.is_some(), file.is_some()), expected);
            for report in stdout.iter().chain(file.iter()) {
                assert_recorded_work(report);
            }
        }
    }

    #[test]
    fn outputs_share_a_snapshot_without_holding_session_locks() {
        let session = Session::new();
        record(&session, ITERATIONS);
        let mut stdout = None;
        let mut file = None;
        session.emit_results(
            false,
            |report| {
                stdout = Some(report.clone());
                // Updating the session while emitting the first destination must not
                // change the snapshot supplied to the second destination.
                drop(session.operations.try_lock().unwrap());
                record(&session, ITERATIONS);
            },
            |report| file = Some(report.clone()),
        );
        drop(session.no_stdout().no_file());

        assert_recorded_work(&stdout.unwrap());
        assert_recorded_work(&file.unwrap());
    }

    #[test]
    fn unused_session_emits_nothing() {
        assert_silent(Session::new(), false);
    }

    #[test]
    fn unmeasured_operations_emit_nothing() {
        let session = Session::new();
        _ = session.operation(OPERATION);
        assert_silent(session, false);
    }

    #[test]
    fn zero_iterations_emit_nothing() {
        let session = Session::new();
        record(&session, 0);
        assert_silent(session, false);
    }

    #[test]
    fn unwinding_emits_nothing() {
        let session = Session::new();
        record(&session, ITERATIONS);
        assert_silent(session, true);
    }

    fn record(session: &Session, iterations: u64) {
        let operation = session.operation(OPERATION);
        let _span = operation.measure_thread().iterations(iterations);
        register_fake_allocation(BYTES, ALLOCATIONS);
    }

    fn assert_silent(session: Session, panicking: bool) {
        let mut stdout = false;
        let mut file = false;
        session.emit_results(panicking, |_| stdout = true, |_| file = true);
        drop(session.no_stdout().no_file());

        assert!(!stdout);
        assert!(!file);
    }

    fn assert_recorded_work(report: &Report) {
        let operations = report.operations().collect::<Vec<_>>();
        assert_eq!(operations.len(), 1);
        let (name, operation) = operations.first().unwrap();
        assert_eq!(*name, OPERATION);
        assert_eq!(operation.total_iterations(), ITERATIONS);
        assert_eq!(operation.total_bytes_allocated(), BYTES);
        assert_eq!(operation.total_allocations_count(), ALLOCATIONS);
    }
}
