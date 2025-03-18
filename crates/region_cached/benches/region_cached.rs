use std::{hint::black_box, num::NonZero, thread};

use benchmark_utils::{AbWorker, ThreadPool, bench_on_threadpool, bench_on_threadpool_ab};
use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use region_cached::{RegionCachedCopyExt, RegionCachedExt, region_cached};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let one_thread = ThreadPool::new(
        ProcessorSet::builder()
            .same_memory_region()
            .performance_processors_only()
            .take(NonZero::new(1).unwrap())
            .unwrap(),
    );

    let two_threads = ThreadPool::new(
        ProcessorSet::builder()
            .same_memory_region()
            .performance_processors_only()
            .take(NonZero::new(2).unwrap())
            .unwrap(),
    );

    let all_threads = ThreadPool::all();

    // Not every system is going to have multiple memory regions, so only some can do this.
    let two_memory_regions = ProcessorSet::builder()
        .performance_processors_only()
        .different_memory_regions()
        .take(NonZero::new(2).unwrap())
        .map(ThreadPool::new);

    let mut group = c.benchmark_group("region_cached_unpinned");

    group.bench_function("get", |b| {
        b.iter(|| {
            region_cached!(static VALUE: u32 = 99942);

            black_box(VALUE.get_regional());
        })
    });

    group.bench_function("set", |b| {
        b.iter(|| {
            region_cached!(static VALUE: u32 = 99942);

            VALUE.set(black_box(566));
        })
    });

    group.finish();

    let mut group = c.benchmark_group("region_cached_pinned");

    group.bench_function("get", |b| {
        region_cached!(static VALUE: u32 = 99942);

        b.iter_custom(|iters| {
            bench_on_threadpool(
                &one_thread,
                iters,
                || (),
                |_| _ = black_box(VALUE.get_regional()),
            )
        });
    });

    group.bench_function("set", |b| {
        region_cached!(static VALUE: u32 = 99942);

        b.iter_custom(|iters| {
            bench_on_threadpool(&one_thread, iters, || (), |_| VALUE.set(black_box(566)))
        });
    });

    group.finish();

    let mut group = c.benchmark_group("region_cached_pinned_par");

    // Two threads perform "get" in a loop.
    group.bench_function("par_get", |b| {
        region_cached!(static VALUE: u32 = 99942);

        b.iter_custom(|iters| {
            bench_on_threadpool(
                &two_threads,
                iters,
                || (),
                |_| _ = black_box(VALUE.get_regional()),
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
                |_| _ = black_box(VALUE.get_regional()),
            )
        });
    });

    // One thread performs "get" in a loop, another performs "set" in a loop.
    group.bench_function("par_get_set", |b| {
        region_cached!(static VALUE: u32 = 99942);

        b.iter_custom(|iters| {
            bench_on_threadpool_ab(
                &two_threads,
                iters,
                |_| (),
                |worker, _| match worker {
                    AbWorker::A => _ = black_box(VALUE.get_regional()),
                    AbWorker::B => VALUE.set(black_box(566)),
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
                |worker, _| match worker {
                    AbWorker::A => VALUE.with_regional(|v| {
                        _ = black_box(*v);
                        thread::yield_now();
                    }),
                    AbWorker::B => VALUE.set(black_box(566)),
                },
            )
        });
    });

    if let Some(thread_pool) = two_memory_regions {
        // Two threads perform "get" in a loop.
        // Both threads work until both have hit the target iteration count.
        group.bench_function("par_get_2region", |b| {
            region_cached!(static VALUE: u32 = 99942);

            b.iter_custom(|iters| {
                bench_on_threadpool(
                    &thread_pool,
                    iters,
                    || (),
                    |_| _ = black_box(VALUE.get_regional()),
                )
            });
        });

        // One thread performs "get" in a loop, another performs "set" in a loop.
        group.bench_function("par_get_set_2region", |b| {
            region_cached!(static VALUE: u32 = 99942);

            b.iter_custom(|iters| {
                bench_on_threadpool_ab(
                    &thread_pool,
                    iters,
                    |_| (),
                    |worker, _| match worker {
                        AbWorker::A => _ = black_box(VALUE.get_regional()),
                        AbWorker::B => VALUE.set(black_box(566)),
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
                    &thread_pool,
                    iters,
                    |_| (),
                    |worker, _| match worker {
                        AbWorker::A => VALUE.with_regional(|v| {
                            _ = black_box(*v);
                            thread::yield_now();
                        }),
                        AbWorker::B => VALUE.set(black_box(566)),
                    },
                )
            });
        });
    }

    group.finish();
}
