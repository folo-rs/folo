//! Callgrind benchmarks for the `alloc_tracker` tracking machinery itself.
//!
//! Paired with `alloc_tracker_tracking_overhead.rs`, which measures the same two
//! subgroups on real hardware.
//!
//! # Scope and caveats
//!
//! Callgrind models allocation as essentially free, so these numbers are **not** a
//! measure of what allocation costs. They measure the instruction count of the
//! tracking wrapper that sits in front of the system allocator, which is precisely
//! the quantity this package promises to keep small. Read every `tracked_*` figure
//! against its `untracked_*` sibling: the difference between the pair is the
//! tracking cost, and the absolute values are meaningless on their own.
//!
//! The `allocator` subgroup isolates deallocation and reallocation by allocating in
//! a setup function, which Gungraun evaluates outside the measured region. The cost
//! of allocation alone is the `alloc_dealloc` pair minus the matching `dealloc` case.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "These lints originate in Gungraun macro expansion and cannot be addressed in \
          this benchmark."
    )
)]

use alloc_tracker::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[cfg(not(target_os = "linux"))]
fn main() {
    // Valgrind is Linux-only. On other platforms this bench target compiles
    // to a no-op so `cargo build --all-targets` still works.
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig, main};
#[cfg(target_os = "linux")]
pub use linux::*;

// `--collect-bus=yes` makes Callgrind emit the global bus event (`Ge`), which counts
// lock-prefixed instructions. The tracking counters are per-thread and relaxed, so a
// non-zero `Ge` here would signal an unintended atomic read-modify-write on the hot
// path.
#[cfg(target_os = "linux")]
main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes", "--collect-bus=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    ),
    library_benchmark_groups = [allocator, overhead]
);

#[cfg(target_os = "linux")]
mod linux {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::hint::black_box;
    use std::sync::LazyLock;

    use alloc_tracker::{Operation, Session};
    use gungraun::prelude::*;

    use super::ALLOCATOR;

    /// Representative of an ordinary short-lived object, comfortably inside the size
    /// classes that system allocators serve from a thread-local fast path.
    const SMALL_SIZE: usize = 64;

    /// Past the size classes that system allocators serve from their fast path.
    const LARGE_SIZE: usize = 64 * 1024;

    /// Growth target for the reallocation cases.
    const REALLOC_GROWN_SIZE: usize = SMALL_SIZE * 2;

    /// Layout of a `size`-byte block at an alignment an ordinary Rust value would ask for.
    fn layout(size: usize) -> Layout {
        Layout::from_size_align(size, align_of::<u64>()).expect("benchmark layout is valid")
    }

    /// Allocates one block through `allocator` for a benchmark that frees it.
    fn allocate<A: GlobalAlloc>(allocator: &A, size: usize) -> (*mut u8, Layout) {
        let layout = layout(size);

        // SAFETY: `layout` has non-zero size, which is what `GlobalAlloc::alloc` requires.
        let ptr = unsafe { allocator.alloc(layout) };
        assert!(!ptr.is_null(), "benchmark allocation failed");

        (ptr, layout)
    }

    /// Allocates one block through `allocator` and immediately frees it.
    fn alloc_dealloc<A: GlobalAlloc>(allocator: &A, size: usize) {
        let (ptr, layout) = allocate(allocator, size);

        black_box(ptr);

        // SAFETY: `ptr` was returned by `alloc` for this same `layout` and has not been freed.
        unsafe {
            allocator.dealloc(ptr, layout);
        }
    }

    /// Grows `block` to `REALLOC_GROWN_SIZE` through `allocator`, then frees it.
    fn realloc_dealloc<A: GlobalAlloc>(allocator: &A, block: (*mut u8, Layout)) {
        let (ptr, layout) = block;

        // SAFETY: `ptr` was returned by `alloc` for `layout`, and `REALLOC_GROWN_SIZE` is
        // non-zero and small enough that rounding it up to `layout`'s alignment cannot
        // overflow `isize`.
        let grown = unsafe { allocator.realloc(ptr, layout, REALLOC_GROWN_SIZE) };
        assert!(!grown.is_null(), "benchmark reallocation failed");

        let grown_layout = Layout::from_size_align(REALLOC_GROWN_SIZE, layout.align())
            .expect("grown layout is valid");

        // SAFETY: `grown` was returned by `realloc` for `REALLOC_GROWN_SIZE` at `layout`'s
        // alignment.
        unsafe {
            allocator.dealloc(grown, grown_layout);
        }
    }

    /// Frees `block` through `allocator`.
    fn dealloc<A: GlobalAlloc>(allocator: &A, block: (*mut u8, Layout)) {
        let (ptr, layout) = block;

        // SAFETY: `ptr` was returned by the matching allocator's `alloc` for this same
        // `layout` in the setup function, and has not been freed.
        unsafe {
            allocator.dealloc(ptr, layout);
        }
    }

    #[library_benchmark]
    #[bench::run(SMALL_SIZE)]
    fn allocator_untracked_alloc_dealloc_small(size: usize) {
        alloc_dealloc(&System, black_box(size));
    }

    #[library_benchmark]
    #[bench::run(SMALL_SIZE)]
    fn allocator_tracked_alloc_dealloc_small(size: usize) {
        alloc_dealloc(&ALLOCATOR, black_box(size));
    }

    #[library_benchmark]
    #[bench::run(LARGE_SIZE)]
    fn allocator_untracked_alloc_dealloc_large(size: usize) {
        alloc_dealloc(&System, black_box(size));
    }

    #[library_benchmark]
    #[bench::run(LARGE_SIZE)]
    fn allocator_tracked_alloc_dealloc_large(size: usize) {
        alloc_dealloc(&ALLOCATOR, black_box(size));
    }

    #[library_benchmark]
    #[bench::run(allocate(&System, SMALL_SIZE))]
    fn allocator_untracked_dealloc_small(block: (*mut u8, Layout)) {
        dealloc(&System, black_box(block));
    }

    #[library_benchmark]
    #[bench::run(allocate(&ALLOCATOR, SMALL_SIZE))]
    fn allocator_tracked_dealloc_small(block: (*mut u8, Layout)) {
        dealloc(&ALLOCATOR, black_box(block));
    }

    #[library_benchmark]
    #[bench::run(allocate(&System, SMALL_SIZE))]
    fn allocator_untracked_realloc_grow(block: (*mut u8, Layout)) {
        realloc_dealloc(&System, black_box(block));
    }

    #[library_benchmark]
    #[bench::run(allocate(&ALLOCATOR, SMALL_SIZE))]
    fn allocator_tracked_realloc_grow(block: (*mut u8, Layout)) {
        realloc_dealloc(&ALLOCATOR, black_box(block));
    }

    library_benchmark_group!(
        name = allocator,
        benchmarks = [
            allocator_untracked_alloc_dealloc_small,
            allocator_tracked_alloc_dealloc_small,
            allocator_untracked_alloc_dealloc_large,
            allocator_tracked_alloc_dealloc_large,
            allocator_untracked_dealloc_small,
            allocator_tracked_dealloc_small,
            allocator_untracked_realloc_grow,
            allocator_tracked_realloc_grow
        ]
    );

    /// Session and operations for the span-overhead scenarios.
    ///
    /// The session is deliberately never dropped: it exists only to hand out
    /// operations, and dropping it would emit a report the benchmark has no use for.
    /// Its operation names are workload labels rather than Criterion identifiers,
    /// which `docs/naming.md` permits for a session that is itself under measurement.
    struct Fixture {
        thread_op: Operation,
        process_op: Operation,
        _session: Session,
    }

    static FIXTURE: LazyLock<Fixture> = LazyLock::new(|| {
        let session = Session::new().no_stdout().no_file();

        Fixture {
            thread_op: session.operation("thread_span_empty"),
            process_op: session.operation("process_span_empty"),
            _session: session,
        }
    });

    /// Forces the fixture into existence so its one-time construction, and the
    /// first-touch initialization of this thread's allocation counters, stay outside
    /// the measured region.
    fn fixture() -> &'static Fixture {
        let fixture = &*FIXTURE;
        drop(fixture.thread_op.measure_thread().iterations(1));
        fixture
    }

    #[library_benchmark]
    #[bench::run(fixture())]
    fn overhead_thread_span_empty(fixture: &'static Fixture) {
        let _span = fixture.thread_op.measure_thread().iterations(1);
    }

    #[library_benchmark]
    #[bench::run(fixture())]
    fn overhead_process_span_empty(fixture: &'static Fixture) {
        let _span = fixture.process_op.measure_process().iterations(1);
    }

    library_benchmark_group!(
        name = overhead,
        benchmarks = [overhead_thread_span_empty, overhead_process_span_empty]
    );
}
