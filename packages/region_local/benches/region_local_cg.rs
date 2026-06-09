//! Callgrind benchmarks for the `region_local` crate.
//!
//! Paired with `region_local.rs` (the `read` and `write` Criterion subgroups)
//! which covers the same operations under wall-clock measurement plus
//! multi-threaded and cross-region contention scenarios.
//!
//! Scenarios isolate the per-call cost of region-local reads and
//! writes so each can be tracked at instruction-level granularity:
//!
//! * `read_get_local_first_touch` — first read; initializes the per-thread
//!   snapshot and the region-aware infrastructure.
//! * `read_get_local_warm` — subsequent read; hits the cached snapshot.
//! * `read_with_local_warm` — closure-form read on a warm snapshot.
//! * `read_std_thread_local_get_warm` — `thread_local!` `Cell` baseline.
//! * `write_set_local_first_touch` — first write; pays the same one-shot
//!   initialization cost as the first read.
//! * `write_set_local_warm` — subsequent write; updates the per-region
//!   storage without paying initialization cost.
//!
//! Multi-threaded contention is intentionally out of scope; Callgrind
//! cannot model cache-coherence traffic meaningfully, and the
//! Criterion bench already covers cross-thread and cross-region paths.

#![allow(
    missing_docs,
    reason = "no need for API documentation on benchmark code"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Tracking issue drafts live at \
          c:/Source/gungraun-lint-issues/ pending upstream filing."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Valgrind is Linux-only.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::cell::Cell;
    use std::hint::black_box;

    use gungraun::prelude::*;
    use region_local::{RegionLocalCopyExt as _, RegionLocalExt as _, region_local};

    // One static per scenario keeps each bench's first-touch behavior
    // isolated even if gungraun's per-subprocess execution model changes.
    region_local!(static FIRST_TOUCH_VALUE: u32 = 99942);

    region_local!(static WARM_VALUE: u32 = 99942);

    region_local!(static WITH_VALUE: u32 = 99942);

    region_local!(static SET_FIRST_TOUCH_VALUE: u32 = 99942);

    region_local!(static SET_WARM_VALUE: u32 = 99942);

    thread_local! {
        static STD_VALUE: Cell<u32> = const { Cell::new(99942) };
    }

    fn warm_get_local() {
        _ = WARM_VALUE.get_local();
    }

    fn warm_with_local() {
        WITH_VALUE.with_local(|v| {
            _ = *v;
        });
    }

    fn warm_set_local() {
        SET_WARM_VALUE.set_local(1);
    }

    fn warm_std_thread_local() {
        _ = STD_VALUE.with(Cell::get);
    }

    // ---------- Read paths ----------

    #[library_benchmark]
    fn read_get_local_first_touch() -> u32 {
        FIRST_TOUCH_VALUE.get_local()
    }

    #[library_benchmark]
    #[bench::warm(warm_get_local())]
    fn read_get_local_warm(_: ()) -> u32 {
        WARM_VALUE.get_local()
    }

    #[library_benchmark]
    #[bench::warm(warm_with_local())]
    fn read_with_local_warm(_: ()) -> u32 {
        WITH_VALUE.with_local(|v| *v)
    }

    // ---------- Write path ----------

    #[library_benchmark]
    fn write_set_local_first_touch() {
        SET_FIRST_TOUCH_VALUE.set_local(black_box(566));
    }

    #[library_benchmark]
    #[bench::warm(warm_set_local())]
    fn write_set_local_warm(_: ()) {
        SET_WARM_VALUE.set_local(black_box(566));
    }

    // ---------- Comparison baseline (logically part of `read`) ----------

    #[library_benchmark]
    #[bench::warm(warm_std_thread_local())]
    fn read_std_thread_local_get_warm(_: ()) -> u32 {
        STD_VALUE.with(Cell::get)
    }

    library_benchmark_group!(
        name = read,
        benchmarks = [
            read_get_local_first_touch,
            read_get_local_warm,
            read_with_local_warm,
            read_std_thread_local_get_warm,
        ]
    );

    library_benchmark_group!(
        name = write,
        benchmarks = [write_set_local_first_touch, write_set_local_warm]
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{read, write};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = read, write
);
