//! Benchmarks basic operations of the `region_cached!` macro.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::{hint::black_box, thread};

use benchmark_utils::{AbWorker, ThreadPool, bench_on_threadpool, bench_on_threadpool_ab};
use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use new_zealand::nz;
use region_cached::{RegionCachedCopyExt, RegionCachedExt, region_cached};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let one_thread = ThreadPool::new(
        &ProcessorSet::builder()
            .performance_processors_only()
            .take(nz!(1))
            .unwrap(),
    );

    let two_threads = ThreadPool::new(
        &ProcessorSet::builder()
            .same_memory_region()
            .performance_processors_only()
            .take(nz!(2))
            .unwrap(),
    );

    let all_threads = ThreadPool::default();

    // Not every system is going to have multiple memory regions, so only some can do this.
    let two_memory_regions = ProcessorSet::builder()
        .performance_processors_only()
        .different_memory_regions()
        .take(nz!(2))
        .map(|x| ThreadPool::new(&x));

    let mut group = c.benchmark_group("region_cached");

    group.bench_function("get_unpin", |b| {
        b.iter(|| {
            region_cached!(static VALUE: u32 = 99942);

            black_box(VALUE.get_cached());
        });
    });

    group.bench_function("set_unpin", |b| {
        b.iter(|| {
            region_cached!(static VALUE: u32 = 99942);

            VALUE.set_global(black_box(566));
        });
    });

    group.bench_function("get_pin", |b| {
        region_cached!(static VALUE: u32 = 99942);

        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_thread,
                iters,
                || (),
                |()| _ = black_box(VALUE.get_cached()),
            )
        });
    });

    group.bench_function("set_pin", |b| {
        region_cached!(static VALUE: u32 = 99942);

        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_thread,
                iters,
                || (),
                |()| VALUE.set_global(black_box(566)),
            )
        });
    });

    // Two threads perform "get" in a loop.
    group.bench_function("par_get", |b| {
        region_cached!(static VALUE: u32 = 99942);

        b.iter_custom(|iters| {
            bench_on_threadpool(
                &two_threads,
                iters,
                || (),
                |()| _ = black_box(VALUE.get_cached()),
            )
        });
    });

    // All threads perform "get" in a loop.
    group.bench_function("par_get_all", |b| {
        region_cached!(static VALUE: u32 = 99942);

        b.iter_custom(|iters| {
            bench_on_threadpool(
                &all_threads,
                iters,
                || (),
                |()| _ = black_box(VALUE.get_cached()),
            )
        });
    });

    if let Some(ref thread_pool) = two_memory_regions {
        // Two threads perform "get" in a loop.
        // Both threads work until both have hit the target iteration count.
        group.bench_function("par_get_2region", |b| {
            region_cached!(static VALUE: u32 = 99942);

            b.iter_custom(|iters| {
                bench_on_threadpool(
                    thread_pool,
                    iters,
                    || (),
                    |()| _ = black_box(VALUE.get_cached()),
                )
            });
        });
    }

    group.finish();

    let mut group = c.benchmark_group("region_cached_get_set_pin");

    // One thread performs "get" in a loop, another performs "set" in a loop.
    group.bench_function("par_get_set", |b| {
        region_cached!(static VALUE: u32 = 99942);

        b.iter_custom(|iters| {
            bench_on_threadpool_ab(
                &two_threads,
                iters,
                |_| (),
                |worker, ()| match worker {
                    AbWorker::A => _ = black_box(VALUE.get_cached()),
                    AbWorker::B => VALUE.set_global(black_box(566)),
                },
            )
        });
    });

    // One thread performs "with" in a loop, another performs "set" in a loop.
    // The "with" thread is slow, also doing some "other stuff" in the callback.
    group.bench_function("par_with_set_busy", |b| {
        region_cached!(static VALUE: u32 = 99942);

        b.iter_custom(|iters| {
            bench_on_threadpool_ab(
                &two_threads,
                iters,
                |_| (),
                |worker, ()| match worker {
                    AbWorker::A => VALUE.with_cached(|v| {
                        _ = black_box(*v);
                        thread::yield_now();
                    }),
                    AbWorker::B => VALUE.set_global(black_box(566)),
                },
            )
        });
    });

    if let Some(ref thread_pool) = two_memory_regions {
        // One thread performs "get" in a loop, another performs "set" in a loop.
        group.bench_function("par_get_set_2region", |b| {
            region_cached!(static VALUE: u32 = 99942);

            b.iter_custom(|iters| {
                bench_on_threadpool_ab(
                    thread_pool,
                    iters,
                    |_| (),
                    |worker, ()| match worker {
                        AbWorker::A => _ = black_box(VALUE.get_cached()),
                        AbWorker::B => VALUE.set_global(black_box(566)),
                    },
                )
            });
        });

        // One thread performs "with" in a loop, another performs "set" in a loop.
        // The "with" thread is slow, also doing some "other stuff" in the callback.
        group.bench_function("par_with_set_busy_2region", |b| {
            region_cached!(static VALUE: u32 = 99942);

            b.iter_custom(|iters| {
                bench_on_threadpool_ab(
                    thread_pool,
                    iters,
                    |_| (),
                    |worker, ()| match worker {
                        AbWorker::A => VALUE.with_cached(|v| {
                            _ = black_box(*v);
                            thread::yield_now();
                        }),
                        AbWorker::B => VALUE.set_global(black_box(566)),
                    },
                )
            });
        });
    }

    group.finish();
}
