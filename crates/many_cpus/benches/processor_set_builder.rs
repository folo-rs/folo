//! Benchmarking operations on the `ProcessorSetBuilder` type.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::time::Duration;

use benchmark_utils::{ThreadPool, bench_on_threadpool};
use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use new_zealand::nz;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let thread_pool = ThreadPool::default();

    let mut group = c.benchmark_group("ProcessorSetBuilder");

    // Results from this are really unstable for whatever reason. Give it more time to stabilize.
    group.measurement_time(Duration::from_secs(30));

    group.bench_function("all", |b| {
        b.iter(|| {
            black_box(ProcessorSet::builder().take_all().unwrap());
        });
    });

    group.bench_function("one", |b| {
        b.iter(|| {
            black_box(ProcessorSet::builder().take(nz!(1)).unwrap());
        });
    });

    group.bench_function("only_evens", |b| {
        b.iter(|| {
            black_box(
                ProcessorSet::builder()
                    .filter(|p| p.id() % 2 == 0)
                    .take_all()
                    .unwrap(),
            );
        });
    });

    group.finish();

    let mut group = c.benchmark_group("ProcessorSetBuilder_MT");

    // Results from this are really unstable for whatever reason. Give it more time to stabilize.
    group.measurement_time(Duration::from_secs(30));

    group.bench_function("all", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                || (),
                |()| {
                    black_box(ProcessorSet::builder().take_all().unwrap());
                },
            )
        });
    });

    group.bench_function("one", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                || (),
                |()| {
                    black_box(ProcessorSet::builder().take(nz!(1)).unwrap());
                },
            )
        });
    });

    group.bench_function("only_evens", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                || (),
                |()| {
                    black_box(
                        ProcessorSet::builder()
                            .filter(|p| p.id() % 2 == 0)
                            .take_all()
                            .unwrap(),
                    );
                },
            )
        });
    });

    group.finish();
}
