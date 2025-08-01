//! Benchmarks basic operations of the `region_cached!` macro.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::thread;

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use new_zealand::nz;
use par_bench::{Run, ThreadPool};
use region_cached::{RegionCachedCopyExt, RegionCachedExt, region_cached};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut one_thread = ThreadPool::new(ProcessorSet::single());

    // Not every system is going to have at least two processors, so only some can do this.
    let mut two_threads_same_region = ProcessorSet::builder()
        .same_memory_region()
        .performance_processors_only()
        .take(nz!(2))
        .map(|x| ThreadPool::new(&x));

    // Not every system is going to have multiple memory regions, so only some can do this.
    let mut two_threads_different_region = ProcessorSet::builder()
        .performance_processors_only()
        .different_memory_regions()
        .take(nz!(2))
        .map(|x| ThreadPool::new(&x));

    let mut group = c.benchmark_group("region_cached");

    // We intentionally do NOT use the thread pool here because thread pool threads are
    // always pinned and this is specifically to measure performance on an unpinned thread.
    group.bench_function("get_unpin", |b| {
        b.iter(|| {
            region_cached!(static VALUE: u32 = 99942);

            black_box(VALUE.get_cached());
        });
    });

    // We intentionally do NOT use the thread pool here because thread pool threads are
    // always pinned and this is specifically to measure performance on an unpinned thread.
    group.bench_function("set_unpin", |b| {
        b.iter(|| {
            region_cached!(static VALUE: u32 = 99942);

            VALUE.set_global(black_box(566));
        });
    });

    Run::new()
        .iter(|_| {
            region_cached!(static VALUE: u32 = 99942);

            VALUE.get_cached()
        })
        .execute_criterion_on(&mut one_thread, &mut group, "get_pin");

    Run::new()
        .iter(|_| {
            region_cached!(static VALUE: u32 = 99942);

            VALUE.set_global(black_box(566));
        })
        .execute_criterion_on(&mut one_thread, &mut group, "set_pin");

    // Two threads perform "get" in a loop.
    if let Some(ref mut thread_pool) = two_threads_same_region {
        Run::new()
            .iter(|_| {
                region_cached!(static VALUE: u32 = 99942);

                VALUE.get_cached()
            })
            .execute_criterion_on(thread_pool, &mut group, "par_get_same_region");
    }

    if let Some(ref mut thread_pool) = two_threads_different_region {
        // Two threads perform "get" in a loop.
        // Both threads work until both have hit the target iteration count.
        Run::new()
            .iter(|_| {
                region_cached!(static VALUE: u32 = 99942);

                VALUE.get_cached()
            })
            .execute_criterion_on(thread_pool, &mut group, "par_get_different_region");
    }

    group.finish();

    let mut group = c.benchmark_group("region_cached_get_set_pin");

    if let Some(ref mut thread_pool) = two_threads_same_region {
        // One thread performs "get" in a loop, another performs "set" in a loop.
        Run::new()
            .groups(nz!(2))
            .iter(|args| {
                region_cached!(static VALUE: u32 = 99942);

                if args.meta().group_index() == 0 {
                    _ = black_box(VALUE.get_cached());
                } else {
                    VALUE.set_global(black_box(566));
                }
            })
            .execute_criterion_on(thread_pool, &mut group, "par_get_set_same_region");

        // One thread performs "with" in a loop, another performs "set" in a loop.
        // The "with" thread is slow, also doing some "other stuff" in the callback.
        Run::new()
            .groups(nz!(2))
            .iter(|args| {
                region_cached!(static VALUE: u32 = 99942);

                if args.meta().group_index() == 0 {
                    VALUE.with_cached(|v| {
                        _ = black_box(*v);
                        thread::yield_now();
                    });
                } else {
                    VALUE.set_global(black_box(566));
                }
            })
            .execute_criterion_on(thread_pool, &mut group, "par_with_set_busy_same_region");
    }

    if let Some(ref mut thread_pool) = two_threads_different_region {
        // One thread performs "get" in a loop, another performs "set" in a loop.
        Run::new()
            .groups(nz!(2))
            .iter(|args| {
                region_cached!(static VALUE: u32 = 99942);

                if args.meta().group_index() == 0 {
                    _ = black_box(VALUE.get_cached());
                } else {
                    VALUE.set_global(black_box(566));
                }
            })
            .execute_criterion_on(thread_pool, &mut group, "par_get_set_different_region");

        // One thread performs "with" in a loop, another performs "set" in a loop.
        // The "with" thread is slow, also doing some "other stuff" in the callback.
        Run::new()
            .groups(nz!(2))
            .iter(|args| {
                region_cached!(static VALUE: u32 = 99942);

                if args.meta().group_index() == 0 {
                    VALUE.with_cached(|v| {
                        _ = black_box(*v);
                        thread::yield_now();
                    });
                } else {
                    VALUE.set_global(black_box(566));
                }
            })
            .execute_criterion_on(
                thread_pool,
                &mut group,
                "par_with_set_busy_different_region",
            );
    }

    group.finish();
}
