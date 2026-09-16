//! Schedules external-I/O cases for both libtest and nextest without changing their watchdogs.

#[cfg(any(windows, test))]
use std::sync::Mutex;

use testing::with_watchdog;

// All I/O cases live in this binary. Nextest supplies process-level isolation through its group;
// libtest needs this in-process slot. See docs/implementation.md, "Verification boundary tests".
#[cfg(windows)]
static IO_TEST: Mutex<()> = Mutex::new(());

pub(crate) fn with_io_test(test: impl FnOnce() + Send + 'static) {
    #[cfg(windows)]
    with_slot(&IO_TEST, || with_watchdog(test));

    #[cfg(not(windows))]
    with_watchdog(test);
}

#[cfg(any(windows, test))]
fn with_slot(slot: &Mutex<()>, run: impl FnOnce()) {
    // Scheduling is not test execution: a queued case must not consume its watchdog budget.
    // A watchdog panic can leave its worker running. Poisoning prevents new I/O from overlapping
    // that worker without waiting for a possibly hung worker to release an owned permit.
    let _slot = slot
        .lock()
        .expect("an earlier I/O test failed; execution cannot safely continue");

    run();
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use testing::assert_panics;

    use super::*;

    #[test]
    fn successful_execution_releases_the_slot() {
        with_watchdog(|| {
            let slot = Mutex::new(());
            let calls = RefCell::new(Vec::new());
            with_slot(&slot, || calls.borrow_mut().push("first"));
            with_slot(&slot, || calls.borrow_mut().push("second"));
            assert_eq!(*calls.borrow(), ["first", "second"]);
        });
    }

    #[test]
    fn failed_execution_prevents_later_io() {
        with_watchdog(|| {
            let slot = Mutex::new(());
            // Simulate the caller-side failure directly, without waiting for a real deadline.
            assert_panics(|| with_slot(&slot, || panic!("watchdog caller failed")));

            let started = Cell::new(false);
            assert_panics(|| with_slot(&slot, || started.set(true)));
            assert!(!started.get());
        });
    }
}
