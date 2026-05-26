//! Benchmark comparing insertion performance of standard library primitives
//! against `infinity_pool` primitives in the target scenario:
//! 1. Lots of insertions.
//! 2. Lots of removals at arbitrary points.
#![allow(
    dead_code,
    clippy::collection_is_never_read,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::explicit_counter_loop,
    clippy::undocumented_unsafe_blocks,
    clippy::integer_division,
    clippy::indexing_slicing,
    missing_docs,
    reason = "duty of care is slightly lowered for benchmark code"
)]

use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloc_tracker::Allocator;
use criterion::{Criterion, criterion_group, criterion_main};
use infinity_pool::*;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

/// Number of objects to pre-fill before starting the timed benchmark span.
/// This ensures we start from a "hot state" with existing items.
const INITIAL_ITEMS: u64 = 10_000;

/// Number of items to add and remove in each batch operation.
const BATCH_SIZE: u64 = 15;

/// Number of batch operations to perform during the timed span.
/// Each batch adds `BATCH_SIZE` items and removes `BATCH_SIZE` existing items.
const BATCH_COUNT: u64 = 10_000;

fn churn_insertion_benchmark(c: &mut Criterion) {
    let allocs = alloc_tracker::Session::new();

    let mut group = c.benchmark_group("ip_vs_std");

    // Criterion's default 5 seconds just goes not give the precision we need, we get constant
    // plus or minus 10% noise that just prevents any sort of fine-tuning.
    group.measurement_time(Duration::from_secs(20));
    group.sample_size(2000);

    // Box::pin() baseline with churn (insertion + removal pattern)
    let allocs_op = allocs.operation("Box::pin()");
    group.bench_function("Box::pin()", |b| {
        b.iter_custom(|iters| {
            let mut all_boxes = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_boxes.push(Vec::<Pin<Box<u64>>>::with_capacity(
                    (INITIAL_ITEMS + BATCH_SIZE) as usize,
                ));
            }

            // Pre-fill with initial items outside timed span
            for boxes in &mut all_boxes {
                for i in 0..INITIAL_ITEMS {
                    boxes.push(Box::pin(i));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for boxes in &mut all_boxes {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = INITIAL_ITEMS;

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new items
                    for _ in 0..BATCH_SIZE {
                        boxes.push(Box::pin(next_value));
                        next_value = next_value.wrapping_add(1);
                    }

                    // Remove BATCH_SIZE random existing items
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..boxes.len());
                        boxes.swap_remove(target_index);
                    }
                }
            }
            start.elapsed()
        });
    });

    // Rc::pin() baseline with churn (insertion + removal pattern)
    let allocs_op = allocs.operation("Rc::pin()");
    group.bench_function("Rc::pin()", |b| {
        b.iter_custom(|iters| {
            let mut all_rcs = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_rcs.push(Vec::<Pin<Rc<u64>>>::with_capacity(
                    (INITIAL_ITEMS + BATCH_SIZE) as usize,
                ));
            }

            // Pre-fill with initial items outside timed span
            for rcs in &mut all_rcs {
                for i in 0..INITIAL_ITEMS {
                    rcs.push(Rc::pin(i));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for rcs in &mut all_rcs {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = INITIAL_ITEMS;

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new items
                    for _ in 0..BATCH_SIZE {
                        rcs.push(Rc::pin(next_value));
                        next_value = next_value.wrapping_add(1);
                    }

                    // Remove BATCH_SIZE random existing items
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..rcs.len());
                        rcs.swap_remove(target_index);
                    }
                }
            }
            start.elapsed()
        });
    });

    // Arc::pin() baseline with churn (insertion + removal pattern)
    let allocs_op = allocs.operation("Arc::pin()");
    group.bench_function("Arc::pin()", |b| {
        b.iter_custom(|iters| {
            let mut all_arcs = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_arcs.push(Vec::<Pin<Arc<u64>>>::with_capacity(
                    (INITIAL_ITEMS + BATCH_SIZE) as usize,
                ));
            }

            // Pre-fill with initial items outside timed span
            for arcs in &mut all_arcs {
                for i in 0..INITIAL_ITEMS {
                    arcs.push(Arc::pin(i));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for arcs in &mut all_arcs {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = INITIAL_ITEMS;

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new items
                    for _ in 0..BATCH_SIZE {
                        arcs.push(Arc::pin(next_value));
                        next_value = next_value.wrapping_add(1);
                    }

                    // Remove BATCH_SIZE random existing items
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..arcs.len());
                        arcs.swap_remove(target_index);
                    }
                }
            }
            start.elapsed()
        });
    });

    // PinnedPool<T> (thread-safe, reference counted) with churn
    let allocs_op = allocs.operation("PinnedPool");
    group.bench_function("PinnedPool", |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_handles = Vec::with_capacity(iters as usize);

            // Pre-allocate pools and handle vectors outside timed span
            for _ in 0..iters {
                let pool = PinnedPool::<u64>::new();
                all_pools.push(pool);
                all_handles.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..INITIAL_ITEMS {
                    handles.push(pool.insert(i));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = INITIAL_ITEMS;

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new items
                    for _ in 0..BATCH_SIZE {
                        handles.push(pool.insert(next_value));
                        next_value = next_value.wrapping_add(1);
                    }

                    // Remove BATCH_SIZE random existing items
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..handles.len());
                        handles.swap_remove(target_index);
                    }
                }
            }
            start.elapsed()
        });
    });

    // LocalPinnedPool<T> (single-threaded, reference counted) with churn
    let allocs_op = allocs.operation("LocalPinnedPool");
    group.bench_function("LocalPinnedPool", |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_handles = Vec::with_capacity(iters as usize);

            // Pre-allocate pools and handle vectors outside timed span
            for _ in 0..iters {
                let pool = LocalPinnedPool::<u64>::new();
                all_pools.push(pool);
                all_handles.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..INITIAL_ITEMS {
                    handles.push(pool.insert(i));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = INITIAL_ITEMS;

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new items
                    for _ in 0..BATCH_SIZE {
                        handles.push(pool.insert(next_value));
                        next_value = next_value.wrapping_add(1);
                    }

                    // Remove BATCH_SIZE random existing items
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..handles.len());
                        handles.swap_remove(target_index);
                    }
                }
            }
            start.elapsed()
        });
    });

    // RawPinnedPool<T> (raw, manual lifetime management) with churn
    let allocs_op = allocs.operation("RawPinnedPool");
    group.bench_function("RawPinnedPool", |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_handles = Vec::with_capacity(iters as usize);

            // Pre-allocate pools and handle vectors outside timed span
            for _ in 0..iters {
                let pool = RawPinnedPool::<u64>::new();
                all_pools.push(pool);
                all_handles.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..INITIAL_ITEMS {
                    handles.push(pool.insert(i));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = INITIAL_ITEMS;

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new items
                    for _ in 0..BATCH_SIZE {
                        handles.push(pool.insert(next_value));
                        next_value = next_value.wrapping_add(1);
                    }

                    // Remove BATCH_SIZE random existing items
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..handles.len());
                        let handle = handles.swap_remove(target_index);
                        unsafe {
                            pool.remove(handle);
                        }
                    }
                }
            }
            start.elapsed()
        });
    });

    // OpaquePool (thread-safe, reference counted, type-erased) with churn
    let allocs_op = allocs.operation("OpaquePool");
    group.bench_function("OpaquePool", |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_handles = Vec::with_capacity(iters as usize);

            // Pre-allocate pools and handle vectors outside timed span
            for _ in 0..iters {
                let pool = OpaquePool::with_layout_of::<u64>();
                all_pools.push(pool);
                all_handles.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..INITIAL_ITEMS {
                    handles.push(pool.insert(i));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = INITIAL_ITEMS;

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new items
                    for _ in 0..BATCH_SIZE {
                        handles.push(pool.insert(next_value));
                        next_value = next_value.wrapping_add(1);
                    }

                    // Remove BATCH_SIZE random existing items
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..handles.len());
                        handles.swap_remove(target_index);
                    }
                }
            }
            start.elapsed()
        });
    });

    // LocalOpaquePool (single-threaded, reference counted, type-erased) with churn
    let allocs_op = allocs.operation("LocalOpaquePool");
    group.bench_function("LocalOpaquePool", |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_handles = Vec::with_capacity(iters as usize);

            // Pre-allocate pools and handle vectors outside timed span
            for _ in 0..iters {
                let pool = LocalOpaquePool::with_layout_of::<u64>();
                all_pools.push(pool);
                all_handles.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..INITIAL_ITEMS {
                    handles.push(pool.insert(i));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = INITIAL_ITEMS;

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new items
                    for _ in 0..BATCH_SIZE {
                        handles.push(pool.insert(next_value));
                        next_value = next_value.wrapping_add(1);
                    }

                    // Remove BATCH_SIZE random existing items
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..handles.len());
                        handles.swap_remove(target_index);
                    }
                }
            }
            start.elapsed()
        });
    });

    // RawOpaquePool (raw, manual lifetime management, type-erased) with churn
    let allocs_op = allocs.operation("RawOpaquePool");
    group.bench_function("RawOpaquePool", |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_handles = Vec::with_capacity(iters as usize);

            // Pre-allocate pools and handle vectors outside timed span
            for _ in 0..iters {
                let pool = RawOpaquePool::with_layout_of::<u64>();
                all_pools.push(pool);
                all_handles.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..INITIAL_ITEMS {
                    handles.push(pool.insert(i));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = INITIAL_ITEMS;

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new items
                    for _ in 0..BATCH_SIZE {
                        handles.push(pool.insert(next_value));
                        next_value = next_value.wrapping_add(1);
                    }

                    // Remove BATCH_SIZE random existing items
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..handles.len());
                        let handle = handles.swap_remove(target_index);
                        unsafe {
                            pool.remove(handle);
                        }
                    }
                }
            }
            start.elapsed()
        });
    });

    // BlindPool (thread-safe, reference counted, multiple types) with churn
    let allocs_op = allocs.operation("BlindPool");
    group.bench_function("BlindPool", |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_handles = Vec::with_capacity(iters as usize);

            // Pre-allocate pools and handle vectors outside timed span
            for _ in 0..iters {
                let pool = BlindPool::new();
                all_pools.push(pool);
                all_handles.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..INITIAL_ITEMS {
                    handles.push(pool.insert(i));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = INITIAL_ITEMS;

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new items
                    for _ in 0..BATCH_SIZE {
                        handles.push(pool.insert(next_value));
                        next_value = next_value.wrapping_add(1);
                    }

                    // Remove BATCH_SIZE random existing items
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..handles.len());
                        handles.swap_remove(target_index);
                    }
                }
            }
            start.elapsed()
        });
    });

    // BlindPool variants where the dispatch holds extra distinct layouts so the per-insert
    // dispatch traversal touches more entries. The churn pattern still operates on u64
    // values; the extra layouts only populate the dispatch and exercise the lookup path.
    // These pair 1:1 with the Callgrind scenarios `blind_pool_insert_into_10k_n{5,8,16}_layouts`.
    blind_pool_churn_with_extra_layouts(&mut group, &allocs, "BlindPool_5layouts", |pool| {
        drop(pool.insert::<u8>(0));
        drop(pool.insert::<u16>(0));
        drop(pool.insert::<u32>(0));
        drop(pool.insert::<u128>(0));
    });

    blind_pool_churn_with_extra_layouts(&mut group, &allocs, "BlindPool_8layouts", |pool| {
        drop(pool.insert::<u8>(0));
        drop(pool.insert::<u16>(0));
        drop(pool.insert::<u32>(0));
        drop(pool.insert::<u128>(0));
        drop(pool.insert::<[u8; 3]>([0; 3]));
        drop(pool.insert::<[u8; 5]>([0; 5]));
        drop(pool.insert::<[u8; 6]>([0; 6]));
    });

    blind_pool_churn_with_extra_layouts(&mut group, &allocs, "BlindPool_16layouts", |pool| {
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
    });

    // Adversarial wall-clock counterpart of the Callgrind
    // `blind_pool_insert_into_10k_n8_layouts_adversarial` scenario. The dispatch holds 8
    // entries. Each iteration of the inner timed loop performs a u128 insert immediately
    // before each u64 insert: the u128 insert always moves itself to the front of the
    // dispatch's MRU order, which forces the subsequent u64 insert to be the most-distant
    // entry in MRU order — exactly the worst-case linear scan the typical-case benches
    // never observe (because their tight u64-only loop keeps u64 permanently at front).
    blind_pool_churn_n8_layouts_adversarial(&mut group, &allocs);

    // Mixed-layout wall-clock scenario. The dispatch holds 8 entries; accesses are evenly
    // distributed across all 8 layouts in a pre-shuffled (deterministic) order, so no MRU
    // or hit-count-based predictor sees a recognizable pattern. This is the realistic
    // middle ground between the "one hot layout" common case (which existing N=8 benches
    // exercise) and the worst-case adversarial pattern.
    blind_pool_churn_n8_layouts_mixed(&mut group, &allocs);

    // LocalBlindPool (single-threaded, reference counted, multiple types) with churn
    let allocs_op = allocs.operation("LocalBlindPool");
    group.bench_function("LocalBlindPool", |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_handles = Vec::with_capacity(iters as usize);

            // Pre-allocate pools and handle vectors outside timed span
            for _ in 0..iters {
                let pool = LocalBlindPool::new();
                all_pools.push(pool);
                all_handles.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..INITIAL_ITEMS {
                    handles.push(pool.insert(i));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = INITIAL_ITEMS;

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new items
                    for _ in 0..BATCH_SIZE {
                        handles.push(pool.insert(next_value));
                        next_value = next_value.wrapping_add(1);
                    }

                    // Remove BATCH_SIZE random existing items
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..handles.len());
                        handles.swap_remove(target_index);
                    }
                }
            }
            start.elapsed()
        });
    });

    // RawBlindPool (raw, manual lifetime management, multiple types) with churn
    let allocs_op = allocs.operation("RawBlindPool");
    group.bench_function("RawBlindPool", |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_handles = Vec::with_capacity(iters as usize);

            // Pre-allocate pools and handle vectors outside timed span
            for _ in 0..iters {
                let pool = RawBlindPool::new();
                all_pools.push(pool);
                all_handles.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            // Pre-fill with initial items outside timed span
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..INITIAL_ITEMS {
                    handles.push(pool.insert(i));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = INITIAL_ITEMS;

                for _ in 0..BATCH_COUNT {
                    // Add BATCH_SIZE new items
                    for _ in 0..BATCH_SIZE {
                        handles.push(pool.insert(next_value));
                        next_value = next_value.wrapping_add(1);
                    }

                    // Remove BATCH_SIZE random existing items
                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..handles.len());
                        let handle = handles.swap_remove(target_index);
                        unsafe {
                            pool.remove(handle);
                        }
                    }
                }
            }
            start.elapsed()
        });
    });

    group.finish();

    allocs.print_to_stdout();
}

// Drives the BlindPool churn measurement for a pool that has been pre-populated with extra
// inner pools via `extra_layouts`. The inner pools persist in the dispatch even after the
// inserted handles drop, so the measured churn exercises a dispatch with N distinct
// `LayoutKey` entries (where N = 1 + number of extra layouts added by the closure).
fn blind_pool_churn_with_extra_layouts<F>(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    allocs: &alloc_tracker::Session,
    label: &'static str,
    extra_layouts: F,
) where
    F: Fn(&BlindPool),
{
    let allocs_op = allocs.operation(label);
    group.bench_function(label, |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_handles = Vec::with_capacity(iters as usize);

            for _ in 0..iters {
                let pool = BlindPool::new();
                extra_layouts(&pool);
                all_pools.push(pool);
                all_handles.push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..INITIAL_ITEMS {
                    handles.push(pool.insert(i));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = INITIAL_ITEMS;

                for _ in 0..BATCH_COUNT {
                    for _ in 0..BATCH_SIZE {
                        handles.push(pool.insert(next_value));
                        next_value = next_value.wrapping_add(1);
                    }

                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..handles.len());
                        handles.swap_remove(target_index);
                    }
                }
            }
            start.elapsed()
        });
    });
}

// Adversarial-MRU counterpart of `blind_pool_churn_with_extra_layouts`. Each measured
// u64 insert is preceded by a u128 insert. The u128 insert always moves itself to
// the front of the dispatch's MRU order, so each subsequent u64 insert must scan past
// every other entry before finding u64 — the worst case the MTF dispatch is designed
// to be tolerable but slower at. The dispatch holds 8 distinct layouts (7 extras + u64);
// the inserted u128 handle is one of those 7 extras, so the dispatch size is invariant.
// Both inserts contribute to the measured time; what matters across branches is the
// relative cost of the adversarial path.
fn blind_pool_churn_n8_layouts_adversarial(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    allocs: &alloc_tracker::Session,
) {
    let label = "BlindPool_8layouts_adversarial";
    let allocs_op = allocs.operation(label);
    group.bench_function(label, |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_u64_handles = Vec::with_capacity(iters as usize);
            let mut all_u128_handles = Vec::with_capacity(iters as usize);

            for _ in 0..iters {
                let pool = BlindPool::new();
                // 7 extra layouts so the dispatch holds 8 entries once u64 is added.
                drop(pool.insert::<u8>(0));
                drop(pool.insert::<u16>(0));
                drop(pool.insert::<u32>(0));
                drop(pool.insert::<[u8; 3]>([0; 3]));
                drop(pool.insert::<[u8; 5]>([0; 5]));
                drop(pool.insert::<[u8; 6]>([0; 6]));
                drop(pool.insert::<u128>(0));
                all_pools.push(pool);
                all_u64_handles
                    .push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
                all_u128_handles
                    .push(Vec::with_capacity((INITIAL_ITEMS + BATCH_SIZE) as usize));
            }

            for (pool, handles) in all_pools.iter_mut().zip(all_u64_handles.iter_mut()) {
                for i in 0..INITIAL_ITEMS {
                    handles.push(pool.insert::<u64>(i));
                }
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for ((pool, u64_handles), u128_handles) in all_pools
                .iter_mut()
                .zip(all_u64_handles.iter_mut())
                .zip(all_u128_handles.iter_mut())
            {
                let mut rng = SmallRng::seed_from_u64(42);
                let mut next_value = INITIAL_ITEMS;

                for _ in 0..BATCH_COUNT {
                    for _ in 0..BATCH_SIZE {
                        // The u128 insert bumps u128 to the front of MRU; the u64
                        // insert that follows is then the most distant entry.
                        u128_handles.push(pool.insert::<u128>(0));
                        u64_handles.push(pool.insert::<u64>(next_value));
                        next_value = next_value.wrapping_add(1);
                    }

                    for _ in 0..BATCH_SIZE {
                        let target_index = rng.random_range(0..u64_handles.len());
                        u64_handles.swap_remove(target_index);
                        let target_index = rng.random_range(0..u128_handles.len());
                        u128_handles.swap_remove(target_index);
                    }
                }
            }
            start.elapsed()
        });
    });
}

criterion_group!(benches, churn_insertion_benchmark);
criterion_main!(benches);

// Mixed-layout wall-clock counterpart of `blind_pool_churn_n8_layouts_adversarial`. The
// dispatch holds 8 distinct layouts, and each measured iteration inserts (and later
// removes) items distributed evenly across all 8 layouts in a pre-shuffled, deterministic
// order. The shuffle ensures no recognizable pattern that an MRU front-cache or
// hit-count-based reordering could lock onto, so the average per-insert scan position
// approximates N/2 rather than 0 (typical case) or N-1 (adversarial).
fn blind_pool_churn_n8_layouts_mixed(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    allocs: &alloc_tracker::Session,
) {
    let label = "BlindPool_8layouts_mixed";
    let allocs_op = allocs.operation(label);
    group.bench_function(label, |b| {
        b.iter_custom(|iters| {
            // Pre-build the access pattern: BATCH_COUNT * BATCH_SIZE entries, each in
            // [0, 8), with each value appearing the same number of times, shuffled with a
            // fixed seed so the order is deterministic but arbitrary.
            let total_accesses = (BATCH_COUNT * BATCH_SIZE) as usize;
            let access: Vec<u8> = {
                let mut a: Vec<u8> = (0..total_accesses).map(|i| (i % 8) as u8).collect();
                let mut shuffle_rng = SmallRng::seed_from_u64(7777);
                for i in (1..a.len()).rev() {
                    let j = shuffle_rng.random_range(0..=i);
                    a.swap(i, j);
                }
                a
            };

            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_handles: Vec<(
                Vec<BlindPooledMut<u8>>,
                Vec<BlindPooledMut<u16>>,
                Vec<BlindPooledMut<u32>>,
                Vec<BlindPooledMut<u64>>,
                Vec<BlindPooledMut<u128>>,
                Vec<BlindPooledMut<[u8; 3]>>,
                Vec<BlindPooledMut<[u8; 5]>>,
                Vec<BlindPooledMut<[u8; 6]>>,
            )> = Vec::with_capacity(iters as usize);

            for _ in 0..iters {
                let pool = BlindPool::new();
                let cap = INITIAL_ITEMS as usize / 8 + BATCH_SIZE as usize;
                let mut handles = (
                    Vec::with_capacity(cap),
                    Vec::with_capacity(cap),
                    Vec::with_capacity(cap),
                    Vec::with_capacity(cap),
                    Vec::with_capacity(cap),
                    Vec::with_capacity(cap),
                    Vec::with_capacity(cap),
                    Vec::with_capacity(cap),
                );
                // Pre-populate INITIAL_ITEMS distributed evenly across the 8 layouts so
                // each per-type slab is hot before the timed loop starts.
                for i in 0..INITIAL_ITEMS {
                    match (i % 8) as u8 {
                        0 => handles.0.push(pool.insert::<u8>(0)),
                        1 => handles.1.push(pool.insert::<u16>(0)),
                        2 => handles.2.push(pool.insert::<u32>(0)),
                        3 => handles.3.push(pool.insert::<u64>(0)),
                        4 => handles.4.push(pool.insert::<u128>(0)),
                        5 => handles.5.push(pool.insert::<[u8; 3]>([0; 3])),
                        6 => handles.6.push(pool.insert::<[u8; 5]>([0; 5])),
                        _ => handles.7.push(pool.insert::<[u8; 6]>([0; 6])),
                    }
                }
                all_pools.push(pool);
                all_handles.push(handles);
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                let mut rng = SmallRng::seed_from_u64(42);

                for batch_idx in 0..BATCH_COUNT {
                    let batch_start = (batch_idx * BATCH_SIZE) as usize;

                    // Insert BATCH_SIZE items, layout chosen by the pre-shuffled sequence.
                    for i in 0..BATCH_SIZE as usize {
                        match access[batch_start + i] {
                            0 => handles.0.push(pool.insert::<u8>(0)),
                            1 => handles.1.push(pool.insert::<u16>(0)),
                            2 => handles.2.push(pool.insert::<u32>(0)),
                            3 => handles.3.push(pool.insert::<u64>(0)),
                            4 => handles.4.push(pool.insert::<u128>(0)),
                            5 => handles.5.push(pool.insert::<[u8; 3]>([0; 3])),
                            6 => handles.6.push(pool.insert::<[u8; 5]>([0; 5])),
                            _ => handles.7.push(pool.insert::<[u8; 6]>([0; 6])),
                        }
                    }

                    // Remove BATCH_SIZE items, layout chosen by the same sequence; a
                    // uniformly random item within each type's handle vector is removed.
                    for i in 0..BATCH_SIZE as usize {
                        match access[batch_start + i] {
                            0 => {
                                let len = handles.0.len();
                                let target = rng.random_range(0..len);
                                handles.0.swap_remove(target);
                            }
                            1 => {
                                let len = handles.1.len();
                                let target = rng.random_range(0..len);
                                handles.1.swap_remove(target);
                            }
                            2 => {
                                let len = handles.2.len();
                                let target = rng.random_range(0..len);
                                handles.2.swap_remove(target);
                            }
                            3 => {
                                let len = handles.3.len();
                                let target = rng.random_range(0..len);
                                handles.3.swap_remove(target);
                            }
                            4 => {
                                let len = handles.4.len();
                                let target = rng.random_range(0..len);
                                handles.4.swap_remove(target);
                            }
                            5 => {
                                let len = handles.5.len();
                                let target = rng.random_range(0..len);
                                handles.5.swap_remove(target);
                            }
                            6 => {
                                let len = handles.6.len();
                                let target = rng.random_range(0..len);
                                handles.6.swap_remove(target);
                            }
                            _ => {
                                let len = handles.7.len();
                                let target = rng.random_range(0..len);
                                handles.7.swap_remove(target);
                            }
                        }
                    }
                }
            }
            start.elapsed()
        });
    });
}
