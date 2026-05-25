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

/// Caps iterator consumption so a failure to terminate in `histogram_deltas` surfaces as a
/// fast unit-test failure elsewhere rather than letting this test hang.
const HISTOGRAM_ITER_SAFETY_BOUND: usize = 8;

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

    // First call initializes the bucket storage (this allocates).
    state
        .histogram_deltas(BUCKETS, [5_u64, 12, 8, 3])
        .take(HISTOGRAM_ITER_SAFETY_BOUND)
        .for_each(drop);

    // Subsequent calls must perform no allocations.
    let iterations = 16_u64;
    {
        let _span = op.measure_thread().iterations(iterations);
        for _ in 0..iterations {
            state
                .histogram_deltas(BUCKETS, [7_u64, 9, 11, 4])
                .take(HISTOGRAM_ITER_SAFETY_BOUND)
                .for_each(drop);
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
