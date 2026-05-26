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
//! * `blind_pool_insert_with_5_layouts` — untyped path with 5 distinct
//!   layouts; isolates the layout-dispatch cost.
//! * `arc_pin_baseline_insert` — `Arc::pin` for comparison.
//! * `box_pin_baseline_insert` — `Box::pin` for comparison.
//!
//! The `OpaquePool` family wraps the same inner `RawOpaquePool` as the
//! `PinnedPool` family, but differs on two axes the typed variants do
//! not exercise: the runtime layout assertion on every `insert` (the
//! typed wrappers can statically prove the layout match and use the
//! unchecked path), and the per-handle drop/remove cost on a pool whose
//! API does not name the stored type. The sibling scenarios isolate
//! these costs at instruction-level granularity:
//!
//! * `opaque_pool_insert_into_10k` — `OpaquePool::insert` (`Arc` + `Mutex` +
//!   layout-check); paired with the `OpaquePool` bench in the `ip_vs_std`
//!   Criterion group.
//! * `local_opaque_pool_insert_into_10k` — `LocalOpaquePool::insert`
//!   (`Rc` + `RefCell` + layout-check); paired with the `LocalOpaquePool`
//!   bench in the `ip_vs_std` Criterion group.
//! * `raw_opaque_pool_insert_into_10k` — `RawOpaquePool::insert` (layout
//!   check only, no ref count); paired with the `RawOpaquePool` bench in
//!   the `ip_vs_std` Criterion group.
//! * `opaque_pool_drop_handle_from_10k` — drop a `PooledMut<u64>`,
//!   exercising the `Arc`-clone drop path through the inner pool's mutex.
//! * `local_opaque_pool_drop_handle_from_10k` — drop a `LocalPooledMut<u64>`,
//!   exercising the `Rc`-clone drop path through the inner pool's `RefCell`.
//! * `raw_opaque_pool_remove_from_10k` — explicit `unsafe pool.remove(handle)`
//!   on a populated `RawOpaquePool`; raw handles have no `Drop`, so the
//!   measurement isolates the unguarded slot-release cost.
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
        BlindPool, BlindPooledMut, LocalOpaquePool, LocalPinnedPool, LocalPooledMut, OpaquePool,
        PinnedPool, PooledMut, RawOpaquePool, RawPinnedPool, RawPooledMut,
    };

    // Anchor count used to populate pools before the measured operation —
    // matches the private `INITIAL_ITEMS` constant in `infinity_pool_vs_std.rs`
    // so Callgrind and wall-clock benches observe the same steady-state.
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

    struct OpaquePoolState {
        pool: OpaquePool,
        #[expect(
            dead_code,
            reason = "anchors keep slots in the populated pool occupied during measurement; their `Drop` is the cleanup contract"
        )]
        anchors: Vec<PooledMut<u64>>,
    }

    fn make_populated_opaque_pool() -> OpaquePoolState {
        let pool = OpaquePool::with_layout_of::<u64>();
        let anchors: Vec<PooledMut<u64>> = (0..ANCHOR_COUNT).map(|i| pool.insert(i)).collect();
        OpaquePoolState { pool, anchors }
    }

    struct LocalOpaquePoolState {
        pool: LocalOpaquePool,
        #[expect(
            dead_code,
            reason = "anchors keep slots in the populated pool occupied during measurement; their `Drop` is the cleanup contract"
        )]
        anchors: Vec<LocalPooledMut<u64>>,
    }

    fn make_populated_local_opaque_pool() -> LocalOpaquePoolState {
        let pool = LocalOpaquePool::with_layout_of::<u64>();
        let anchors: Vec<LocalPooledMut<u64>> = (0..ANCHOR_COUNT).map(|i| pool.insert(i)).collect();
        LocalOpaquePoolState { pool, anchors }
    }

    // `RawOpaquePool` handles have no `Drop`, so the anchors are intentionally
    // discarded inside setup: dropping them does not release any slots. The
    // pool itself is moved into the bench fn and dropped after the timed call.
    fn make_populated_raw_opaque_pool() -> RawOpaquePool {
        let mut pool = RawOpaquePool::with_layout_of::<u64>();
        for i in 0..ANCHOR_COUNT {
            _ = pool.insert(i);
        }
        pool
    }

    // State for the `raw_opaque_pool_remove_from_10k` bench: a populated raw
    // pool plus a freshly-inserted extra handle that the bench removes via
    // `unsafe pool.remove(handle)`. Mirrors the `PinnedPoolWithExtraHandle`
    // shape used by `pinned_pool_drop_handle_from_10k`.
    struct RawOpaquePoolWithExtraHandle {
        pool: RawOpaquePool,
        extra: RawPooledMut<u64>,
    }

    fn make_raw_opaque_pool_with_extra_handle() -> RawOpaquePoolWithExtraHandle {
        let mut pool = RawOpaquePool::with_layout_of::<u64>();
        for i in 0..ANCHOR_COUNT {
            _ = pool.insert(i);
        }
        let extra = pool.insert(ANCHOR_COUNT);
        RawOpaquePoolWithExtraHandle { pool, extra }
    }

    // State for `opaque_pool_drop_handle_from_10k`. Mirrors the pinned-pool
    // variant: anchors keep the pool fully populated, and the bench drops
    // the `extra` handle so the measurement isolates the per-handle drop
    // path through the Arc + Mutex inner pool.
    struct OpaquePoolWithExtraHandle {
        pool: OpaquePool,
        anchors: Vec<PooledMut<u64>>,
        extra: PooledMut<u64>,
    }

    fn make_opaque_pool_with_extra_handle() -> OpaquePoolWithExtraHandle {
        let pool = OpaquePool::with_layout_of::<u64>();
        let anchors: Vec<PooledMut<u64>> = (0..ANCHOR_COUNT).map(|i| pool.insert(i)).collect();
        let extra = pool.insert(ANCHOR_COUNT);
        OpaquePoolWithExtraHandle {
            pool,
            anchors,
            extra,
        }
    }

    // State for `local_opaque_pool_drop_handle_from_10k`. Mirrors the
    // managed-opaque variant but exercises the single-threaded Rc + RefCell
    // drop path instead of the Arc + Mutex one.
    struct LocalOpaquePoolWithExtraHandle {
        pool: LocalOpaquePool,
        anchors: Vec<LocalPooledMut<u64>>,
        extra: LocalPooledMut<u64>,
    }

    fn make_local_opaque_pool_with_extra_handle() -> LocalOpaquePoolWithExtraHandle {
        let pool = LocalOpaquePool::with_layout_of::<u64>();
        let anchors: Vec<LocalPooledMut<u64>> = (0..ANCHOR_COUNT).map(|i| pool.insert(i)).collect();
        let extra = pool.insert(ANCHOR_COUNT);
        LocalOpaquePoolWithExtraHandle {
            pool,
            anchors,
            extra,
        }
    }

    // State for the `blind_pool_insert_with_5_layouts` bench. The pool holds
    // anchors for five distinct layouts so the dispatch path performs a
    // multi-key lookup. The five wrapper types below have sizes 1, 2, 4, 8,
    // and 24 bytes respectively, giving distinct `LayoutKey`s (which are
    // derived from `(size, align)`); distinct sizes alone are sufficient
    // here.
    struct BlindPoolFiveLayoutsState {
        pool: BlindPool,
        #[expect(
            dead_code,
            reason = "anchors keep slots in the populated pool occupied during measurement; their `Drop` is the cleanup contract"
        )]
        anchors: Vec<Box<dyn std::any::Any>>,
    }

    #[derive(Clone, Copy)]
    #[expect(
        dead_code,
        reason = "fields exist only to define the type layout; their values are never read"
    )]
    struct L1(u8);
    #[derive(Clone, Copy)]
    #[expect(
        dead_code,
        reason = "fields exist only to define the type layout; their values are never read"
    )]
    struct L2(u16);
    #[derive(Clone, Copy)]
    #[expect(
        dead_code,
        reason = "fields exist only to define the type layout; their values are never read"
    )]
    struct L3(u32);
    #[derive(Clone, Copy)]
    #[expect(
        dead_code,
        reason = "fields exist only to define the type layout; their values are never read"
    )]
    struct L4(u64);
    #[derive(Clone, Copy)]
    #[expect(
        dead_code,
        reason = "fields exist only to define the type layout; their values are never read"
    )]
    struct L5([u64; 3]);

    fn make_blind_pool_with_5_layouts() -> BlindPoolFiveLayoutsState {
        let pool = BlindPool::new();
        let anchors: Vec<Box<dyn std::any::Any>> = vec![
            Box::new(pool.insert::<L1>(L1(0))),
            Box::new(pool.insert::<L2>(L2(0))),
            Box::new(pool.insert::<L3>(L3(0))),
            Box::new(pool.insert::<L4>(L4(0))),
            Box::new(pool.insert::<L5>(L5([0; 3]))),
        ];
        BlindPoolFiveLayoutsState { pool, anchors }
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
    #[bench::five_layouts(make_blind_pool_with_5_layouts())]
    fn blind_pool_insert_with_5_layouts(
        state: BlindPoolFiveLayoutsState,
    ) -> (BlindPoolFiveLayoutsState, BlindPooledMut<L4>) {
        // Insert into the L4 (u64-sized) layout — the fourth of five present.
        // Dispatch must locate the correct inner pool out of five.
        let handle = black_box(&state.pool).insert::<L4>(black_box(L4(42)));
        (state, handle)
    }

    #[library_benchmark]
    #[bench::populated(make_populated_opaque_pool())]
    fn opaque_pool_insert_into_10k(state: OpaquePoolState) -> (OpaquePoolState, PooledMut<u64>) {
        // Exercises the managed (Arc + Mutex) opaque insert path with the
        // runtime layout assertion that the typed `PinnedPool` wrapper
        // bypasses via `insert_unchecked`.
        let handle = black_box(&state.pool).insert(black_box(ANCHOR_COUNT));
        (state, handle)
    }

    #[library_benchmark]
    #[bench::populated(make_populated_local_opaque_pool())]
    fn local_opaque_pool_insert_into_10k(
        state: LocalOpaquePoolState,
    ) -> (LocalOpaquePoolState, LocalPooledMut<u64>) {
        // Exercises the single-threaded (Rc + RefCell) opaque insert path
        // with the runtime layout assertion that `LocalPinnedPool` bypasses
        // via `insert_unchecked`.
        let handle = black_box(&state.pool).insert(black_box(ANCHOR_COUNT));
        (state, handle)
    }

    #[library_benchmark]
    #[bench::populated(make_populated_raw_opaque_pool())]
    fn raw_opaque_pool_insert_into_10k(
        mut pool: RawOpaquePool,
    ) -> (RawOpaquePool, RawPooledMut<u64>) {
        // Exercises the raw opaque insert path: no ref-counting, no
        // synchronization, but the runtime layout assertion still runs
        // because callers may insert any same-layout `T`.
        let handle = black_box(&mut pool).insert(black_box(ANCHOR_COUNT));
        (pool, handle)
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
    #[bench::with_extra(make_opaque_pool_with_extra_handle())]
    fn opaque_pool_drop_handle_from_10k(
        state: OpaquePoolWithExtraHandle,
    ) -> (OpaquePool, Vec<PooledMut<u64>>) {
        // Drops a `PooledMut<u64>` whose `Drop` impl removes the slot
        // through the `Arc<Mutex<RawOpaquePoolThreadSafe>>` inner pool.
        // Isolates the managed-opaque per-handle drop cost relative to
        // the typed `pinned_pool_drop_handle_from_10k` baseline.
        let OpaquePoolWithExtraHandle {
            pool,
            anchors,
            extra,
        } = state;
        drop(black_box(extra));
        (pool, anchors)
    }

    #[library_benchmark]
    #[bench::with_extra(make_local_opaque_pool_with_extra_handle())]
    fn local_opaque_pool_drop_handle_from_10k(
        state: LocalOpaquePoolWithExtraHandle,
    ) -> (LocalOpaquePool, Vec<LocalPooledMut<u64>>) {
        // Drops a `LocalPooledMut<u64>` whose `Drop` impl removes the slot
        // through the `Rc<RefCell<RawOpaquePool>>` inner pool. Isolates the
        // single-threaded opaque per-handle drop cost.
        let LocalOpaquePoolWithExtraHandle {
            pool,
            anchors,
            extra,
        } = state;
        drop(black_box(extra));
        (pool, anchors)
    }

    #[library_benchmark]
    #[bench::with_extra(make_raw_opaque_pool_with_extra_handle())]
    fn raw_opaque_pool_remove_from_10k(state: RawOpaquePoolWithExtraHandle) -> RawOpaquePool {
        // Explicitly removes a `RawPooledMut<u64>` from a populated raw
        // pool. `RawPooledMut` has no `Drop`, so the measurement isolates
        // the unguarded slot-release path with no ref-count or
        // synchronization overhead.
        let RawOpaquePoolWithExtraHandle { mut pool, extra } = state;
        // SAFETY: `extra` was produced by `pool.insert` above and has not
        // been removed yet, so the handle refers to an object currently
        // present in this pool.
        unsafe {
            pool.remove(black_box(extra));
        }
        pool
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
            blind_pool_insert_with_5_layouts,
            opaque_pool_insert_into_10k,
            local_opaque_pool_insert_into_10k,
            raw_opaque_pool_insert_into_10k,
            arc_pin_baseline_insert,
            box_pin_baseline_insert,
        ]
    );

    library_benchmark_group!(
        name = drop_group,
        benchmarks = [
            pinned_pool_drop_handle_from_10k,
            opaque_pool_drop_handle_from_10k,
            local_opaque_pool_drop_handle_from_10k,
            raw_opaque_pool_remove_from_10k,
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
pub use linux::{deref_group, drop_group, insert_group, iter_group};

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = insert_group, drop_group, iter_group, deref_group
);
