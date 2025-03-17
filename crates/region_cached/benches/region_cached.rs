use std::{hint::black_box, num::NonZero};

use benchmark_utils::{AbWorker, ThreadPool, bench_on_threadpool, bench_on_threadpool_ab};
use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use region_cached::{RegionCachedCopyExt, RegionCachedExt, region_cached};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let two_threads = ThreadPool::new(
        ProcessorSet::builder()
            .same_memory_region()
            .performance_processors_only()
            .take(NonZero::new(2).unwrap())
            .unwrap(),
    );

    // Not every system is going to have multiple memory regions, so only some can do this.
    let two_memory_regions = ProcessorSet::builder()
        .performance_processors_only()
        .different_memory_regions()
        .take(NonZero::new(2).unwrap())
        .map(ThreadPool::new);

    let mut group = c.benchmark_group("region_cached");

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

    let mut group = c.benchmark_group("region_cached_par");

    // Two threads perform "get" in a loop.
    // Both threads work until both have hit the target iteration count.
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

    // One thread performs "get" in a loop, another performs "set" in a loop.
    // Both threads work until both have hit the target iteration count.
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
        // Both threads work until both have hit the target iteration count.
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
    }

    group.finish();
}
