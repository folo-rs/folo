//! Callgrind benchmarks for the `infinity_pool` crate.
//!
//! Paired Criterion (wall-clock) coverage lives in two sibling files:
//!
//! * `infinity_pool_vs_std.rs` (the `ip_vs_std` Criterion group) covers
//!   the insert/drop scenarios here under a comprehensive churn pattern.
//! * `infinity_pool_focused.rs` (the `focused` Criterion group) covers
//!   the remaining scenarios as elementary single-op measurements:
//!   deref, multi-layout `BlindPool` insert, raw bulk drop, and iter.
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
//! * `blind_pool_insert_into_10k` — untyped path with 1 distinct layout.
//! * `blind_pool_insert_into_10k_n5_layouts` — dispatch over 5 distinct layouts.
//! * `blind_pool_insert_into_10k_n8_layouts` — dispatch over 8 distinct layouts.
//! * `blind_pool_insert_into_10k_n16_layouts` — dispatch over 16 layouts (heap spill).
//! * `blind_pool_insert_into_10k_n8_layouts_adversarial` — measured key is *not* the
//!   most-recently used; exercises the worst-case linear-scan path.
//! * `arc_pin_baseline_insert` — `Arc::pin` for comparison.
//! * `box_pin_baseline_insert` — `Box::pin` for comparison.
//!
//! Iteration and bulk-drop scenarios exercise the per-slot scan paths
//! that are not visible through insert/remove micro-benchmarks:
//!
//! * `pinned_pool_iter_full_10k` — dense iteration over 10k occupied slots.
//! * `pinned_pool_iter_sparse_10k` — sparse iteration: 1250 of 10000 slots
//!   occupied, exposes the per-empty-slot scan overhead.
//! * `raw_pinned_pool_drop_full_with_10k_u64` — drop a `RawPinnedPool`
//!   populated with 10k `u64` values; raw handles have no `Drop`, so the
//!   measurement isolates the slab's per-slot drop-scan cost for a payload
//!   that does not need drop.
//! * `raw_pinned_pool_drop_full_with_10k_string` — same shape with a payload
//!   that does need drop, so the per-slot dropper indirection runs and each
//!   `String` allocation is freed.
//!
//! The pool is pre-populated outside the measured region so each
//! scenario measures only the one operation under test.

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
    use std::hint::black_box;
    use std::pin::Pin;
    use std::sync::Arc;

    use gungraun::prelude::*;
    use infinity_pool::{
        BlindPool, BlindPooledMut, LocalPinnedPool, PinnedPool, PooledMut, RawPinnedPool,
    };

    // Anchor count used to populate pools before the measured operation —
    // matches `ip_vs_std::INITIAL_ITEMS` so Callgrind and wall-clock benches
    // observe the same steady-state.
    const ANCHOR_COUNT: u64 = 10_000;

    struct PinnedPoolState {
        pool: PinnedPool<u64>,
        #[expect(
            dead_code,
            reason = "anchors keep slots in the populated pool occupied during measurement; their `Drop` is the cleanup contract"
        )]
        anchors: Vec<PooledMut<u64>>,
    }

    struct LocalPinnedPoolState {
        pool: LocalPinnedPool<u64>,
        #[expect(
            dead_code,
            reason = "anchors keep slots in the populated pool occupied during measurement; their `Drop` is the cleanup contract"
        )]
        anchors: Vec<infinity_pool::LocalPooledMut<u64>>,
    }

    struct BlindPoolState {
        pool: BlindPool,
        #[expect(
            dead_code,
            reason = "anchors keep slots in the populated pool occupied during measurement; their `Drop` is the cleanup contract"
        )]
        anchors: Vec<BlindPooledMut<u64>>,
    }

    fn make_empty_pinned_pool() -> PinnedPool<u64> {
        PinnedPool::new()
    }

    fn make_populated_pinned_pool() -> PinnedPoolState {
        let pool = PinnedPool::<u64>::new();
        let anchors: Vec<PooledMut<u64>> = (0..ANCHOR_COUNT).map(|i| pool.insert(i)).collect();
        PinnedPoolState { pool, anchors }
    }

    fn make_populated_local_pinned_pool() -> LocalPinnedPoolState {
        let pool = LocalPinnedPool::<u64>::new();
        let anchors: Vec<_> = (0..ANCHOR_COUNT).map(|i| pool.insert(i)).collect();
        LocalPinnedPoolState { pool, anchors }
    }

    fn make_populated_blind_pool() -> BlindPoolState {
        let pool = BlindPool::new();
        let anchors: Vec<BlindPooledMut<u64>> =
            (0..ANCHOR_COUNT).map(|i| pool.insert::<u64>(i)).collect();
        BlindPoolState { pool, anchors }
    }

    // Populates the dispatch with N - 1 extra inner pools by inserting and immediately
    // dropping one item of each anchor type. The inner `RawOpaquePool` instances remain
    // in the dispatch even after all items are removed, so the measured `pool.insert::<u64>`
    // call must traverse a dispatch with N distinct `LayoutKey` entries.
    //
    // The 10k u64 anchor inserts that follow ensure u64 is the most-recently-used entry
    // in the dispatch — the typical pattern in real `BlindPool` workloads where one or
    // two layouts dominate. This is the scenario the move-to-front linear scan is
    // designed to make cheap.
    fn make_populated_blind_pool_n5_layouts() -> BlindPoolState {
        let pool = BlindPool::new();
        // 4 extra layouts so the dispatch holds 5 entries once u64 is added.
        drop(pool.insert::<u8>(0));
        drop(pool.insert::<u16>(0));
        drop(pool.insert::<u32>(0));
        drop(pool.insert::<u128>(0));
        let anchors: Vec<BlindPooledMut<u64>> =
            (0..ANCHOR_COUNT).map(|i| pool.insert::<u64>(i)).collect();
        BlindPoolState { pool, anchors }
    }

    fn make_populated_blind_pool_n8_layouts() -> BlindPoolState {
        let pool = BlindPool::new();
        // 7 extra layouts so the dispatch holds 8 entries once u64 is added.
        drop(pool.insert::<u8>(0));
        drop(pool.insert::<u16>(0));
        drop(pool.insert::<u32>(0));
        drop(pool.insert::<u128>(0));
        drop(pool.insert::<[u8; 3]>([0; 3]));
        drop(pool.insert::<[u8; 5]>([0; 5]));
        drop(pool.insert::<[u8; 6]>([0; 6]));
        let anchors: Vec<BlindPooledMut<u64>> =
            (0..ANCHOR_COUNT).map(|i| pool.insert::<u64>(i)).collect();
        BlindPoolState { pool, anchors }
    }

    fn make_populated_blind_pool_n16_layouts() -> BlindPoolState {
        let pool = BlindPool::new();
        // 15 extra layouts via varied byte-array sizes (size N, align 1) plus u128 so the
        // dispatch holds 16 entries once u64 is added. This exceeds the inline capacity
        // of the dispatch and forces heap spillover.
        drop(pool.insert::<[u8; 1]>([0; 1]));
        drop(pool.insert::<[u8; 2]>([0; 2]));
        drop(pool.insert::<[u8; 3]>([0; 3]));
        drop(pool.insert::<[u8; 4]>([0; 4]));
        drop(pool.insert::<[u8; 5]>([0; 5]));
        drop(pool.insert::<[u8; 6]>([0; 6]));
        drop(pool.insert::<[u8; 7]>([0; 7]));
        drop(pool.insert::<[u8; 9]>([0; 9]));
        drop(pool.insert::<[u8; 10]>([0; 10]));
        drop(pool.insert::<[u8; 11]>([0; 11]));
        drop(pool.insert::<[u8; 12]>([0; 12]));
        drop(pool.insert::<[u8; 13]>([0; 13]));
        drop(pool.insert::<[u8; 14]>([0; 14]));
        drop(pool.insert::<[u8; 15]>([0; 15]));
        drop(pool.insert::<u128>(0));
        let anchors: Vec<BlindPooledMut<u64>> =
            (0..ANCHOR_COUNT).map(|i| pool.insert::<u64>(i)).collect();
        BlindPoolState { pool, anchors }
    }

    // Adversarial setup for the dispatch: builds an 8-entry dispatch where the measured
    // key (u64) is *not* the most-recently used. The dispatch internally maintains
    // most-recently-used order, so an insert here cannot benefit from the front fast
    // path and must walk through the other entries before finding u64.
    //
    // This complements the locality-friendly `make_populated_blind_pool_n8_layouts`
    // helper and quantifies the worst-case scan cost.
    fn make_adversarial_blind_pool_n8_layouts() -> BlindPoolState {
        let pool = BlindPool::new();
        // u64 first so it is initially in the dispatch with anchor items, then 7 extras
        // inserted afterward — each pushes u64 further back in MRU order. Final MRU
        // order: [u128, [u8; 6], [u8; 5], [u8; 3], u32, u16, u8, u64].
        let anchors: Vec<BlindPooledMut<u64>> =
            (0..ANCHOR_COUNT).map(|i| pool.insert::<u64>(i)).collect();
        drop(pool.insert::<u8>(0));
        drop(pool.insert::<u16>(0));
        drop(pool.insert::<u32>(0));
        drop(pool.insert::<[u8; 3]>([0; 3]));
        drop(pool.insert::<[u8; 5]>([0; 5]));
        drop(pool.insert::<[u8; 6]>([0; 6]));
        drop(pool.insert::<u128>(0));
        BlindPoolState { pool, anchors }
    }

    // State for the sparse-iteration bench: insert ANCHOR_COUNT (10 000)
    // items, then drop 7 out of every 8, leaving 1 250 surviving handles
    // scattered across the slabs allocated for the 10 000 inserts.
    // Exposes the cost the iterator pays for scanning past empty slots.
    struct SparsePinnedPoolState {
        pool: PinnedPool<u64>,
        #[expect(
            dead_code,
            reason = "kept anchors hold the surviving slots; their `Drop` is the cleanup contract"
        )]
        anchors: Vec<PooledMut<u64>>,
    }

    fn make_sparse_pinned_pool() -> SparsePinnedPoolState {
        let pool = PinnedPool::<u64>::new();
        let all: Vec<PooledMut<u64>> = (0..ANCHOR_COUNT).map(|i| pool.insert(i)).collect();
        // Keep every 8th, drop the rest, leaving 1250 surviving handles
        // scattered across the slabs allocated for the 10 000 inserts.
        let mut anchors: Vec<PooledMut<u64>> = Vec::with_capacity(1250);
        for (idx, handle) in all.into_iter().enumerate() {
            if idx.is_multiple_of(8) {
                anchors.push(handle);
            } else {
                drop(handle);
            }
        }
        SparsePinnedPoolState { pool, anchors }
    }

    // Bulk-slab-drop bench with a payload that does NOT need drop.
    // The `RawPooledMut` handles produced by `insert` are intentionally
    // discarded inside setup (excluded from timing): `RawPooledMut` has
    // no `Drop` and does not keep slots alive, so this leaves the pool
    // fully populated while keeping the timed routine free of any
    // unrelated `Vec<RawPooledMut<_>>` deallocation cost.
    fn make_raw_pinned_pool_u64_bulk() -> RawPinnedPool<u64> {
        let mut pool = RawPinnedPool::<u64>::new();
        for i in 0..ANCHOR_COUNT {
            _ = pool.insert(i);
        }
        pool
    }

    // Bulk-slab-drop bench with a payload that DOES need drop.
    // Each `String` carries an owned heap allocation that must be freed
    // via the per-slot type-erased dropper invocation. As for the u64
    // counterpart above, the handles are discarded inside setup so the
    // measurement focuses on the pool drop path.
    fn make_raw_pinned_pool_string_bulk() -> RawPinnedPool<String> {
        let mut pool = RawPinnedPool::<String>::new();
        for i in 0..ANCHOR_COUNT {
            _ = pool.insert(format!("item-{i}"));
        }
        pool
    }

    // State for the drop benchmark: a populated pool plus a freshly-inserted
    // extra handle that the bench drops. The pool keeps its 10k anchors so
    // the drop only frees one slot in a fully-populated slab.
    struct PinnedPoolWithExtraHandle {
        pool: PinnedPool<u64>,
        anchors: Vec<PooledMut<u64>>,
        extra: PooledMut<u64>,
    }

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
    fn make_pinned_pool_for_deref() -> PinnedPoolWithExtraHandle {
        make_pinned_pool_with_extra_handle()
    }

    // ---------- Insert paths ----------

    #[library_benchmark]
    #[bench::empty(make_empty_pinned_pool())]
    fn pinned_pool_insert_into_empty(pool: PinnedPool<u64>) -> (PinnedPool<u64>, PooledMut<u64>) {
        let handle = black_box(&pool).insert(black_box(42_u64));
        (pool, handle)
    }

    #[library_benchmark]
    #[bench::populated(make_populated_pinned_pool())]
    fn pinned_pool_insert_into_10k(state: PinnedPoolState) -> (PinnedPoolState, PooledMut<u64>) {
        let handle = black_box(&state.pool).insert(black_box(ANCHOR_COUNT));
        (state, handle)
    }

    #[library_benchmark]
    #[bench::populated(make_populated_local_pinned_pool())]
    fn local_pinned_pool_insert_into_10k(
        state: LocalPinnedPoolState,
    ) -> (LocalPinnedPoolState, infinity_pool::LocalPooledMut<u64>) {
        let handle = black_box(&state.pool).insert(black_box(ANCHOR_COUNT));
        (state, handle)
    }

    #[library_benchmark]
    #[bench::populated(make_populated_blind_pool())]
    fn blind_pool_insert_into_10k(state: BlindPoolState) -> (BlindPoolState, BlindPooledMut<u64>) {
        let handle = black_box(&state.pool).insert::<u64>(black_box(ANCHOR_COUNT));
        (state, handle)
    }

    #[library_benchmark]
    #[bench::populated(make_populated_blind_pool_n5_layouts())]
    fn blind_pool_insert_into_10k_n5_layouts(
        state: BlindPoolState,
    ) -> (BlindPoolState, BlindPooledMut<u64>) {
        let handle = black_box(&state.pool).insert::<u64>(black_box(ANCHOR_COUNT));
        (state, handle)
    }

    #[library_benchmark]
    #[bench::populated(make_populated_blind_pool_n8_layouts())]
    fn blind_pool_insert_into_10k_n8_layouts(
        state: BlindPoolState,
    ) -> (BlindPoolState, BlindPooledMut<u64>) {
        let handle = black_box(&state.pool).insert::<u64>(black_box(ANCHOR_COUNT));
        (state, handle)
    }

    #[library_benchmark]
    #[bench::populated(make_populated_blind_pool_n16_layouts())]
    fn blind_pool_insert_into_10k_n16_layouts(
        state: BlindPoolState,
    ) -> (BlindPoolState, BlindPooledMut<u64>) {
        let handle = black_box(&state.pool).insert::<u64>(black_box(ANCHOR_COUNT));
        (state, handle)
    }

    #[library_benchmark]
    #[bench::populated(make_adversarial_blind_pool_n8_layouts())]
    fn blind_pool_insert_into_10k_n8_layouts_adversarial(
        state: BlindPoolState,
    ) -> (BlindPoolState, BlindPooledMut<u64>) {
        let handle = black_box(&state.pool).insert::<u64>(black_box(ANCHOR_COUNT));
        (state, handle)
    }

    // ---------- Drop path ----------

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

    #[library_benchmark]
    #[bench::u64_no_drop(make_raw_pinned_pool_u64_bulk())]
    fn raw_pinned_pool_drop_full_with_10k_u64(pool: RawPinnedPool<u64>) {
        // Drops the pool. Isolates the slab's per-slot drop-scan cost for
        // a payload that does not need drop — the per-slot
        // `drop_in_place::<u64>` is a no-op the compiler elides, but the
        // dropper indirection (`SlotMeta::drop` -> `Dropper::drop` -> fn-ptr)
        // and the per-slot `catch_unwind` setup both run regardless.
        drop(black_box(pool));
    }

    #[library_benchmark]
    #[bench::string_with_drop(make_raw_pinned_pool_string_bulk())]
    fn raw_pinned_pool_drop_full_with_10k_string(pool: RawPinnedPool<String>) {
        // Drops the pool. Each slot's `String` allocation is freed via the
        // per-slot type-erased dropper invocation, on top of the same
        // per-slot scan and `catch_unwind` setup.
        drop(black_box(pool));
    }

    // ---------- Iteration path ----------

    #[library_benchmark]
    #[bench::populated(make_populated_pinned_pool())]
    fn pinned_pool_iter_full_10k(state: PinnedPoolState) -> PinnedPoolState {
        // Dense iteration over 10k occupied slots. Each yielded pointer is
        // black-boxed to defeat the iter-elision pass.
        state.pool.with_iter(|iter| {
            for ptr in iter {
                _ = black_box(ptr);
            }
        });
        state
    }

    #[library_benchmark]
    #[bench::sparse(make_sparse_pinned_pool())]
    fn pinned_pool_iter_sparse_10k(state: SparsePinnedPoolState) -> SparsePinnedPoolState {
        // Sparse iteration: 1250 surviving handles scattered across the
        // slabs allocated for the 10 000 inserts. Exposes the per-empty-slot
        // scan cost.
        state.pool.with_iter(|iter| {
            for ptr in iter {
                _ = black_box(ptr);
            }
        });
        state
    }

    // ---------- Deref path ----------

    #[library_benchmark]
    #[bench::populated(make_pinned_pool_for_deref())]
    fn pinned_pool_deref_handle(state: PinnedPoolWithExtraHandle) -> PinnedPoolWithExtraHandle {
        let value: &u64 = &state.extra;
        _ = black_box(*value);
        state
    }

    // ---------- Baselines ----------

    #[library_benchmark]
    fn arc_pin_baseline_insert() -> Pin<Arc<u64>> {
        Arc::pin(black_box(42_u64))
    }

    #[library_benchmark]
    fn box_pin_baseline_insert() -> Pin<Box<u64>> {
        Box::pin(black_box(42_u64))
    }

    library_benchmark_group!(
        name = insert_group,
        benchmarks = [
            pinned_pool_insert_into_empty,
            pinned_pool_insert_into_10k,
            local_pinned_pool_insert_into_10k,
            blind_pool_insert_into_10k,
            blind_pool_insert_into_10k_n5_layouts,
            blind_pool_insert_into_10k_n8_layouts,
            blind_pool_insert_into_10k_n16_layouts,
            blind_pool_insert_into_10k_n8_layouts_adversarial,
            arc_pin_baseline_insert,
            box_pin_baseline_insert,
        ]
    );

    library_benchmark_group!(
        name = drop_group,
        benchmarks = [
            pinned_pool_drop_handle_from_10k,
            raw_pinned_pool_drop_full_with_10k_u64,
            raw_pinned_pool_drop_full_with_10k_string,
        ]
    );

    library_benchmark_group!(
        name = iter_group,
        benchmarks = [pinned_pool_iter_full_10k, pinned_pool_iter_sparse_10k]
    );

    library_benchmark_group!(name = deref_group, benchmarks = [pinned_pool_deref_handle]);
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{deref_group, drop_group, insert_group, iter_group};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = insert_group, drop_group, iter_group, deref_group
);
