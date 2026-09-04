//! Benchmarks measuring the compute overhead of `alloc_tracker` logic itself.
//!
//! The `overhead` subgroup benchmarks empty spans — spans that do no work but still pay for
//! span creation and destruction. The `allocator` subgroup benchmarks the [`GlobalAlloc`]
//! paths themselves, running each operation through the tracking wrapper and through the
//! bare system allocator so the difference between the pair is the tracking cost.
//!
//! # Scenario contract with the Callgrind benchmarks
//!
//! `alloc_tracker_tracking_overhead_cg.rs` measures these same scenarios as instruction
//! counts. A figure from one file only says anything about the same-named figure in the
//! other if both describe the same workload, so this file is the authority on what a
//! scenario means and the Callgrind file follows it:
//!
//! * Block sizes are the ones the constants below name. The Callgrind file repeats them
//!   because the two benchmarks are separate binaries, so a change here must be made there
//!   as well.
//! * A `tracked_*` scenario measures the steady state, with this thread's counters already
//!   created and registered. This file primes them once before the group; the Callgrind
//!   file isolates every benchmark in its own process and so primes them per scenario.
//! * A Callgrind body measures only what its name says, because Gungraun evaluates the
//!   setup expression outside the collected region. Its `realloc_grow` case therefore
//!   covers the reallocation and the release but not the initial allocation, which the
//!   timing here necessarily includes. Compare figures within a file, not across the two.
//! * The Callgrind file may isolate variants that have no counterpart here, which
//!   `docs/naming.md` permits.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::alloc::{GlobalAlloc, Layout, handle_alloc_error};
use std::hint::black_box;

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

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

criterion_group!(benches, span_overhead, allocator_overhead);
criterion_main!(benches);

fn span_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("alloc_tracker_tracking_overhead/overhead");

    // What an `iter` closure costs before any span is involved, so the span figures below
    // can be read against it.
    group.bench_function("baseline_empty", |b| {
        b.iter(|| {
            black_box(());
        });
    });

    {
        // This bench measures tracking overhead itself, so the session suppresses
        // its own stdout and file output on drop.
        let alloc_session = Session::new().no_stdout().no_file();

        let process_op =
            alloc_session.operation("alloc_tracker_tracking_overhead/overhead/process_span_empty");
        group.bench_function("process_span_empty", |b| {
            b.iter(|| {
                let _span = process_op.measure_process().iterations(1);
                black_box(());
            });
        });

        let thread_op =
            alloc_session.operation("alloc_tracker_tracking_overhead/overhead/thread_span_empty");
        group.bench_function("thread_span_empty", |b| {
            b.iter(|| {
                let _span = thread_op.measure_thread().iterations(1);
                black_box(());
            });
        });
    }

    group.finish();
}

fn allocator_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("alloc_tracker_tracking_overhead/allocator");

    // The first tracked allocation on a thread initializes that thread's counters and
    // registers them globally. Pay that one-time cost here so it does not land in the
    // measurements below.
    alloc_dealloc(&ALLOCATOR, layout(SMALL_SIZE));

    group.bench_function("untracked_alloc_dealloc_small", |b| {
        b.iter(|| alloc_dealloc(&std::alloc::System, layout(SMALL_SIZE)));
    });
    group.bench_function("tracked_alloc_dealloc_small", |b| {
        b.iter(|| alloc_dealloc(&ALLOCATOR, layout(SMALL_SIZE)));
    });

    group.bench_function("untracked_alloc_dealloc_large", |b| {
        b.iter(|| alloc_dealloc(&std::alloc::System, layout(LARGE_SIZE)));
    });
    group.bench_function("tracked_alloc_dealloc_large", |b| {
        b.iter(|| alloc_dealloc(&ALLOCATOR, layout(LARGE_SIZE)));
    });

    group.bench_function("untracked_realloc_grow", |b| {
        b.iter(|| {
            alloc_realloc_dealloc(&std::alloc::System, layout(SMALL_SIZE), REALLOC_GROWN_SIZE);
        });
    });
    group.bench_function("tracked_realloc_grow", |b| {
        b.iter(|| alloc_realloc_dealloc(&ALLOCATOR, layout(SMALL_SIZE), REALLOC_GROWN_SIZE));
    });

    group.finish();
}

/// Layout of a `size`-byte block at an alignment an ordinary Rust value would ask for.
fn layout(size: usize) -> Layout {
    Layout::from_size_align(size, align_of::<u64>()).expect(
        "a type's alignment is a non-zero power of two and the benchmark sizes are orders of \
         magnitude below the layout size limit",
    )
}

fn alloc_dealloc<A: GlobalAlloc>(allocator: &A, layout: Layout) {
    // SAFETY: `layout` has non-zero size, which is what `GlobalAlloc::alloc` requires.
    let ptr = unsafe { allocator.alloc(layout) };

    if ptr.is_null() {
        handle_alloc_error(layout);
    }

    black_box(ptr);

    // SAFETY: `ptr` was returned by `alloc` for this same `layout` and has not been freed.
    unsafe {
        allocator.dealloc(ptr, layout);
    }
}

fn alloc_realloc_dealloc<A: GlobalAlloc>(allocator: &A, layout: Layout, new_size: usize) {
    // SAFETY: `layout` has non-zero size, which is what `GlobalAlloc::alloc` requires.
    let ptr = unsafe { allocator.alloc(layout) };

    if ptr.is_null() {
        handle_alloc_error(layout);
    }

    let grown_layout = Layout::from_size_align(new_size, layout.align()).expect(
        "the alignment already came from a valid layout and the grown benchmark size is \
         orders of magnitude below the layout size limit",
    );

    // SAFETY: `ptr` was returned by `alloc` for `layout`, and `new_size` is non-zero and
    // small enough that rounding it up to `layout`'s alignment cannot overflow `isize`.
    let grown = unsafe { allocator.realloc(ptr, layout, new_size) };

    if grown.is_null() {
        // The original block is still live, but a benchmark that cannot obtain memory has
        // nothing left to measure, and `handle_alloc_error` does not return.
        handle_alloc_error(grown_layout);
    }

    black_box(grown);

    // SAFETY: `grown` was returned by `realloc` for `new_size` at `layout`'s alignment.
    unsafe {
        allocator.dealloc(grown, grown_layout);
    }
}
