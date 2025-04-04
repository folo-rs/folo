use std::{hint::black_box, num::NonZero};

use benchmark_utils::{ThreadPool, bench_on_threadpool};
use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let thread_pool = ThreadPool::all();

    let mut group = c.benchmark_group("ProcessorSetBuilder");

    group.bench_function("all", |b| {
        b.iter(|| {
            black_box(ProcessorSet::builder().take_all().unwrap());
        })
    });

    group.bench_function("one", |b| {
        b.iter(|| {
            black_box(
                ProcessorSet::builder()
                    .take(NonZero::new(1).unwrap())
                    .unwrap(),
            );
        })
    });

    group.bench_function("only_evens", |b| {
        b.iter(|| {
            black_box(
                ProcessorSet::builder()
                    .filter(|p| p.id() % 2 == 0)
                    .take_all()
                    .unwrap(),
            );
        })
    });

    group.finish();

    let mut group = c.benchmark_group("ProcessorSetBuilder_MT");

    group.bench_function("all", |b| {
        b.iter_custom(|iters| {
            bench_on_threadpool(
                &thread_pool,
                iters,
                || (),
                |_| {
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
                |_| {
                    black_box(
                        ProcessorSet::builder()
                            .take(NonZero::new(1).unwrap())
                            .unwrap(),
                    );
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
                |_| {
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
