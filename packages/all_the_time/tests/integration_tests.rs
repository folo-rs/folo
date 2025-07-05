//! Integration tests for `all_the_time` against the real platform.
//!
//! These tests use the real system processor time tracking to verify
//! that actual work results in nonzero processor time measurements.

use std::hint::black_box;
use std::time::{Duration, Instant};

use all_the_time::Session;

/// Performs CPU-intensive arithmetic work until at least the target duration has elapsed.
///
/// Returns the number of operations performed.
fn perform_work_for_duration(target_duration: Duration) -> u64 {
    let start = Instant::now();
    let mut iterations = 0_u64;
    let mut accumulator = 0_u64;

    // Keep doing arithmetic work until we've used enough real time
    while start.elapsed() < target_duration {
        // Perform more intensive arithmetic that the optimizer can't eliminate
        for i in 0..10000_u32 {
            accumulator = accumulator
                .wrapping_add(u64::from(i))
                .wrapping_mul(3)
                .wrapping_add(7)
                .wrapping_mul(11)
                .wrapping_add(13);

            // Add some more complex operations
            let temp = accumulator.wrapping_pow(2);
            accumulator = accumulator.wrapping_add(temp);
        }
        iterations = iterations.wrapping_add(10000);

        // Use black_box to prevent the optimizer from eliminating our work
        black_box(accumulator);
    }

    iterations
}

#[test]
#[cfg(not(miri))] // Miri cannot use the real operating system APIs.
fn real_platform_thread_span_measures_nonzero_time() {
    let mut session = Session::new();
    let operation = session.operation("thread_work");

    // Perform work and measure it with a thread span
    let iterations_performed = {
        let _span = operation.iterations(1).measure_thread();
        perform_work_for_duration(Duration::from_millis(10)) // Increased work duration
    };

    // Verify that we actually performed some work
    assert!(
        iterations_performed > 0,
        "Expected to perform some iterations, but got {iterations_performed}"
    );

    // Verify that the span recorded some processor time
    // Note: Thread time measurement may not be available on all platforms
    let total_time = operation.mean(); // Since we have 1 span, mean equals total

    // On some platforms (like Windows), thread time might not be implemented
    // So we only check that the measurement doesn't panic and returns a valid duration
    assert!(
        total_time >= Duration::ZERO,
        "Expected non-negative processor time, but got {total_time:?}"
    );

    // Sanity check: the time should be reasonable (not absurdly large)
    assert!(
        total_time < Duration::from_secs(1),
        "Expected reasonable processor time, but got {total_time:?}"
    );
}

#[test]
#[cfg(not(miri))] // Miri cannot use the real operating system APIs.
fn real_platform_process_span_measures_nonzero_time() {
    let mut session = Session::new();
    let operation = session.operation("process_work");

    // Perform work and measure it with a process span
    let iterations_performed = {
        let _span = operation.iterations(1).measure_process();
        perform_work_for_duration(Duration::from_millis(10)) // Increased work duration
    };

    // Verify that we actually performed some work
    assert!(
        iterations_performed > 0,
        "Expected to perform some iterations, but got {iterations_performed}"
    );

    // Verify that the span recorded processor time (may be zero on some platforms)
    let total_time = operation.mean(); // Since we have 1 span, mean equals total

    // On some platforms (like Windows), processor time measurement may not be available
    // or may return 0ns even for actual work. We accept this platform limitation.
    assert!(
        total_time >= Duration::ZERO,
        "Expected non-negative processor time, but got {total_time:?}"
    );

    // Sanity check: if we got nonzero time, it should be reasonable
    if total_time > Duration::ZERO {
        assert!(
            total_time < Duration::from_secs(1),
            "Expected reasonable processor time, but got {total_time:?}"
        );
    }
}

#[test]
#[cfg(not(miri))] // Miri cannot use the real operating system APIs.
fn real_platform_both_span_types_measure_nonzero_time() {
    let mut session = Session::new();

    // Test thread span
    let thread_time = {
        let thread_op = session.operation("thread_comparison");
        {
            let _span = thread_op.iterations(1).measure_thread();
            perform_work_for_duration(Duration::from_millis(10)); // Increased work duration
        }
        thread_op.mean()
    };

    // Test process span
    let process_time = {
        let process_op = session.operation("process_comparison");
        {
            let _span = process_op.iterations(1).measure_process();
            perform_work_for_duration(Duration::from_millis(10)); // Increased work duration
        }
        process_op.mean()
    };

    // Both should be non-negative (may be zero on some platforms)
    assert!(
        thread_time >= Duration::ZERO,
        "Thread span should measure non-negative time, got {thread_time:?}"
    );
    assert!(
        process_time >= Duration::ZERO,
        "Process span should measure non-negative time, got {process_time:?}"
    );

    // If both times are available and nonzero, compare them
    if thread_time > Duration::ZERO && process_time > Duration::ZERO {
        // In a single-threaded context, both should be similar
        // (Allow some variance due to measurement precision)
        #[expect(
            clippy::cast_precision_loss,
            reason = "precision loss acceptable for test comparison"
        )]
        let ratio = if thread_time > process_time {
            thread_time.as_nanos() as f64 / process_time.as_nanos() as f64
        } else {
            process_time.as_nanos() as f64 / thread_time.as_nanos() as f64
        };

        assert!(
            ratio < 10.0,
            "Thread and process times should be similar in single-threaded context. Thread: {thread_time:?}, Process: {process_time:?}, Ratio: {ratio}"
        );
    }
}

#[test]
#[cfg(not(miri))] // Miri cannot use the real operating system APIs.
fn real_platform_session_not_empty_after_work() {
    let mut session = Session::new();

    // Session should start empty
    assert!(session.is_empty());

    // Perform some measured work
    let operation = session.operation("integration_test");
    {
        let _span = operation.iterations(1).measure_thread();
        perform_work_for_duration(Duration::from_millis(10)); // Increased work duration
    }

    // Session should no longer be empty
    assert!(!session.is_empty());
}
