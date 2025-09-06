//! Benchmark comparing insertion performance with churn (insertions + removals)
#![allow(
    dead_code,
    clippy::collection_is_never_read,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::uninlined_format_args,
    clippy::explicit_counter_loop,
    clippy::integer_division,
    missing_docs,
    unused_doc_comments,
    reason = "Benchmark code, relax"
)]

use std::collections::VecDeque;
use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use infinity_pool::*;

fn churn_insertion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("ip_churn_insertion_performance");

    // Vec<T> baseline with churn (insertion + removal pattern)
    group.bench_function("Vec<T>", |b| {
        b.iter_custom(|iters| {
            let mut vec = Vec::<u64>::with_capacity(iters as usize);

            let start = Instant::now();
            for i in 0..iters {
                vec.push(i);

                // Every 3rd iteration, remove the first element
                if i > 0 && i % 3 == 2 && !vec.is_empty() {
                    vec.remove(0);
                }
            }
            start.elapsed()
        });
    });

    // PinnedPool<T> (thread-safe, reference counted) with churn
    group.bench_function("PinnedPool", |b| {
        b.iter_custom(|iters| {
            let mut pool = PinnedPool::<u64>::new();
            pool.reserve(iters as usize);
            let mut handles = VecDeque::with_capacity(iters as usize);

            let start = Instant::now();
            for i in 0..iters {
                handles.push_back(pool.insert(i));

                // Every 3rd iteration, remove the first handle
                if i > 0 && i % 3 == 2 && !handles.is_empty() {
                    handles.pop_front();
                }
            }
            start.elapsed()
        });
    });

    // LocalPinnedPool<T> (single-threaded, reference counted) with churn
    group.bench_function("LocalPinnedPool", |b| {
        b.iter_custom(|iters| {
            let mut pool = LocalPinnedPool::<u64>::new();
            pool.reserve(iters as usize);
            let mut handles = VecDeque::with_capacity(iters as usize);

            let start = Instant::now();
            for i in 0..iters {
                handles.push_back(pool.insert(i));

                // Every 3rd iteration, remove the first handle
                if i > 0 && i % 3 == 2 && !handles.is_empty() {
                    handles.pop_front();
                }
            }
            start.elapsed()
        });
    });

    // RawPinnedPool<T> (raw, manual lifetime management) with churn
    group.bench_function("RawPinnedPool", |b| {
        b.iter_custom(|iters| {
            let mut pool = RawPinnedPool::<u64>::new();
            pool.reserve(iters as usize);
            let mut handles = VecDeque::with_capacity(iters as usize);

            let start = Instant::now();
            for i in 0..iters {
                handles.push_back(pool.insert(i));

                // Every 3rd iteration, remove the first handle
                if i > 0 && i % 3 == 2 && !handles.is_empty() {
                    if let Some(handle) = handles.pop_front() {
                        pool.remove_mut(handle);
                    }
                }
            }
            start.elapsed()
        });
    });

    // OpaquePool (thread-safe, reference counted, type-erased) with churn
    group.bench_function("OpaquePool", |b| {
        b.iter_custom(|iters| {
            let mut pool = OpaquePool::with_layout_of::<u64>();
            pool.reserve(iters as usize);
            let mut handles = VecDeque::with_capacity(iters as usize);

            let start = Instant::now();
            for i in 0..iters {
                handles.push_back(pool.insert(i));

                // Every 3rd iteration, remove the first handle
                if i > 0 && i % 3 == 2 && !handles.is_empty() {
                    handles.pop_front();
                }
            }
            start.elapsed()
        });
    });

    // LocalOpaquePool (single-threaded, reference counted, type-erased) with churn
    group.bench_function("LocalOpaquePool", |b| {
        b.iter_custom(|iters| {
            let mut pool = LocalOpaquePool::with_layout_of::<u64>();
            pool.reserve(iters as usize);
            let mut handles = VecDeque::with_capacity(iters as usize);

            let start = Instant::now();
            for i in 0..iters {
                handles.push_back(pool.insert(i));

                // Every 3rd iteration, remove the first handle
                if i > 0 && i % 3 == 2 && !handles.is_empty() {
                    handles.pop_front();
                }
            }
            start.elapsed()
        });
    });

    // RawOpaquePool (raw, manual lifetime management, type-erased) with churn
    group.bench_function("RawOpaquePool", |b| {
        b.iter_custom(|iters| {
            let mut pool = RawOpaquePool::with_layout_of::<u64>();
            pool.reserve(iters as usize);
            let mut handles = VecDeque::with_capacity(iters as usize);

            let start = Instant::now();
            for i in 0..iters {
                handles.push_back(pool.insert(i));

                // Every 3rd iteration, remove the first handle
                if i > 0 && i % 3 == 2 && !handles.is_empty() {
                    if let Some(handle) = handles.pop_front() {
                        pool.remove_mut(handle);
                    }
                }
            }
            start.elapsed()
        });
    });

    // BlindPool (thread-safe, reference counted, multiple types) with churn
    group.bench_function("BlindPool", |b| {
        b.iter_custom(|iters| {
            let mut pool = BlindPool::new();
            pool.reserve_for::<u64>(iters as usize);
            let mut handles = VecDeque::with_capacity(iters as usize);

            let start = Instant::now();
            for i in 0..iters {
                handles.push_back(pool.insert(i));

                // Every 3rd iteration, remove the first handle
                if i > 0 && i % 3 == 2 && !handles.is_empty() {
                    handles.pop_front();
                }
            }
            start.elapsed()
        });
    });

    // LocalBlindPool (single-threaded, reference counted, multiple types) with churn
    group.bench_function("LocalBlindPool", |b| {
        b.iter_custom(|iters| {
            let mut pool = LocalBlindPool::new();
            pool.reserve_for::<u64>(iters as usize);
            let mut handles = VecDeque::with_capacity(iters as usize);

            let start = Instant::now();
            for i in 0..iters {
                handles.push_back(pool.insert(i));

                // Every 3rd iteration, remove the first handle
                if i > 0 && i % 3 == 2 && !handles.is_empty() {
                    handles.pop_front();
                }
            }
            start.elapsed()
        });
    });

    // RawBlindPool (raw, manual lifetime management, multiple types) with churn
    group.bench_function("RawBlindPool", |b| {
        b.iter_custom(|iters| {
            let mut pool = RawBlindPool::new();
            pool.reserve_for::<u64>(iters as usize);
            let mut handles = VecDeque::with_capacity(iters as usize);

            let start = Instant::now();
            for i in 0..iters {
                handles.push_back(pool.insert(i));

                // Every 3rd iteration, remove the first handle
                if i > 0 && i % 3 == 2 && !handles.is_empty() {
                    if let Some(handle) = handles.pop_front() {
                        pool.remove_mut(handle);
                    }
                }
            }
            start.elapsed()
        });
    });

    group.finish();
}

/// Criterion benchmark group for churn insertion performance tests
criterion_group!(benches, churn_insertion_benchmark);
criterion_main!(benches);
