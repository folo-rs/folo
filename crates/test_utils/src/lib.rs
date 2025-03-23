use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    process,
    sync::mpsc,
    thread,
    time::Duration,
};

/// We expect tests to be very fast (sub-second), so this is a very generous timeout.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Executes a thread-safe function on a background thread and abandons it if
/// it does not complete before the provided timeout.
#[must_use]
pub fn execute_or_abandon<F, R>(f: F) -> Option<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (sender, receiver) = mpsc::channel();

    // There are multiple ways for the called function to fail:
    // 1. It fails to finish in the allowed time span.
    // 2. It panics, so the result is never sent.
    //
    // In both cases, the channel will get closed and recv_timeout
    // will signal an error saying the channel is broken.
    thread::spawn(move || {
        let result = f();
        sender.send(result).unwrap();
    });

    match receiver.recv_timeout(TEST_TIMEOUT) {
        Ok(result) => Some(result),
        Err(_) => None,
    }
}

/// Executes a function on the current thread and sets up a watchdog timer that terminates the
/// process it the target function does not complete before the provided timeout. This is a variant
/// of `execute_or_abandon()` that can be used with single-threaded logic that does not support being
/// moved to a background thread.
pub fn execute_or_terminate_process<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let (sender, receiver) = mpsc::channel();

    let watchdog = thread::spawn(move || match receiver.recv_timeout(TEST_TIMEOUT) {
        Ok(()) => {}
        Err(_) => {
            eprintln!("Test timed out, terminating process.");
            #[allow(clippy::exit)]
            process::exit(911);
        }
    });

    let result = catch_unwind(AssertUnwindSafe(f));

    // We signal "done" no matter whether it panics or succeeds, all we care about is timeout.
    sender.send(()).unwrap();

    // We must wait for this to finish, otherwise Miri leak detection will be angry at us.
    watchdog.join().unwrap();

    // This will re-raise any panic if one occurred.
    result.unwrap()
}
