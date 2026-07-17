//! Integration tests for `all_the_time` against the real platform.
//!
//! These tests verify that significant CPU work results in measurable
//! processor time. All tests require non-zero measurements to pass.

use std::hint::black_box;
use std::time::Duration;

use all_the_time::Session;
use cpu_time::ThreadTime;

/// Processor time the measurable work loop consumes before it stops.
///
/// The value sits far above both the tests' 1 ms assertion floor and the coarse
/// (~15.6 ms) resolution of the Windows processor-time clocks, so the recorded
/// measurement stays comfortably non-zero even after quantization.
const WORK_PROCESSOR_TIME_TARGET: Duration = Duration::from_millis(100);

/// Performs intensive CPU work that reliably registers as processor time.
///
/// The loop keeps working until the current thread has consumed
/// [`WORK_PROCESSOR_TIME_TARGET`] of processor time. Bounding on consumed
/// processor time rather than wall-clock time keeps the amount of *measurable*
/// work constant regardless of how the operating system schedules this thread,
/// so a heavily loaded host cannot let real time elapse without the work being
/// counted.
///
/// Thread processor time is a subset of process-wide processor time, so reaching
/// the target on this thread guarantees the process-wide clock has advanced by at
/// least as much, making the bound valid for both thread and process spans.
///
/// Returns the number of operations performed.
fn perform_measurable_cpu_work() -> u64 {
    let start = ThreadTime::now();
    let mut iterations = 0_u64;
    let mut accumulator = 0_u64;

    loop {
        // Intensive arithmetic that cannot be optimized away.
        for i in 0..50_000_u32 {
            accumulator = accumulator
                .wrapping_add(u64::from(i))
                .wrapping_mul(3)
                .wrapping_add(7)
                .wrapping_mul(11)
                .wrapping_add(13);

            let temp = accumulator.wrapping_pow(2);
            accumulator = accumulator.wrapping_add(temp);

            accumulator = accumulator.rotate_left(1).wrapping_sub(i.into());
        }
        iterations = iterations.wrapping_add(50_000);
        black_box(accumulator);

        if start.elapsed() >= WORK_PROCESSOR_TIME_TARGET {
            break;
        }
    }

    iterations
}

/// Reads an operation's per-iteration processor time the way a user inspects
/// results: through the session report rather than the live `Operation` handle.
fn report_processor_time(session: &Session, operation_name: &str) -> Duration {
    session
        .to_report()
        .operations()
        .find(|&(name, _)| name == operation_name)
        .map_or_else(
            || panic!("operation {operation_name:?} should appear in the report"),
            |(_, op)| {
                op.processor_time()
                    .expect("operation with recorded spans has an estimable per-iteration time")
            },
        )
}

#[test]
#[cfg_attr(miri, ignore)] // Miri cannot use the real operating system APIs.
fn real_platform_thread_span_measures_nonzero_time() {
    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("thread_work");

    // Perform significant CPU work and measure it with a thread span
    let iterations_performed = {
        let _span = operation.measure_thread().iterations(1);
        perform_measurable_cpu_work()
    };

    // Verify that we actually performed substantial work
    assert!(
        iterations_performed > 0,
        "Expected to perform substantial work, but only got {iterations_performed} iterations"
    );

    // Verify that the span recorded measurable processor time
    let total_time = report_processor_time(&session, "thread_work");

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
    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("process_work");

    // Perform significant CPU work and measure it with a process span
    let iterations_performed = {
        let _span = operation.measure_process().iterations(1);
        perform_measurable_cpu_work()
    };

    // Verify that we actually performed real work
    assert!(
        iterations_performed >= 50_000_u64,
        "Expected to perform real work, but only got {iterations_performed} iterations"
    );

    // Verify that the span recorded measurable processor time
    let total_time = report_processor_time(&session, "process_work");

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
    let session = Session::new().no_stdout().no_file();

    // Session should start empty
    assert!(session.is_empty());

    // Perform some measured intensive work
    {
        let operation = session.operation("integration_test");
        let _span = operation.measure_thread().iterations(1);
        perform_measurable_cpu_work();
    }
    let measured_time = report_processor_time(&session, "integration_test");

    // Session should no longer be empty
    assert!(!session.is_empty());

    // And the measurement should be meaningful
    assert!(
        measured_time > Duration::ZERO,
        "Expected measurable time for intensive work, got {measured_time:?}"
    );
}
