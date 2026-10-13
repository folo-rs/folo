//! Process-isolated coverage of production logging, including formatting and cleanup.

use testing::{TracingCapture, with_watchdog};

testing::set_allocator!();

/// Owns cross-thread capture while public-API scenarios exercise lifecycle and failure diagnostics.
#[test]
fn formats_lifecycle_and_failure_events() {
    let capture = TracingCapture::start();
    with_watchdog(move || {
        lifecycle::exercise(&capture);
        failures::worker_failure(&capture);
        failures::shutdown_during_unwind(&capture);
    });
}

mod failures;
mod fixtures;
mod lifecycle;
