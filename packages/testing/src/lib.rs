#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Private helpers for testing and examples in Folo packages.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::*;

/// Runs a test with a 10-second timeout to prevent infinite hangs.
///
/// This function wraps a test closure with a timeout mechanism. If the test
/// takes longer than 10 seconds to complete, the process will be terminated
/// to prevent CI/build systems from hanging.
///
/// When the `MUTATION_TESTING` environment variable is set to "1", the watchdog
/// is disabled and the test function is executed directly. This allows mutation
/// testing to properly detect hanging mutations.
///
/// # Panics
///
/// Panics if the test times out after 10 seconds (when not in mutation testing mode).
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

    // Wait for either the test to complete or timeout
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(result) => {
            // Test completed successfully, join the thread to clean up
            test_handle.join().expect("Test thread should not panic");
            result
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // Test timed out - this indicates the test is hanging
            panic!("Test exceeded 10-second timeout");
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

/// Calculates the difference between two f64 values and considers
/// them equal if the difference is not more than `close_enough`.
///
/// This is a "correctly performed" floating point equality comparison.
#[must_use]
pub fn f64_diff_abs(a: f64, b: f64, close_enough: f64) -> f64 {
    let diff = (a - b).abs();

    if diff <= close_enough { 0.0 } else { diff }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

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
}
