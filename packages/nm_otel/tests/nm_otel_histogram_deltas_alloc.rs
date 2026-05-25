//! Allocation assertion test for the histogram delta computation path.
//!
//! Verifies that [`EventState::histogram_deltas`] performs no heap allocations on the
//! steady-state path (after the first call has sized the bucket storage). This locks in
//! the streaming refactor that replaced two intermediate `Vec` allocations per event.

use alloc_tracker::{Allocator, Session};
use nm_otel::__private::EventState;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

const BUCKETS: [i64; 4] = [10, 50, 100, 500];

#[test]
#[cfg_attr(
    miri,
    ignore = "uses a custom #[global_allocator]; miri's runtime allocator instrumentation \
              conflicts with the alloc_tracker probe"
)]
fn histogram_deltas_does_not_allocate_on_steady_state() {
    let session = Session::new();
    let op = session.operation("histogram_deltas_steady_state");

    let mut state = EventState::default();

    // First call initializes the bucket storage (this allocates the bucket Vec).
    // Cumulative: [5, 17, 25, 28]. First-call deltas equal cumulative values.
    let expected_first: [(i64, u64, u64); 4] =
        [(10, 5, 5), (50, 17, 17), (100, 25, 25), (500, 28, 28)];
    assert!(
        state
            .histogram_deltas(BUCKETS, [5_u64, 12, 8, 3])
            .eq(expected_first)
    );

    // Steady-state input is the same on every iteration, so:
    //  * iteration 1 sees cumulative [7, 16, 27, 31] against previous [5, 17, 25, 28]
    //    (note the saturating subtraction at bucket 1: 16 - 17 saturates to 0);
    //  * iterations 2+ see cumulative [7, 16, 27, 31] against previous [7, 16, 27, 31],
    //    so all deltas are zero.
    let expected_steady_first: [(i64, u64, u64); 4] =
        [(10, 7, 2), (50, 16, 0), (100, 27, 2), (500, 31, 3)];
    let expected_steady_subsequent: [(i64, u64, u64); 4] =
        [(10, 7, 0), (50, 16, 0), (100, 27, 0), (500, 31, 0)];

    // `Iterator::eq` against a finite expected array bounds consumption to
    // `expected.len() + 1` calls to `self.next()` and is itself allocation-free.
    // A hypothetical mutation that broke iteration termination would surface here
    // as an assertion failure rather than a test hang.
    let iterations = 16_u64;
    {
        let _span = op.measure_thread().iterations(iterations);
        for i in 0..iterations {
            let expected = if i == 0 {
                expected_steady_first
            } else {
                expected_steady_subsequent
            };
            assert!(
                state
                    .histogram_deltas(BUCKETS, [7_u64, 9, 11, 4])
                    .eq(expected)
            );
        }
    }

    let report = session.to_report();
    let operations: Vec<_> = report.operations().collect();
    let (_name, stats) = operations
        .iter()
        .find(|(name, _)| *name == "histogram_deltas_steady_state")
        .expect("operation should have been recorded");

    assert_eq!(
        stats.total_bytes_allocated(),
        0,
        "steady-state histogram_deltas must not allocate; allocated \
         {} bytes across {} iterations",
        stats.total_bytes_allocated(),
        stats.total_iterations()
    );
}
