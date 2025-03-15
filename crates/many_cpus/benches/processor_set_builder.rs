use std::{hint::black_box, num::NonZero};

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
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
}
