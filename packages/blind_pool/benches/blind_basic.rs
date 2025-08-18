//! Basic benchmarks for the `blind_pool` package.
#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::iter;
use std::time::Instant;

use alloc_tracker::Allocator;
use blind_pool::RawBlindPool;
use criterion::{Criterion, criterion_group, criterion_main};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

const U64_TEST_VALUE: u64 = 1024;

// Custom struct with same layout as u64.
#[repr(C, align(8))]
struct AlmostU64 {
    value: u32,
    _padding: u32,
}

// Different type with different layout
type NotU64 = u32;
const NOT_U64_TEST_VALUE: NotU64 = 512;

fn entrypoint(c: &mut Criterion) {
    let allocs = alloc_tracker::Session::new();

    let mut group = c.benchmark_group("blind_basic");

    let allocs_op = allocs.operation("build_empty");
    group.bench_function("build_empty", |b| {
        b.iter_custom(|iters| {
            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                drop(black_box(RawBlindPool::new()));
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("insert_first");
    group.bench_function("insert_first", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(RawBlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for pool in &mut pools {
                _ = black_box(pool.insert(black_box(U64_TEST_VALUE)));
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("insert_second");
    group.bench_function("insert_second", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(RawBlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            // Pre-warm each pool with one item.
            for pool in &mut pools {
                _ = pool.insert(U64_TEST_VALUE);
            }

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for pool in &mut pools {
                _ = black_box(pool.insert(black_box(U64_TEST_VALUE)));
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("insert_first_two_types_same_layout");
    group.bench_function("insert_first_two_types_same_layout", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(RawBlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for pool in &mut pools {
                _ = black_box(pool.insert(black_box(U64_TEST_VALUE)));
                _ = black_box(pool.insert(black_box(AlmostU64 {
                    value: 42,
                    _padding: 0,
                })));
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("insert_second_two_types_same_layout");
    group.bench_function("insert_second_two_types_same_layout", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(RawBlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            // Pre-warm each pool with one item.
            for pool in &mut pools {
                _ = pool.insert(U64_TEST_VALUE);
            }

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for pool in &mut pools {
                _ = black_box(pool.insert(black_box(U64_TEST_VALUE)));
                _ = black_box(pool.insert(black_box(AlmostU64 {
                    value: 42,
                    _padding: 0,
                })));
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("insert_first_two_types_different_layout");
    group.bench_function("insert_first_two_types_different_layout", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(RawBlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for pool in &mut pools {
                _ = black_box(pool.insert(black_box(U64_TEST_VALUE)));
                _ = black_box(pool.insert(black_box(NOT_U64_TEST_VALUE)));
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("insert_second_two_types_different_layout");
    group.bench_function("insert_second_two_types_different_layout", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(RawBlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            // Pre-warm each pool with one item.
            for pool in &mut pools {
                _ = pool.insert(U64_TEST_VALUE);
            }

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for pool in &mut pools {
                _ = black_box(pool.insert(black_box(U64_TEST_VALUE)));
                _ = black_box(pool.insert(black_box(NOT_U64_TEST_VALUE)));
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("read_one");
    group.bench_function("read_one", |b| {
        b.iter_custom(|iters| {
            let mut pool = RawBlindPool::new();
            let pooled = pool.insert(U64_TEST_VALUE);

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                // SAFETY: The pointer is valid and the memory contains the value we just inserted.
                _ = black_box(unsafe { pooled.ptr().read() });
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("len");
    group.bench_function("len", |b| {
        b.iter_custom(|iters| {
            let mut pool = RawBlindPool::new();

            // Pre-populate pool with 10k items.
            for _ in 0..10_000 {
                _ = pool.insert(U64_TEST_VALUE);
            }

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                _ = black_box(pool.len());
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("is_empty");
    group.bench_function("is_empty", |b| {
        b.iter_custom(|iters| {
            let mut pool = RawBlindPool::new();

            // Pre-populate pool with 10k items.
            for _ in 0..10_000 {
                _ = pool.insert(U64_TEST_VALUE);
            }

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                _ = black_box(pool.is_empty());
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("capacity_of");
    group.bench_function("capacity_of", |b| {
        b.iter_custom(|iters| {
            let mut pool = RawBlindPool::new();

            // Pre-populate pool with 10k items.
            for _ in 0..10_000 {
                _ = pool.insert(U64_TEST_VALUE);
            }

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                _ = black_box(pool.capacity_of::<u64>());
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("read_one_two_types_same_layout");
    group.bench_function("read_one_two_types_same_layout", |b| {
        b.iter_custom(|iters| {
            let mut pool = RawBlindPool::new();
            let pooled1 = pool.insert(U64_TEST_VALUE);
            let pooled2 = pool.insert(AlmostU64 {
                value: 42,
                _padding: 0,
            });

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                // SAFETY: The pointers are valid and the memory contains the values we just inserted.
                _ = black_box(unsafe { pooled1.ptr().read() });
                // SAFETY: The pointers are valid and the memory contains the values we just inserted.
                _ = black_box(unsafe { pooled2.ptr().read() });
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("read_one_two_types_different_layout");
    group.bench_function("read_one_two_types_different_layout", |b| {
        b.iter_custom(|iters| {
            let mut pool = RawBlindPool::new();
            let pooled1 = pool.insert(U64_TEST_VALUE);
            let pooled2 = pool.insert(NOT_U64_TEST_VALUE);

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                // SAFETY: The pointers are valid and the memory contains the values we just inserted.
                _ = black_box(unsafe { pooled1.ptr().read() });
                // SAFETY: The pointers are valid and the memory contains the values we just inserted.
                _ = black_box(unsafe { pooled2.ptr().read() });
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("remove_one");
    group.bench_function("remove_one", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(RawBlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let pooled_items = pools
                .iter_mut()
                .map(|pool| pool.insert(U64_TEST_VALUE))
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for (pool, pooled) in pools.iter_mut().zip(pooled_items) {
                pool.remove(&pooled);
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("remove_one_two_types_same_layout");
    group.bench_function("remove_one_two_types_same_layout", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(RawBlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let pooled_pairs = pools
                .iter_mut()
                .map(|pool| {
                    let pooled1 = pool.insert(U64_TEST_VALUE);
                    let pooled2 = pool.insert(AlmostU64 {
                        value: 42,
                        _padding: 0,
                    });
                    (pooled1, pooled2)
                })
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for (pool, (pooled1, pooled2)) in pools.iter_mut().zip(pooled_pairs) {
                pool.remove(&pooled1);
                pool.remove(&pooled2);
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("remove_one_two_types_different_layout");
    group.bench_function("remove_one_two_types_different_layout", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(RawBlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let pooled_pairs = pools
                .iter_mut()
                .map(|pool| {
                    let pooled1 = pool.insert(U64_TEST_VALUE);
                    let pooled2 = pool.insert(NOT_U64_TEST_VALUE);
                    (pooled1, pooled2)
                })
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for (pool, (pooled1, pooled2)) in pools.iter_mut().zip(pooled_pairs) {
                pool.remove(&pooled1);
                pool.remove(&pooled2);
            }

            start.elapsed()
        });
    });

    group.finish();

    let mut group = c.benchmark_group("blind_slow");

    let allocs_op = allocs.operation("insert_10k");
    group.bench_function("insert_10k", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(RawBlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for pool in &mut pools {
                for _ in 0..10_000 {
                    _ = black_box(pool.insert(black_box(U64_TEST_VALUE)));
                }
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("forward_10_back_5_times_1000");
    group.bench_function("forward_10_back_5_times_1000", |b| {
        // We add 10 items, remove the first 5 and repeat this 1000 times.
        // This can stress the "seeking" and bookkeeping aspects of the pool.
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(RawBlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let mut to_remove = Vec::with_capacity(5);

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for pool in &mut pools {
                for _ in 0..1000 {
                    to_remove.clear();

                    // Add the 5 that we will later remove.
                    for _ in 0..5 {
                        let pooled = pool.insert(black_box(U64_TEST_VALUE));
                        to_remove.push(pooled);
                    }

                    // Add the 5 that we will keep.
                    for _ in 0..5 {
                        _ = black_box(pool.insert(black_box(U64_TEST_VALUE)));
                    }

                    // Remove the first 5.
                    #[expect(clippy::iter_with_drain, reason = "to avoid moving the value")]
                    for pooled in to_remove.drain(..) {
                        pool.remove(&pooled);
                    }
                }
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("remove_10k");
    group.bench_function("remove_10k", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(RawBlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let pooled_sets = pools
                .iter_mut()
                .map(|pool| {
                    iter::repeat_with(|| pool.insert(U64_TEST_VALUE))
                        .take(10_000)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for (pool, pooled_set) in pools.iter_mut().zip(&pooled_sets) {
                for pooled in pooled_set {
                    pool.remove(pooled);
                }
            }

            start.elapsed()
        });
    });

    group.finish();

    allocs.print_to_stdout();
}
