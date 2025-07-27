//! Benchmarks basic operations of the `region_local!` macro.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::sync::LazyLock;
use std::thread;

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use new_zealand::nz;
use par_bench::{Run, ThreadPool};
use region_local::{RegionLocalCopyExt, RegionLocalExt, region_local};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut one_thread = ThreadPool::new(&ProcessorSet::single());

    // Not every system is going to have at least two processors, so only some can do this.
    let two_threads_same_region = ProcessorSet::builder()
        .same_memory_region()
        .performance_processors_only()
        .take(nz!(2))
        .map(|x| ThreadPool::new(&x));

    // Not every system is going to have multiple memory regions, so only some can do this.
    let two_threads_different_region = ProcessorSet::builder()
        .performance_processors_only()
        .different_memory_regions()
        .take(nz!(2))
        .map(|x| ThreadPool::new(&x));

    let mut group = c.benchmark_group("region_local");

    // We intentionally do NOT use the thread pool here because thread pool threads are
    // always pinned and this is specifically to measure performance on an unpinned thread.
    group.bench_function("get_unpin", |b| {
        b.iter(|| {
            region_local!(static VALUE: u32 = 99942);

            black_box(VALUE.get_local());
        });
    });

    // We intentionally do NOT use the thread pool here because thread pool threads are
    // always pinned and this is specifically to measure performance on an unpinned thread.
    group.bench_function("set_unpin", |b| {
        b.iter(|| {
            region_local!(static VALUE: u32 = 99942);

            VALUE.set_local(black_box(566));
        });
    });

    Run::new()
        .iter(|_| {
            region_local!(static VALUE: u32 = 99942);

            VALUE.get_local()
        })
        .execute_criterion_on(&mut one_thread, &mut group, "get_pin");

    Run::new()
        .iter(|_| {
            region_local!(static VALUE: u32 = 99942);

            VALUE.set_local(black_box(566));
        })
        .execute_criterion_on(&mut one_thread, &mut group, "set_pin");

    if let Some(ref mut thread_pool) = two_threads_same_region {
        Run::new()
            .iter(|_| {
                region_local!(static VALUE: u32 = 99942);

                VALUE.get_local()
            })
            .execute_criterion_on(thread_pool, &mut group, "get_pin_two");
    }

    if let Some(ref mut thread_pool) = two_threads_different_region {
        Run::new()
            .iter(|_| {
                region_local!(static VALUE: u32 = 99942);

                VALUE.get_local()
            })
            .execute_criterion_on(thread_pool, &mut group, "get_pin_two_different_region");
    }

    group.finish();

    let mut group = c.benchmark_group("region_local_get_set_pin");

    // One thread performs "get" in a loop, another performs "set" in a loop.
    if let Some(ref thread_pool) = two_threads {
        group.bench_function("par_get_set", |b| {
            region_local!(static VALUE: u32 = 99942);

            b.iter_custom(|iters| {
                bench_on_threadpool_ab(
                    thread_pool,
                    iters,
                    |_| (),
                    |worker, ()| match worker {
                        AbWorker::A => _ = black_box(VALUE.get_local()),
                        AbWorker::B => VALUE.set_local(black_box(566)),
                    },
                )
            });
        });
    }

    // One thread performs "with" in a loop, another performs "set" in a loop.
    // The "with" thread is slow, also doing some "other stuff" in the callback.
    if let Some(ref thread_pool) = two_threads {
        group.bench_function("par_with_set_busy", |b| {
            region_local!(static VALUE: u32 = 99942);

            b.iter_custom(|iters| {
                bench_on_threadpool_ab(
                    thread_pool,
                    iters,
                    |_| (),
                    |worker, ()| match worker {
                        AbWorker::A => VALUE.with_local(|v| {
                            _ = black_box(*v);
                            thread::yield_now();
                        }),
                        AbWorker::B => VALUE.set_local(black_box(566)),
                    },
                )
            });
        });
    }

    if let Some(ref thread_pool) = two_memory_regions {
        // One thread performs "get" in a loop, another performs "set" in a loop.
        group.bench_function("par_get_set_2region", |b| {
            region_local!(static VALUE: u32 = 99942);

            b.iter_custom(|iters| {
                bench_on_threadpool_ab(
                    thread_pool,
                    iters,
                    |_| (),
                    |worker, ()| match worker {
                        AbWorker::A => _ = black_box(VALUE.get_local()),
                        AbWorker::B => VALUE.set_local(black_box(566)),
                    },
                )
            });
        });

        // One thread performs "with" in a loop, another performs "set" in a loop.
        // The "with" thread is slow, also doing some "other stuff" in the callback.
        group.bench_function("par_with_set_busy_2region", |b| {
            region_local!(static VALUE: u32 = 99942);

            b.iter_custom(|iters| {
                bench_on_threadpool_ab(
                    thread_pool,
                    iters,
                    |_| (),
                    |worker, ()| match worker {
                        AbWorker::A => VALUE.with_local(|v| {
                            _ = black_box(*v);
                            thread::yield_now();
                        }),
                        AbWorker::B => VALUE.set_local(black_box(566)),
                    },
                )
            });
        });
    }

    group.finish();
}
