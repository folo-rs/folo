//! Cycle-accurate benchmarks for the `infinity_pool` crate.
//!
//! Paired with `ip_vs_std.rs` (the `ip_vs_std` Criterion group) which
//! covers the same operations under wall-clock measurement in a
//! comprehensive churn pattern.
//!
//! Scenarios isolate the per-operation cost of pool handles so each
//! can be tracked at instruction-level granularity, separated from the
//! adjacent churn-loop overhead the Criterion bench measures:
//!
//! * `pinned_pool_insert_into_empty` — first insert; allocates a slab.
//! * `pinned_pool_insert_into_10k` — Nth insert; reuses an open slab.
//! * `pinned_pool_drop_handle_from_10k` — drop a handle, freeing one slot.
//! * `pinned_pool_deref_handle` — deref through a handle.
//!
//! Sibling variants compare the thread-safe path against single-thread
//! and untyped pools, and against `Arc::pin` and `Box::pin` baselines:
//!
//! * `local_pinned_pool_insert_into_10k` — single-threaded path.
//! * `blind_pool_insert_into_10k` — untyped path.
//! * `arc_pin_baseline_insert` — `Arc::pin` for comparison.
//! * `box_pin_baseline_insert` — `Box::pin` for comparison.
//!
//! The pool is pre-populated outside the measured region so each
//! scenario measures only the one operation under test.

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
use std::hint::black_box;
#[cfg(target_os = "linux")]
use std::pin::Pin;
#[cfg(target_os = "linux")]
use std::sync::Arc;

#[cfg(target_os = "linux")]
use gungraun::prelude::*;
#[cfg(target_os = "linux")]
use infinity_pool::{BlindPool, BlindPooledMut, LocalPinnedPool, PinnedPool, PooledMut};

// Anchor count used to populate pools before the measured operation —
// matches `ip_vs_std::INITIAL_ITEMS` so cycle and wall-clock benches
// observe the same steady-state.
#[cfg(target_os = "linux")]
const ANCHOR_COUNT: u64 = 10_000;

#[cfg(target_os = "linux")]
struct PinnedPoolState {
    pool: PinnedPool<u64>,
    anchors: Vec<PooledMut<u64>>,
}

#[cfg(target_os = "linux")]
struct LocalPinnedPoolState {
    pool: LocalPinnedPool<u64>,
    anchors: Vec<infinity_pool::LocalPooledMut<u64>>,
}

#[cfg(target_os = "linux")]
struct BlindPoolState {
    pool: BlindPool,
    anchors: Vec<BlindPooledMut<u64>>,
}

#[cfg(target_os = "linux")]
fn make_empty_pinned_pool() -> PinnedPool<u64> {
    PinnedPool::new()
}

#[cfg(target_os = "linux")]
fn make_populated_pinned_pool() -> PinnedPoolState {
    let pool = PinnedPool::<u64>::new();
    let anchors: Vec<PooledMut<u64>> = (0..ANCHOR_COUNT).map(|i| pool.insert(i)).collect();
    PinnedPoolState { pool, anchors }
}

#[cfg(target_os = "linux")]
fn make_populated_local_pinned_pool() -> LocalPinnedPoolState {
    let pool = LocalPinnedPool::<u64>::new();
    let anchors: Vec<_> = (0..ANCHOR_COUNT).map(|i| pool.insert(i)).collect();
    LocalPinnedPoolState { pool, anchors }
}

#[cfg(target_os = "linux")]
fn make_populated_blind_pool() -> BlindPoolState {
    let pool = BlindPool::new();
    let anchors: Vec<BlindPooledMut<u64>> =
        (0..ANCHOR_COUNT).map(|i| pool.insert::<u64>(i)).collect();
    BlindPoolState { pool, anchors }
}

// State for the drop benchmark: a populated pool plus a freshly-inserted
// extra handle that the bench drops. The pool keeps its 10k anchors so
// the drop only frees one slot in a fully-populated slab.
#[cfg(target_os = "linux")]
struct PinnedPoolWithExtraHandle {
    pool: PinnedPool<u64>,
    anchors: Vec<PooledMut<u64>>,
    extra: PooledMut<u64>,
}

#[cfg(target_os = "linux")]
fn make_pinned_pool_with_extra_handle() -> PinnedPoolWithExtraHandle {
    let pool = PinnedPool::<u64>::new();
    let anchors: Vec<PooledMut<u64>> = (0..ANCHOR_COUNT).map(|i| pool.insert(i)).collect();
    let extra = pool.insert(ANCHOR_COUNT);
    PinnedPoolWithExtraHandle {
        pool,
        anchors,
        extra,
    }
}

// State for the deref benchmark: a single handle in a populated pool.
#[cfg(target_os = "linux")]
fn make_pinned_pool_for_deref() -> PinnedPoolWithExtraHandle {
    make_pinned_pool_with_extra_handle()
}

// ---------- Insert paths ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::empty(make_empty_pinned_pool())]
fn pinned_pool_insert_into_empty(pool: PinnedPool<u64>) -> (PinnedPool<u64>, PooledMut<u64>) {
    let handle = black_box(&pool).insert(black_box(42_u64));
    (pool, handle)
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::populated(make_populated_pinned_pool())]
fn pinned_pool_insert_into_10k(state: PinnedPoolState) -> (PinnedPoolState, PooledMut<u64>) {
    let handle = black_box(&state.pool).insert(black_box(ANCHOR_COUNT));
    (state, handle)
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::populated(make_populated_local_pinned_pool())]
fn local_pinned_pool_insert_into_10k(
    state: LocalPinnedPoolState,
) -> (LocalPinnedPoolState, infinity_pool::LocalPooledMut<u64>) {
    let handle = black_box(&state.pool).insert(black_box(ANCHOR_COUNT));
    (state, handle)
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::populated(make_populated_blind_pool())]
fn blind_pool_insert_into_10k(state: BlindPoolState) -> (BlindPoolState, BlindPooledMut<u64>) {
    let handle = black_box(&state.pool).insert::<u64>(black_box(ANCHOR_COUNT));
    (state, handle)
}

// ---------- Drop path ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::with_extra(make_pinned_pool_with_extra_handle())]
fn pinned_pool_drop_handle_from_10k(
    state: PinnedPoolWithExtraHandle,
) -> (PinnedPool<u64>, Vec<PooledMut<u64>>) {
    let PinnedPoolWithExtraHandle {
        pool,
        anchors,
        extra,
    } = state;
    drop(black_box(extra));
    (pool, anchors)
}

// ---------- Deref path ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::populated(make_pinned_pool_for_deref())]
fn pinned_pool_deref_handle(state: PinnedPoolWithExtraHandle) -> PinnedPoolWithExtraHandle {
    let value: &u64 = &state.extra;
    _ = black_box(*value);
    state
}

// ---------- Baselines ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
fn arc_pin_baseline_insert() -> Pin<Arc<u64>> {
    Arc::pin(black_box(42_u64))
}

#[cfg(target_os = "linux")]
#[library_benchmark]
fn box_pin_baseline_insert() -> Pin<Box<u64>> {
    Box::pin(black_box(42_u64))
}

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = insert_group,
    benchmarks = [
        pinned_pool_insert_into_empty,
        pinned_pool_insert_into_10k,
        local_pinned_pool_insert_into_10k,
        blind_pool_insert_into_10k,
        arc_pin_baseline_insert,
        box_pin_baseline_insert,
    ]
);

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = drop_group,
    benchmarks = [pinned_pool_drop_handle_from_10k]
);

#[cfg(target_os = "linux")]
library_benchmark_group!(name = deref_group, benchmarks = [pinned_pool_deref_handle]);

#[cfg(target_os = "linux")]
main!(
    library_benchmark_groups = insert_group,
    drop_group,
    deref_group
);
