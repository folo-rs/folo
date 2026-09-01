use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Reports the operation a watchdog-protected test is about to block on.
///
/// [`with_watchdog_phases()`] includes the most recently reported phase in its
/// timeout panic, so a synchronization failure identifies the operation that did
/// not complete.
#[derive(Clone, Debug)]
pub struct WatchdogPhases {
    phase_tx: mpsc::Sender<&'static str>,
}

impl WatchdogPhases {
    /// Replaces the phase shown if the test times out.
    pub fn set(&self, phase: &'static str) {
        // The receiver only disappears after the watchdog has already ended the test.
        _ = self.phase_tx.send(phase);
    }
}

/// Runs a test with a timeout to prevent infinite hangs.
///
/// This function wraps a test closure with a timeout mechanism. The closure runs
/// on a separate worker thread; if it does not complete within the timeout, this
/// function panics on the calling thread so the test harness records a failure
/// instead of letting a hung test block CI/build systems indefinitely. The
/// timed-out worker thread is left detached — the process does not wait for it and
/// reclaims it on exit.
///
/// The timeout is 10 seconds under normal conditions and 60 seconds under
/// Miri, where thread synchronization primitives are significantly slower.
///
/// When the `MUTATION_TESTING` environment variable is set to "1", the watchdog
/// is disabled and the test function is executed directly. This allows mutation
/// testing to properly detect hanging mutations.
///
/// # Panics
///
/// Panics on the calling thread if the wrapped closure does not complete within
/// the timeout. When mutation testing is enabled (`MUTATION_TESTING=1`) the
/// watchdog is disabled and the closure runs directly, so no timeout panic occurs.
///
/// # Example
///
/// ```rust
/// use testing::with_watchdog;
///
/// with_watchdog(|| {
///     // Your test code here
///     assert_eq!(2 + 2, 4);
/// });
/// ```
pub fn with_watchdog<F, R>(test_fn: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    run_with_watchdog(test_fn, |timeout| {
        format!("Test exceeded {}-second timeout", timeout.as_secs())
    })
}

/// Runs a test with a timeout that reports the last active phase.
///
/// Call [`WatchdogPhases::set()`] immediately before each potentially blocking
/// operation. If the closure exceeds the timeout, the panic identifies the last
/// phase it entered.
///
/// # Panics
///
/// Panics on the calling thread if the wrapped closure does not complete within
/// the timeout. Mutation testing disables the timeout.
pub fn with_watchdog_phases<F, R>(initial_phase: &'static str, test_fn: F) -> R
where
    F: FnOnce(WatchdogPhases) -> R + Send + 'static,
    R: Send + 'static,
{
    let (phase_tx, phase_rx) = mpsc::channel();
    run_with_watchdog(
        move || test_fn(WatchdogPhases { phase_tx }),
        move |timeout| phased_timeout_message(initial_phase, &phase_rx, timeout),
    )
}

fn phased_timeout_message(
    initial_phase: &'static str,
    phase_rx: &mpsc::Receiver<&'static str>,
    timeout: Duration,
) -> String {
    let phase = phase_rx.try_iter().last().unwrap_or(initial_phase);
    format!(
        "Test exceeded {}-second timeout during phase: {phase}",
        timeout.as_secs()
    )
}

fn run_with_watchdog<F, R>(test_fn: F, timeout_message: impl FnOnce(Duration) -> String) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    // Check if we are running under mutation testing.
    if std::env::var("MUTATION_TESTING").as_deref() == Ok("1") {
        // Under mutation testing, disable the watchdog to allow hanging mutations.
        return test_fn();
    }

    let (tx, rx) = mpsc::channel();

    // Run the test in a separate thread
    let test_handle = thread::spawn(move || {
        let result = test_fn();
        // Send the result back - if this fails, the receiver has timed out
        drop(tx.send(result));
    });

    // Miri is dramatically slower for thread synchronization, so we use a
    // longer timeout to avoid false positives while still catching real hangs.
    let timeout = if cfg!(miri) {
        Duration::from_mins(1)
    } else {
        Duration::from_secs(10)
    };

    // Wait for either the test to complete or timeout.
    match rx.recv_timeout(timeout) {
        Ok(result) => {
            // Test completed successfully, join the thread to clean up
            test_handle.join().expect("Test thread should not panic");
            result
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // Test timed out - this indicates the test is hanging
            panic!("{}", timeout_message(timeout));
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // Thread panicked, join it to get the panic
            match test_handle.join() {
                Ok(()) => panic!("Test thread disconnected unexpectedly"),
                Err(e) => std::panic::resume_unwind(e),
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(WatchdogPhases: RefUnwindSafe, UnwindSafe);

    #[test]
    fn watchdog_allows_fast_tests() {
        let result = with_watchdog(|| {
            // A test that completes quickly
            42
        });
        assert_eq!(result, 42);
    }

    #[test]
    fn watchdog_returns_correct_value() {
        let result = with_watchdog(|| "hello world");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn phased_watchdog_returns_correct_value() {
        let result = with_watchdog_phases("starting", |phases| {
            phases.set("finishing");
            "hello world"
        });
        assert_eq!(result, "hello world");
    }

    #[test]
    fn phased_timeout_reports_the_initial_phase_before_any_update() {
        let (_phase_tx, phase_rx) = mpsc::channel();
        let message = phased_timeout_message("initial phase", &phase_rx, Duration::ZERO);
        assert!(message.contains("initial phase"));
    }

    #[test]
    fn phased_timeout_reports_the_latest_phase() {
        let (phase_tx, phase_rx) = mpsc::channel();
        phase_tx.send("earlier phase").unwrap();
        phase_tx.send("latest phase").unwrap();

        let message = phased_timeout_message("initial phase", &phase_rx, Duration::ZERO);

        assert!(message.contains("latest phase"));
        assert!(!message.contains("earlier phase"));
        assert!(!message.contains("initial phase"));
    }
}
