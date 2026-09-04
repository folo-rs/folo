//! Benchmarks to measure the compute overhead of `alloc_tracker` logic itself.
//!
//! Two groups, measuring the two places the tracking machinery costs something:
//!
//! * `overhead` benchmarks empty spans — spans that do no work but still pay for
//!   span creation and destruction.
//! * `allocator` benchmarks the [`GlobalAlloc`] paths themselves, running each
//!   operation through the tracking wrapper and through the bare system allocator
//!   so the difference between the pair is the tracking cost.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

/// Representative of an ordinary short-lived object, comfortably inside the size
/// classes that system allocators serve from a thread-local fast path. This is the
/// case where the tracking wrapper's fixed cost is largest relative to the work it
/// wraps.
const SMALL_SIZE: usize = 64;

/// Past the size classes that system allocators serve from their fast path, so the
/// wrapper's fixed cost is measured against a materially more expensive underlying
/// operation.
const LARGE_SIZE: usize = 64 * 1024;

/// Growth target for the reallocation cases. Doubling a small block is the cheapest
/// realistic reallocation, which again maximizes the wrapper's relative share.
const REALLOC_GROWN_SIZE: usize = SMALL_SIZE * 2;

criterion_group!(benches, entrypoint, allocator_paths);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("alloc_tracker_tracking_overhead/overhead");

    // Baseline measurement - no tracking at all
    group.bench_function("baseline_empty", |b| {
        b.iter(|| {
            // Completely empty - just the black_box call
            black_box(());
        });
    });

    // alloc_tracker overhead measurements
    {
        // This bench measures tracking overhead itself, so the session suppresses
        // its own stdout and file output on drop.
        let alloc_session = Session::new().no_stdout().no_file();

        let process_op =
            alloc_session.operation("alloc_tracker_tracking_overhead/overhead/process_span_empty");
        group.bench_function("process_span_empty", |b| {
            b.iter(|| {
                let _span = process_op.measure_process().iterations(1);
                // Empty span - measures only the overhead of span creation/destruction
                black_box(());
            });
        });

        let thread_op =
            alloc_session.operation("alloc_tracker_tracking_overhead/overhead/thread_span_empty");
        group.bench_function("thread_span_empty", |b| {
            b.iter(|| {
                let _span = thread_op.measure_thread().iterations(1);
                // Empty span - measures only the overhead of span creation/destruction
                black_box(());
            });
        });
    }

    group.finish();
}

fn allocator_paths(c: &mut Criterion) {
    let mut group = c.benchmark_group("alloc_tracker_tracking_overhead/allocator");

    // The first tracked allocation on a thread initializes that thread's counters and
    // registers them globally. Pay that one-time cost here so it does not land in the
    // measurements below.
    alloc_dealloc(&ALLOCATOR, layout(SMALL_SIZE));

    group.bench_function("untracked_alloc_dealloc_small", |b| {
        b.iter(|| alloc_dealloc(&System, layout(SMALL_SIZE)));
    });
    group.bench_function("tracked_alloc_dealloc_small", |b| {
        b.iter(|| alloc_dealloc(&ALLOCATOR, layout(SMALL_SIZE)));
    });

    group.bench_function("untracked_alloc_dealloc_large", |b| {
        b.iter(|| alloc_dealloc(&System, layout(LARGE_SIZE)));
    });
    group.bench_function("tracked_alloc_dealloc_large", |b| {
        b.iter(|| alloc_dealloc(&ALLOCATOR, layout(LARGE_SIZE)));
    });

    group.bench_function("untracked_realloc_grow", |b| {
        b.iter(|| alloc_realloc_dealloc(&System, layout(SMALL_SIZE), REALLOC_GROWN_SIZE));
    });
    group.bench_function("tracked_realloc_grow", |b| {
        b.iter(|| alloc_realloc_dealloc(&ALLOCATOR, layout(SMALL_SIZE), REALLOC_GROWN_SIZE));
    });

    group.finish();
}

/// Layout of a `size`-byte block at an alignment an ordinary Rust value would ask for.
fn layout(size: usize) -> Layout {
    Layout::from_size_align(size, align_of::<u64>()).expect("benchmark layout is valid")
}

/// Allocates one block and immediately frees it.
fn alloc_dealloc<A: GlobalAlloc>(allocator: &A, layout: Layout) {
    // SAFETY: `layout` has non-zero size, which is what `GlobalAlloc::alloc` requires.
    let ptr = unsafe { allocator.alloc(layout) };
    assert!(!ptr.is_null(), "benchmark allocation failed");

    black_box(ptr);

    // SAFETY: `ptr` was returned by `alloc` for this same `layout` and has not been freed.
    unsafe {
        allocator.dealloc(ptr, layout);
    }
}

/// Allocates one block, grows it to `new_size`, then frees it.
fn alloc_realloc_dealloc<A: GlobalAlloc>(allocator: &A, layout: Layout, new_size: usize) {
    // SAFETY: `layout` has non-zero size, which is what `GlobalAlloc::alloc` requires.
    let ptr = unsafe { allocator.alloc(layout) };
    assert!(!ptr.is_null(), "benchmark allocation failed");

    // SAFETY: `ptr` was returned by `alloc` for `layout`, and `new_size` is non-zero and
    // small enough that rounding it up to `layout`'s alignment cannot overflow `isize`.
    let grown = unsafe { allocator.realloc(ptr, layout, new_size) };
    assert!(!grown.is_null(), "benchmark reallocation failed");

    black_box(grown);

    let grown_layout =
        Layout::from_size_align(new_size, layout.align()).expect("grown layout is valid");

    // SAFETY: `grown` was returned by `realloc` for `new_size` at `layout`'s alignment.
    unsafe {
        allocator.dealloc(grown, grown_layout);
    }
}
