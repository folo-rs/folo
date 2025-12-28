//! Integration tests for `all_the_time` against the real platform.
//!
//! These tests verify that significant CPU work results in measurable
//! processor time. All tests require non-zero measurements to pass.

use std::hint::black_box;
use std::time::{Duration, Instant};

use all_the_time::Session;

/// Performs intensive CPU work that should be measurable as processor time.
///
/// This function performs enough work to ensure reliable measurement
/// on any platform that supports processor time tracking.
///
/// Returns the number of operations performed.
fn perform_measurable_cpu_work() -> u64 {
    let start = Instant::now();
    let mut iterations = 0_u64;
    let mut accumulator = 0_u64;

    // Perform intensive work for at least 50ms of real time
    // This should be easily measurable as processor time
    while start.elapsed() < Duration::from_millis(50) {
        // Intensive arithmetic that cannot be optimized away
        for i in 0..50_000_u32 {
            accumulator = accumulator
                .wrapping_add(u64::from(i))
                .wrapping_mul(3)
                .wrapping_add(7)
                .wrapping_mul(11)
                .wrapping_add(13);

            let temp = accumulator.wrapping_pow(2);
            accumulator = accumulator.wrapping_add(temp);

            // Additional complex operations
            accumulator = accumulator.rotate_left(1).wrapping_sub(i.into());
        }
        iterations = iterations.wrapping_add(50000);
        black_box(accumulator);
    }

    iterations
}

#[test]
#[cfg_attr(miri, ignore)] // Miri cannot use the real operating system APIs.
fn real_platform_thread_span_measures_nonzero_time() {
    let session = Session::new();
    let operation = session.operation("thread_work");

    // Perform significant CPU work and measure it with a thread span
    let iterations_performed = {
        let _span = operation.measure_thread();
        perform_measurable_cpu_work()
    };

    // Verify that we actually performed substantial work
    assert!(
        iterations_performed > 0,
        "Expected to perform substantial work, but only got {iterations_performed} iterations"
    );

    // Verify that the span recorded measurable processor time
    let total_time = operation.mean();

    // With 50ms+ of intensive work, we must get a non-zero measurement
    assert!(
        total_time > Duration::ZERO,
        "Expected measurable processor time for intensive work, but got {total_time:?}"
    );

    // Sanity check: the time should be reasonable
    assert!(
        total_time >= Duration::from_millis(1),
        "Expected at least 1ms for intensive work, but got {total_time:?}"
    );
    assert!(
        total_time < Duration::from_secs(50),
        "Expected reasonable processor time, but got {total_time:?}"
    );
}

#[test]
#[cfg_attr(miri, ignore)] // Miri cannot use the real operating system APIs.
fn real_platform_process_span_measures_nonzero_time() {
    let session = Session::new();
    let operation = session.operation("process_work");

    // Perform significant CPU work and measure it with a process span
    let iterations_performed = {
        let _span = operation.measure_process();
        perform_measurable_cpu_work()
    };

    // Verify that we actually performed real work
    assert!(
        iterations_performed >= 50_000_u64,
        "Expected to perform real work, but only got {iterations_performed} iterations"
    );

    // Verify that the span recorded measurable processor time
    let total_time = operation.mean();

    // With 50ms+ of intensive work, we must get a non-zero measurement
    assert!(
        total_time > Duration::ZERO,
        "Expected measurable processor time for intensive work, but got {total_time:?}"
    );

    // Sanity check: the time should be reasonable
    assert!(
        total_time >= Duration::from_millis(1),
        "Expected at least 1ms for intensive work, but got {total_time:?}"
    );
    assert!(
        total_time < Duration::from_secs(5),
        "Expected reasonable processor time, but got {total_time:?}"
    );
}

#[test]
#[cfg_attr(miri, ignore)] // Miri cannot use the real operating system APIs.
fn real_platform_session_not_empty_after_work() {
    let session = Session::new();

    // Session should start empty
    assert!(session.is_empty());

    // Perform some measured intensive work
    let measured_time = {
        let operation = session.operation("integration_test");
        {
            let _span = operation.measure_thread();
            perform_measurable_cpu_work();
        }
        operation.mean()
    };

    // Session should no longer be empty
    assert!(!session.is_empty());

    // And the measurement should be meaningful
    assert!(
        measured_time > Duration::ZERO,
        "Expected measurable time for intensive work, got {measured_time:?}"
    );
}
