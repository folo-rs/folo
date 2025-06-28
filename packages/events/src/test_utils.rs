//! Testing utilities for the events crate.
//!
//! This module provides common testing utilities used across multiple test modules
//! to avoid code duplication.

#[cfg(test)]
use std::sync::mpsc;
#[cfg(test)]
use std::thread;
#[cfg(test)]
use std::time::Duration;

/// Runs a test with a 10-second timeout to prevent infinite hangs.
/// If the test does not complete within 10 seconds, the function will panic.
#[cfg(test)]
pub(crate) fn with_watchdog<F, R>(test_fn: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
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
            panic!("Test exceeded 10-second timeout - likely hanging in recv()");
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
