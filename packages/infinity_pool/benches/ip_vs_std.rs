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
    reason = "duty of care is slightly lowered for benchmark code"
)]

use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use alloc_tracker::Allocator;
use criterion::{Criterion, criterion_group, criterion_main};
use infinity_pool::*;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

/// Number of objects to create in each iteration of the benchmark.
/// Using a constant here ensures consistent collection fullness levels and size
/// across runs on different hardware, which may otherwise cause unhelpful variability.
const OBJECT_COUNT: u64 = 10_000;

fn churn_insertion_benchmark(c: &mut Criterion) {
    let allocs = alloc_tracker::Session::new();

    let mut group = c.benchmark_group("ip_vs_std");

    // Box::pin() baseline with churn (insertion + removal pattern)
    let allocs_op = allocs.operation("Box::pin()");
    group.bench_function("Box::pin()", |b| {
        b.iter_custom(|iters| {
            let mut all_boxes = Vec::with_capacity(iters as usize);

            // Pre-allocate per-iteration vectors outside timed span
            for _ in 0..iters {
                all_boxes.push(Vec::<Option<Pin<Box<u64>>>>::with_capacity(
                    OBJECT_COUNT as usize,
                ));
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for boxes in &mut all_boxes {
                for i in 0..OBJECT_COUNT {
                    boxes.push(Some(Box::pin(i)));

                    // Every 3rd iteration, remove the middle element
                    if i % 3 == 2 && !boxes.is_empty() {
                        let middle_index = boxes.len() / 2;
                        boxes[middle_index] = None;
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
                all_rcs.push(Vec::<Option<Pin<Rc<u64>>>>::with_capacity(
                    OBJECT_COUNT as usize,
                ));
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for rcs in &mut all_rcs {
                for i in 0..OBJECT_COUNT {
                    rcs.push(Some(Rc::pin(i)));

                    // Every 3rd iteration, remove the middle element
                    if i % 3 == 2 && !rcs.is_empty() {
                        let middle_index = rcs.len() / 2;
                        rcs[middle_index] = None;
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
                all_arcs.push(Vec::<Option<Pin<Arc<u64>>>>::with_capacity(
                    OBJECT_COUNT as usize,
                ));
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for arcs in &mut all_arcs {
                for i in 0..OBJECT_COUNT {
                    arcs.push(Some(Arc::pin(i)));

                    // Every 3rd iteration, remove the middle element
                    if i % 3 == 2 && !arcs.is_empty() {
                        let middle_index = arcs.len() / 2;
                        arcs[middle_index] = None;
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
                all_handles.push(Vec::<Option<_>>::with_capacity(OBJECT_COUNT as usize));
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..OBJECT_COUNT {
                    handles.push(Some(pool.insert(i)));

                    // Every 3rd iteration, remove the middle handle
                    if i % 3 == 2 && !handles.is_empty() {
                        let middle_index = handles.len() / 2;
                        handles[middle_index] = None;
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
                all_handles.push(Vec::<Option<_>>::with_capacity(OBJECT_COUNT as usize));
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..OBJECT_COUNT {
                    handles.push(Some(pool.insert(i)));

                    // Every 3rd iteration, remove the middle handle
                    if i % 3 == 2 && !handles.is_empty() {
                        let middle_index = handles.len() / 2;
                        handles[middle_index] = None;
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
                all_handles.push(Vec::<Option<_>>::with_capacity(OBJECT_COUNT as usize));
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..OBJECT_COUNT {
                    handles.push(Some(pool.insert(i)));

                    // Every 3rd iteration, remove the middle handle
                    if i % 3 == 2 && !handles.is_empty() {
                        let middle_index = handles.len() / 2;
                        if let Some(handle) = handles[middle_index].take() {
                            pool.remove_mut(handle);
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
                all_handles.push(Vec::<Option<_>>::with_capacity(OBJECT_COUNT as usize));
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..OBJECT_COUNT {
                    handles.push(Some(pool.insert(i)));

                    // Every 3rd iteration, remove the middle handle
                    if i % 3 == 2 && !handles.is_empty() {
                        let middle_index = handles.len() / 2;
                        handles[middle_index] = None;
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
                all_handles.push(Vec::<Option<_>>::with_capacity(OBJECT_COUNT as usize));
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..OBJECT_COUNT {
                    handles.push(Some(pool.insert(i)));

                    // Every 3rd iteration, remove the middle handle
                    if i % 3 == 2 && !handles.is_empty() {
                        let middle_index = handles.len() / 2;
                        handles[middle_index] = None;
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
                all_handles.push(Vec::<Option<_>>::with_capacity(OBJECT_COUNT as usize));
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..OBJECT_COUNT {
                    handles.push(Some(pool.insert(i)));

                    // Every 3rd iteration, remove the middle handle
                    if i % 3 == 2 && !handles.is_empty() {
                        let middle_index = handles.len() / 2;
                        if let Some(handle) = handles[middle_index].take() {
                            pool.remove_mut(handle);
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
                all_handles.push(Vec::<Option<_>>::with_capacity(OBJECT_COUNT as usize));
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..OBJECT_COUNT {
                    handles.push(Some(pool.insert(i)));

                    // Every 3rd iteration, remove the middle handle
                    if i % 3 == 2 && !handles.is_empty() {
                        let middle_index = handles.len() / 2;
                        handles[middle_index] = None;
                    }
                }
            }
            start.elapsed()
        });
    });

    // LocalBlindPool (single-threaded, reference counted, multiple types) with churn
    let allocs_op = allocs.operation("LocalOpaquePool");
    group.bench_function("LocalBlindPool", |b| {
        b.iter_custom(|iters| {
            let mut all_pools = Vec::with_capacity(iters as usize);
            let mut all_handles = Vec::with_capacity(iters as usize);

            // Pre-allocate pools and handle vectors outside timed span
            for _ in 0..iters {
                let pool = LocalBlindPool::new();
                all_pools.push(pool);
                all_handles.push(Vec::<Option<_>>::with_capacity(OBJECT_COUNT as usize));
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..OBJECT_COUNT {
                    handles.push(Some(pool.insert(i)));

                    // Every 3rd iteration, remove the middle handle
                    if i % 3 == 2 && !handles.is_empty() {
                        let middle_index = handles.len() / 2;
                        handles[middle_index] = None;
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
                all_handles.push(Vec::<Option<_>>::with_capacity(OBJECT_COUNT as usize));
            }

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (pool, handles) in all_pools.iter_mut().zip(all_handles.iter_mut()) {
                for i in 0..OBJECT_COUNT {
                    handles.push(Some(pool.insert(i)));

                    // Every 3rd iteration, remove the middle handle
                    if i % 3 == 2 && !handles.is_empty() {
                        let middle_index = handles.len() / 2;
                        if let Some(handle) = handles[middle_index].take() {
                            pool.remove_mut(handle);
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

/// Criterion benchmark group for churn insertion performance tests
criterion_group!(benches, churn_insertion_benchmark);
criterion_main!(benches);
