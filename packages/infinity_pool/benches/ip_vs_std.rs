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
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(2000);

    // Box::pin() baseline with churn (insertion + removal pattern)
    let allocs_op = allocs.operation("Box::pin()");
    group.bench_function("Box::pin()", |b| {
        b.iter_custom(|iters| {
            let mut all_boxes = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_boxes.push(Vec::<Pin<Box<u64>>>::with_capacity(
                    (INITIAL_ITEMS + BATCH_COUNT * BATCH_SIZE) as usize,
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
                    (INITIAL_ITEMS + BATCH_COUNT * BATCH_SIZE) as usize,
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
                    (INITIAL_ITEMS + BATCH_COUNT * BATCH_SIZE) as usize,
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
                all_handles.push(Vec::with_capacity(
                    (INITIAL_ITEMS + BATCH_COUNT * BATCH_SIZE) as usize,
                ));
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
                all_handles.push(Vec::with_capacity(
                    (INITIAL_ITEMS + BATCH_COUNT * BATCH_SIZE) as usize,
                ));
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
                all_handles.push(Vec::with_capacity(
                    (INITIAL_ITEMS + BATCH_COUNT * BATCH_SIZE) as usize,
                ));
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
                        pool.remove_mut(handle);
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
                all_handles.push(Vec::with_capacity(
                    (INITIAL_ITEMS + BATCH_COUNT * BATCH_SIZE) as usize,
                ));
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
                all_handles.push(Vec::with_capacity(
                    (INITIAL_ITEMS + BATCH_COUNT * BATCH_SIZE) as usize,
                ));
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
                all_handles.push(Vec::with_capacity(
                    (INITIAL_ITEMS + BATCH_COUNT * BATCH_SIZE) as usize,
                ));
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
                        pool.remove_mut(handle);
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
                all_handles.push(Vec::with_capacity(
                    (INITIAL_ITEMS + BATCH_COUNT * BATCH_SIZE) as usize,
                ));
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
                all_handles.push(Vec::with_capacity(
                    (INITIAL_ITEMS + BATCH_COUNT * BATCH_SIZE) as usize,
                ));
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
                all_handles.push(Vec::with_capacity(
                    (INITIAL_ITEMS + BATCH_COUNT * BATCH_SIZE) as usize,
                ));
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
                        pool.remove_mut(handle);
                    }
                }
            }
            start.elapsed()
        });
    });

    group.finish();

    allocs.print_to_stdout();
}

criterion_group!(benches, churn_insertion_benchmark);
criterion_main!(benches);
