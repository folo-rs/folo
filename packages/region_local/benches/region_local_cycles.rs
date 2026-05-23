//! Cycle-accurate benchmarks for the `region_local` crate.
//!
//! Paired with `region_local.rs` (the `region_local` Criterion group)
//! which covers the same operations under wall-clock measurement plus
//! multi-threaded and cross-region contention scenarios.
//!
//! Scenarios isolate the per-call cost of region-local reads and
//! writes so each can be tracked at instruction-level granularity:
//!
//! * `get_local_first_touch` — first read; initializes the per-thread
//!   snapshot and the region-aware infrastructure.
//! * `get_local_warm` — subsequent read; hits the cached snapshot.
//! * `with_local_warm` — closure-form read on a warm snapshot.
//! * `set_local_first_touch` — first write; pays the same one-shot
//!   initialization cost as the first read.
//! * `set_local_warm` — subsequent write; updates the per-region
//!   storage without paying initialization cost.
//! * `std_thread_local_get_warm` — `thread_local!` `Cell` baseline.
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
    allow(
        clippy::absolute_paths,
        clippy::allow_attributes_without_reason,
        clippy::exhaustive_structs,
        clippy::partial_pub_fields,
        clippy::pub_underscore_fields,
        clippy::cognitive_complexity,
        clippy::unnecessary_wraps,
        clippy::ignore_without_reason,
        clippy::default_trait_access,
        clippy::needless_pass_by_value,
        clippy::missing_assert_message,
        clippy::elidable_lifetime_names,
        clippy::needless_pass_by_ref_mut,
        clippy::doc_markdown,
        clippy::needless_for_each,
        clippy::redundant_clone,
        clippy::missing_docs_in_private_items,
        clippy::exit,
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block,
        unused_imports,
        unused_qualifications,
        dead_code,
        unreachable_pub,
        missing_debug_implementations,
        unnameable_types,
        non_local_definitions,
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Valgrind is Linux-only.
}

#[cfg(target_os = "linux")]
extern crate gungraun;

#[cfg(target_os = "linux")]
use std::cell::Cell;
#[cfg(target_os = "linux")]
use std::hint::black_box;

#[cfg(target_os = "linux")]
use gungraun::prelude::*;
#[cfg(target_os = "linux")]
use region_local::{RegionLocalCopyExt as _, RegionLocalExt as _, region_local};

// One static per scenario keeps each bench's first-touch behavior
// isolated even if gungraun's per-subprocess execution model changes.
#[cfg(target_os = "linux")]
region_local!(static FIRST_TOUCH_VALUE: u32 = 99942);

#[cfg(target_os = "linux")]
region_local!(static WARM_VALUE: u32 = 99942);

#[cfg(target_os = "linux")]
region_local!(static WITH_VALUE: u32 = 99942);

#[cfg(target_os = "linux")]
region_local!(static SET_FIRST_TOUCH_VALUE: u32 = 99942);

#[cfg(target_os = "linux")]
region_local!(static SET_WARM_VALUE: u32 = 99942);

#[cfg(target_os = "linux")]
thread_local! {
    static STD_VALUE: Cell<u32> = const { Cell::new(99942) };
}

#[cfg(target_os = "linux")]
fn warm_get_local() {
    _ = WARM_VALUE.get_local();
}

#[cfg(target_os = "linux")]
fn warm_with_local() {
    WITH_VALUE.with_local(|v| {
        _ = *v;
    });
}

#[cfg(target_os = "linux")]
fn warm_set_local() {
    SET_WARM_VALUE.set_local(1);
}

#[cfg(target_os = "linux")]
fn warm_std_thread_local() {
    _ = STD_VALUE.with(Cell::get);
}

// ---------- Read paths ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
fn get_local_first_touch() -> u32 {
    FIRST_TOUCH_VALUE.get_local()
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::warm(warm_get_local())]
fn get_local_warm(_: ()) -> u32 {
    WARM_VALUE.get_local()
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::warm(warm_with_local())]
fn with_local_warm(_: ()) -> u32 {
    WITH_VALUE.with_local(|v| *v)
}

// ---------- Write path ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
fn set_local_first_touch() {
    SET_FIRST_TOUCH_VALUE.set_local(black_box(566));
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::warm(warm_set_local())]
fn set_local_warm(_: ()) {
    SET_WARM_VALUE.set_local(black_box(566));
}

// ---------- Baseline ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::warm(warm_std_thread_local())]
fn std_thread_local_get_warm(_: ()) -> u32 {
    STD_VALUE.with(Cell::get)
}

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = read_group,
    benchmarks = [get_local_first_touch, get_local_warm, with_local_warm]
);

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = write_group,
    benchmarks = [set_local_first_touch, set_local_warm]
);

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = baseline_group,
    benchmarks = [std_thread_local_get_warm]
);

#[cfg(target_os = "linux")]
main!(
    library_benchmark_groups = read_group,
    write_group,
    baseline_group
);
