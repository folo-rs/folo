//! Basic benchmarks for the `opaque_pool` crate.
#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::alloc::Layout;
use std::hint::black_box;
use std::iter;
use std::time::Instant;

use alloc_tracker::Allocator;
use criterion::{Criterion, criterion_group, criterion_main};
use opaque_pool::OpaquePool;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

type TestItem = usize;
const TEST_VALUE: TestItem = 1024;

fn entrypoint(c: &mut Criterion) {
    let allocs = alloc_tracker::Session::new();

    let mut group = c.benchmark_group("opaque_basic");

    let allocs_op = allocs.operation("build_empty");
    group.bench_function("build_empty", |b| {
        b.iter_custom(|iters| {
            let layout = Layout::new::<TestItem>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                drop(black_box(OpaquePool::builder().layout(layout).build()));
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("insert_one");
    group.bench_function("insert_one", |b| {
        b.iter_custom(|iters| {
            let layout = Layout::new::<TestItem>();

            let mut pools = iter::repeat_with(|| OpaquePool::builder().layout(layout).build())
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for pool in &mut pools {
                // SAFETY: The layout of TestItem matches the pool's layout.
                _ = black_box(unsafe { pool.insert(black_box(TEST_VALUE)) });
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("read_one");
    group.bench_function("read_one", |b| {
        b.iter_custom(|iters| {
            let layout = Layout::new::<TestItem>();

            let mut pool = OpaquePool::builder().layout(layout).build();

            // SAFETY: The layout of TestItem matches the pool's layout.
            let pool_ticket = unsafe { pool.insert(TEST_VALUE) };

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                // SAFETY: The layout of TestItem matches the pool's layout.
                _ = black_box(unsafe { pool_ticket.ptr().cast::<TestItem>().read() });
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("remove_one");
    group.bench_function("remove_one", |b| {
        b.iter_custom(|iters| {
            let layout = Layout::new::<TestItem>();

            let mut pools = iter::repeat_with(|| OpaquePool::builder().layout(layout).build())
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let pool_tickets = pools
                .iter_mut()
                .map(|pool| {
                    // SAFETY: The layout of TestItem matches the pool's layout.
                    unsafe { pool.insert(TEST_VALUE) }
                })
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for (pool, ticket) in pools.iter_mut().zip(pool_tickets) {
                pool.remove(ticket);
            }

            start.elapsed()
        });
    });

    group.finish();

    let mut group = c.benchmark_group("opaque_slow");

    let allocs_op = allocs.operation("insert_10k");
    group.bench_function("insert_10k", |b| {
        b.iter_custom(|iters| {
            let layout = Layout::new::<TestItem>();

            let mut pools = iter::repeat_with(|| OpaquePool::builder().layout(layout).build())
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for pool in &mut pools {
                for _ in 0..10_000 {
                    // SAFETY: The layout of TestItem matches the pool's layout.
                    _ = black_box(unsafe { pool.insert(black_box(TEST_VALUE)) });
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
            let layout = Layout::new::<TestItem>();

            let mut pools = iter::repeat_with(|| OpaquePool::builder().layout(layout).build())
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
                        // SAFETY: The layout of TestItem matches the pool's layout.
                        let pooled = unsafe { pool.insert(black_box(TEST_VALUE)) };

                        to_remove.push(pooled);
                    }

                    // Add the 5 that we will keep.
                    for _ in 0..5 {
                        // SAFETY: The layout of TestItem matches the pool's layout.
                        _ = black_box(unsafe { pool.insert(black_box(TEST_VALUE)) });
                    }

                    // Remove the first 5.
                    #[expect(clippy::iter_with_drain, reason = "to avoid moving the value")]
                    for pooled in to_remove.drain(..) {
                        pool.remove(pooled);
                    }
                }
            }

            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("remove_10k");
    group.bench_function("remove_10k", |b| {
        b.iter_custom(|iters| {
            let layout = Layout::new::<TestItem>();

            let mut pools = iter::repeat_with(|| OpaquePool::builder().layout(layout).build())
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            let pool_ticket_sets = pools
                .iter_mut()
                .map(|pool| {
                    iter::repeat_with(|| {
                        // SAFETY: The layout of TestItem matches the pool's layout.
                        unsafe { pool.insert(TEST_VALUE) }
                    })
                    .take(10_000)
                    .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for (pool, ticket_set) in pools.iter_mut().zip(&pool_ticket_sets) {
                for ticket in ticket_set {
                    pool.remove(*ticket);
                }
            }

            start.elapsed()
        });
    });

    group.finish();

    allocs.print_to_stdout();
}
