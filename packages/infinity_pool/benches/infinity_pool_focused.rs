//! Focused wall-clock benchmarks paired with `infinity_pool_cg.rs`.
//!
//! Each scenario here measures the same operation as one Callgrind
//! scenario whose pairing is not already covered by the churn-style
//! benchmarks in `infinity_pool_vs_std.rs`:
//!
//! | This file (Criterion)                  | `infinity_pool_cg.rs` (Callgrind)             |
//! |----------------------------------------|-----------------------------------------------|
//! | `focused/deref_handle`                 | `pinned_pool_deref_handle`                    |
//! | `focused/blind_insert_with_5_layouts`  | `blind_pool_insert_with_5_layouts`            |
//! | `focused/raw_drop_full_u64`            | `raw_pinned_pool_drop_full_with_10k_u64`      |
//! | `focused/raw_drop_full_string`         | `raw_pinned_pool_drop_full_with_10k_string`   |
//! | `focused/iter_full`                    | `pinned_pool_iter_full_10k`                   |
//! | `focused/iter_sparse`                  | `pinned_pool_iter_sparse_10k`                 |
//!
//! Insert and drop scenarios that are already covered by the churn
//! benchmarks in `infinity_pool_vs_std.rs` are intentionally not
//! duplicated here.

#![allow(
    missing_docs,
    clippy::let_underscore_must_use,
    clippy::undocumented_unsafe_blocks,
    reason = "duty of care is slightly lowered for benchmark code"
)]

use std::hint::black_box;
use std::time::Instant;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use infinity_pool::{BlindPool, BlindPooledMut, PinnedPool, PooledMut, RawPinnedPool};

/// Anchor count used to populate pools before the measured operation —
/// matches `ANCHOR_COUNT` in `infinity_pool_cg.rs` so wall-clock and
/// Callgrind benches observe the same steady-state.
const ANCHOR_COUNT: u64 = 10_000;

/// Number of surviving handles in the sparse-iteration scenario
/// (every 8th of `ANCHOR_COUNT`). Mirrors the CG counterpart.
#[expect(
    clippy::cast_possible_truncation,
    reason = "ANCHOR_COUNT is 10 000; the value fits in usize on every supported target"
)]
const SPARSE_SURVIVOR_COUNT: usize = ANCHOR_COUNT.div_ceil(8) as usize;

criterion_group!(benches, focused);
criterion_main!(benches);

fn focused(c: &mut Criterion) {
    deref_handle(c);
    blind_insert_with_5_layouts(c);
    raw_drop_full_u64(c);
    raw_drop_full_string(c);
    iter_full(c);
    iter_sparse(c);
}

/// Pairs with `pinned_pool_deref_handle`. Measures the deref of a handle
/// into a populated pool. The deref is non-mutating, so the same pool
/// and handle are reused across all iterations of a sample.
fn deref_handle(c: &mut Criterion) {
    let mut group = c.benchmark_group("infinity_pool_focused/focused");

    group.bench_function("deref_handle", |b| {
        b.iter_custom(|iters| {
            let pool = PinnedPool::<u64>::new();
            let anchors: Vec<PooledMut<u64>> = (0..ANCHOR_COUNT).map(|i| pool.insert(i)).collect();
            let extra = pool.insert(ANCHOR_COUNT);

            let start = Instant::now();
            for _ in 0..iters {
                let value: &u64 = &extra;
                _ = black_box(*value);
            }
            let elapsed = start.elapsed();

            _ = black_box(&extra);
            _ = black_box(&anchors);
            _ = black_box(&pool);
            elapsed
        });
    });

    group.finish();
}

/// Pairs with `blind_pool_insert_with_5_layouts`. Measures one insert
/// into a `BlindPool` that already holds anchors for five distinct
/// layouts, exercising the multi-key dispatch path. Each iteration
/// requires a fresh populated pool because insert mutates the pool;
/// the setup is batched and excluded from timing. The newly inserted
/// handle is returned alongside the pool and anchors so its drop runs
/// after timing stops — the routine measures only the insert path.
fn blind_insert_with_5_layouts(c: &mut Criterion) {
    let mut group = c.benchmark_group("infinity_pool_focused/focused");

    group.bench_function("blind_insert_with_5_layouts", |b| {
        b.iter_batched(
            make_blind_pool_with_5_layouts,
            |(pool, anchors)| {
                let handle: BlindPooledMut<u64> = black_box(&pool).insert::<u64>(black_box(42_u64));
                (pool, anchors, handle)
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

/// Pairs with `raw_pinned_pool_drop_full_with_10k_u64`. Measures the
/// drop of a `RawPinnedPool<u64>` populated with 10 000 items. The
/// payload does not need drop, so this isolates the slab's per-slot
/// drop-scan and the `Dropper` indirection. Fresh state per iteration;
/// `LargeInput` keeps the batch small so peak memory stays bounded
/// (each setup input holds ~80 KB of payload plus slab metadata).
fn raw_drop_full_u64(c: &mut Criterion) {
    let mut group = c.benchmark_group("infinity_pool_focused/focused");

    group.bench_function("raw_drop_full_u64", |b| {
        b.iter_batched(
            make_raw_pinned_pool_u64_bulk,
            |pool| drop(black_box(pool)),
            BatchSize::LargeInput,
        );
    });

    group.finish();
}

/// Pairs with `raw_pinned_pool_drop_full_with_10k_string`. Measures the
/// drop of a `RawPinnedPool<String>` populated with 10 000 items.
/// `String` has a non-trivial `Drop`, so the per-slot dropper
/// indirection runs and the heap allocations are freed. `LargeInput`
/// caps the batch size so the bench does not measure under allocator
/// pressure caused by many fully populated `String` pools coexisting.
fn raw_drop_full_string(c: &mut Criterion) {
    let mut group = c.benchmark_group("infinity_pool_focused/focused");

    group.bench_function("raw_drop_full_string", |b| {
        b.iter_batched(
            make_raw_pinned_pool_string_bulk,
            |pool| drop(black_box(pool)),
            BatchSize::LargeInput,
        );
    });

    group.finish();
}

/// Pairs with `pinned_pool_iter_full_10k`. Measures dense iteration
/// over a fully populated 10 000-item `PinnedPool`. Iteration is
/// non-mutating, so the same pool is reused across iterations.
fn iter_full(c: &mut Criterion) {
    let mut group = c.benchmark_group("infinity_pool_focused/focused");

    group.bench_function("iter_full", |b| {
        b.iter_custom(|iters| {
            let pool = PinnedPool::<u64>::new();
            let anchors: Vec<PooledMut<u64>> = (0..ANCHOR_COUNT).map(|i| pool.insert(i)).collect();

            let start = Instant::now();
            for _ in 0..iters {
                pool.with_iter(|iter| {
                    for ptr in iter {
                        _ = black_box(ptr);
                    }
                });
            }
            let elapsed = start.elapsed();

            _ = black_box(&anchors);
            _ = black_box(&pool);
            elapsed
        });
    });

    group.finish();
}

/// Pairs with `pinned_pool_iter_sparse_10k`. Measures iteration over a
/// `PinnedPool` populated with 10 000 inserts where seven out of every
/// eight have been dropped, leaving 1 250 surviving handles scattered
/// across the slabs allocated for the original inserts. Exposes the
/// per-empty-slot scan cost. Iteration is non-mutating, so state is
/// reused across all iterations.
fn iter_sparse(c: &mut Criterion) {
    let mut group = c.benchmark_group("infinity_pool_focused/focused");

    group.bench_function("iter_sparse", |b| {
        b.iter_custom(|iters| {
            let (pool, survivors) = make_sparse_pinned_pool();

            let start = Instant::now();
            for _ in 0..iters {
                pool.with_iter(|iter| {
                    for ptr in iter {
                        _ = black_box(ptr);
                    }
                });
            }
            let elapsed = start.elapsed();

            _ = black_box(&survivors);
            _ = black_box(&pool);
            elapsed
        });
    });

    group.finish();
}

fn make_blind_pool_with_5_layouts() -> (BlindPool, Vec<Box<dyn std::any::Any>>) {
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

    let pool = BlindPool::new();
    let anchors: Vec<Box<dyn std::any::Any>> = vec![
        Box::new(pool.insert::<L1>(L1(0))),
        Box::new(pool.insert::<L2>(L2(0))),
        Box::new(pool.insert::<L3>(L3(0))),
        Box::new(pool.insert::<L4>(L4(0))),
        Box::new(pool.insert::<L5>(L5([0; 3]))),
    ];
    (pool, anchors)
}

fn make_raw_pinned_pool_u64_bulk() -> RawPinnedPool<u64> {
    let mut pool = RawPinnedPool::<u64>::new();
    for i in 0..ANCHOR_COUNT {
        _ = pool.insert(i);
    }
    pool
}

fn make_raw_pinned_pool_string_bulk() -> RawPinnedPool<String> {
    let mut pool = RawPinnedPool::<String>::new();
    for i in 0..ANCHOR_COUNT {
        _ = pool.insert(format!("item-{i}"));
    }
    pool
}

fn make_sparse_pinned_pool() -> (PinnedPool<u64>, Vec<PooledMut<u64>>) {
    let pool = PinnedPool::<u64>::new();
    let all: Vec<PooledMut<u64>> = (0..ANCHOR_COUNT).map(|i| pool.insert(i)).collect();
    let mut survivors: Vec<PooledMut<u64>> = Vec::with_capacity(SPARSE_SURVIVOR_COUNT);
    for (idx, handle) in all.into_iter().enumerate() {
        if idx.is_multiple_of(8) {
            survivors.push(handle);
        } else {
            drop(handle);
        }
    }
    (pool, survivors)
}
