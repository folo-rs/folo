//! Basic benchmarks for the `pinned_pool` crate.
#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::iter;
use std::time::Instant;

use alloc_tracker::Allocator;
use criterion::{Criterion, criterion_group, criterion_main};
use pinned_pool::PinnedPool;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

type TestItem = usize;
const TEST_VALUE: TestItem = 1024;

fn entrypoint(c: &mut Criterion) {
    let allocs = alloc_tracker::Session::new();

    let mut group = c.benchmark_group("pinned_basic");

    let allocs_op = allocs.operation("build_empty");
    group.bench_function("build_empty", |b| {
        b.iter_custom(|iters| {
            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                drop(black_box(PinnedPool::<TestItem>::new()));
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("insert_first");
    group.bench_function("insert_first", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(PinnedPool::<TestItem>::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for pool in &mut pools {
                _ = black_box(pool.insert(black_box(TEST_VALUE)));
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("insert_second");
    group.bench_function("insert_second", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(PinnedPool::<TestItem>::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            // Pre-warm each pool with one item.
            for pool in &mut pools {
                _ = pool.insert(TEST_VALUE);
            }

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for pool in &mut pools {
                _ = black_box(pool.insert(black_box(TEST_VALUE)));
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("read_one");
    group.bench_function("read_one", |b| {
        b.iter_custom(|iters| {
            let mut pool = PinnedPool::<TestItem>::new();
            let key = pool.insert(TEST_VALUE);

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                _ = black_box(pool.get(key));
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("len");
    group.bench_function("len", |b| {
        b.iter_custom(|iters| {
            let mut pool = PinnedPool::<TestItem>::new();

            // Pre-populate pool with 10k items.
            for _ in 0..10_000 {
                _ = pool.insert(TEST_VALUE);
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
            let mut pool = PinnedPool::<TestItem>::new();

            // Pre-populate pool with 10k items.
            for _ in 0..10_000 {
                _ = pool.insert(TEST_VALUE);
            }

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                _ = black_box(pool.is_empty());
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("capacity");
    group.bench_function("capacity", |b| {
        b.iter_custom(|iters| {
            let mut pool = PinnedPool::<TestItem>::new();

            // Pre-populate pool with 10k items.
            for _ in 0..10_000 {
                _ = pool.insert(TEST_VALUE);
            }

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                _ = black_box(pool.capacity());
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("remove_one");
    group.bench_function("remove_one", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(PinnedPool::<TestItem>::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let keys = pools
                .iter_mut()
                .map(|pool| pool.insert(TEST_VALUE))
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for (pool, key) in pools.iter_mut().zip(keys) {
                pool.remove(key);
            }

            start.elapsed()
        });
    });

    group.finish();

    let mut group = c.benchmark_group("pinned_slow");

    let allocs_op = allocs.operation("insert_10k");
    group.bench_function("insert_10k", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(PinnedPool::<TestItem>::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for pool in &mut pools {
                for _ in 0..10_000 {
                    _ = black_box(pool.insert(black_box(TEST_VALUE)));
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
            let mut pools = iter::repeat_with(PinnedPool::<TestItem>::new)
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
                        let key = pool.insert(black_box(TEST_VALUE));
                        to_remove.push(key);
                    }

                    // Add the 5 that we will keep.
                    for _ in 0..5 {
                        _ = black_box(pool.insert(black_box(TEST_VALUE)));
                    }

                    // Remove the first 5.
                    #[expect(clippy::iter_with_drain, reason = "to avoid moving the value")]
                    for key in to_remove.drain(..) {
                        pool.remove(key);
                    }
                }
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("remove_10k");
    group.bench_function("remove_10k", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(PinnedPool::<TestItem>::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let key_sets = pools
                .iter_mut()
                .map(|pool| {
                    iter::repeat_with(|| pool.insert(TEST_VALUE))
                        .take(10_000)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for (pool, key_set) in pools.iter_mut().zip(&key_sets) {
                for key in key_set {
                    pool.remove(*key);
                }
            }

            start.elapsed()
        });
    });

    group.finish();

    allocs.print_to_stdout();
}
