//! Basic benchmarks for the `pinned_pool` crate.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use pinned_pool::PinnedPool;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

type TestItem = [u8; 1024];

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("pp_fill");

    group.bench_function("empty", |b| {
        b.iter(|| {
            drop(black_box(PinnedPool::<TestItem>::new()));
        });
    });

    group.bench_function("one", |b| {
        b.iter(|| {
            let mut pool = PinnedPool::<TestItem>::new();

            _ = pool.insert([0; 1024]);

            drop(pool);
        });
    });

    group.bench_function("ten_thousand", |b| {
        b.iter(|| {
            let mut pool = PinnedPool::<TestItem>::new();

            for _ in 0..10_000 {
                _ = pool.insert([0; 1024]);
            }

            drop(pool);
        });
    });

    group.finish();

    let mut group = c.benchmark_group("pp_get");

    group.bench_function("one", |b| {
        let mut pool = PinnedPool::<TestItem>::new();
        let key = pool.insert([0; 1024]);

        b.iter(|| {
            black_box(pool.get(key));
        });
    });

    group.bench_function("ten_thousand", |b| {
        let mut pool = PinnedPool::<TestItem>::new();

        let mut key = None;

        for _ in 0..10_000 {
            key = Some(pool.insert([0; 1024]));
        }

        let key = key.unwrap();

        b.iter(|| {
            black_box(pool.get(key));
        });
    });

    group.finish();
}
