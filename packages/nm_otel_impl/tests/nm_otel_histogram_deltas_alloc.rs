//! Allocation assertion test for the histogram delta computation path.
//!
//! Verifies that [`EventState::histogram_deltas`] performs no heap allocations on the
//! warm path after the first call has sized the bucket storage.

use alloc_tracker::{Allocator, Session};
use nm_otel_impl::EventState;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

// Multiple bounds exercise cumulative conversion and retained per-bucket state.
const HISTOGRAM_BUCKET_BOUNDS: [i64; 4] = [10, 50, 100, 500];
// A stable name allows the allocation report to identify the measured operation.
const OPERATION_NAME: &str = "histogram_deltas_steady_state";
// Repetition makes incidental fixed-cost allocations visible in the aggregate report.
const MEASURED_EXPORT_COUNT: u64 = 16;
// The first measured export compares against initializing state rather than warm state.
const FIRST_MEASURED_EXPORT_INDEX: u64 = 0;
// The warm-path allocation contract permits no allocator activity.
const NO_ALLOCATED_BYTES: u64 = 0;

#[test]
#[cfg_attr(
    miri,
    ignore = "The custom global allocator conflicts with Miri's runtime allocator \
              instrumentation."
)]
fn histogram_deltas_does_not_allocate_on_steady_state() {
    let session = Session::new().no_stdout().no_file();
    let op = session.operation(OPERATION_NAME);

    let mut state = EventState::default();

    // Drive the initializing path outside the allocation measurement.
    let expected_first: [(i64, u64, u64); 4] =
        [(10, 5, 5), (50, 17, 17), (100, 25, 25), (500, 28, 28)];
    assert!(
        state
            .histogram_deltas(HISTOGRAM_BUCKET_BOUNDS, [5_u64, 12, 8, 3])
            .eq(expected_first)
    );

    // The first measured export mixes positive and saturating deltas. Reusing the same
    // cumulative input then exercises the zero-delta warm path.
    let expected_steady_first: [(i64, u64, u64); 4] =
        [(10, 7, 2), (50, 16, 0), (100, 27, 2), (500, 31, 3)];
    let expected_steady_subsequent: [(i64, u64, u64); 4] =
        [(10, 7, 0), (50, 16, 0), (100, 27, 0), (500, 31, 0)];

    // A finite expected sequence makes incorrect termination fail without a timeout and
    // keeps the assertion itself allocation-free.
    {
        let _span = op.measure_thread().iterations(MEASURED_EXPORT_COUNT);
        for export_index in 0..MEASURED_EXPORT_COUNT {
            let expected = if export_index == FIRST_MEASURED_EXPORT_INDEX {
                expected_steady_first
            } else {
                expected_steady_subsequent
            };
            assert!(
                state
                    .histogram_deltas(HISTOGRAM_BUCKET_BOUNDS, [7_u64, 9, 11, 4])
                    .eq(expected)
            );
        }
    }

    let report = session.to_report();
    let operations: Vec<_> = report.operations().collect();
    let (_, stats) = operations
        .iter()
        .find(|(name, _)| *name == OPERATION_NAME)
        .unwrap();

    assert_eq!(stats.total_bytes_allocated(), NO_ALLOCATED_BYTES);
}
