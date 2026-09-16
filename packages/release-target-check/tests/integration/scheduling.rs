//! Schedules external-I/O cases for both libtest and nextest without changing their watchdogs.

#[cfg(windows)]
use std::sync::{Mutex, PoisonError};

use testing::with_watchdog;

// All I/O cases live in this binary. Nextest supplies process-level isolation through its group;
// libtest needs this in-process slot. See docs/implementation.md, "Verification boundary tests".
#[cfg(windows)]
static IO_TEST: Mutex<()> = Mutex::new(());

pub(crate) fn with_io_test(test: impl FnOnce() + Send + 'static) {
    // Scheduling is not test execution: a queued case must not consume its watchdog budget.
    // The lock protects no data, so a failed test cannot invalidate the next fixture.
    #[cfg(windows)]
    let _slot = IO_TEST.lock().unwrap_or_else(PoisonError::into_inner);

    with_watchdog(test);
}
