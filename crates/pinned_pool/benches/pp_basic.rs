//! Basic benchmarks for the `pinned_pool` crate.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::{collections::VecDeque, hint::black_box};

use criterion::{Criterion, criterion_group, criterion_main};
use pinned_pool::PinnedPool;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

type TestItem = usize;
const TEST_VALUE: TestItem = 1024;

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

            _ = pool.insert(TEST_VALUE);

            drop(pool);
        });
    });

    group.bench_function("ten_thousand", |b| {
        b.iter(|| {
            let mut pool = PinnedPool::<TestItem>::new();

            for _ in 0..10_000 {
                _ = pool.insert(TEST_VALUE);
            }

            drop(pool);
        });
    });

    group.bench_function("forward_10_back_5_times_1000", |b| {
        // We add 10 items, remove the first 5 and repeat this 1000 times.
        let mut pool = PinnedPool::<TestItem>::new();

        let mut all_keys = VecDeque::with_capacity(1000 * 10);

        b.iter(|| {
            for _ in 0..10 {
                all_keys.push_back(pool.insert(TEST_VALUE));
            }

            for _ in 0..5 {
                pool.remove(all_keys.pop_front().unwrap());
            }
        });
    });

    group.finish();

    let mut group = c.benchmark_group("pp_get");

    group.bench_function("one", |b| {
        let mut pool = PinnedPool::<TestItem>::new();
        let key = pool.insert(TEST_VALUE);

        b.iter(|| {
            black_box(pool.get(key));
        });
    });

    group.bench_function("ten_thousand", |b| {
        let mut pool = PinnedPool::<TestItem>::new();

        let mut key = None;

        for _ in 0..10_000 {
            key = Some(pool.insert(TEST_VALUE));
        }

        let key = key.unwrap();

        b.iter(|| {
            black_box(pool.get(key));
        });
    });

    group.finish();
}
