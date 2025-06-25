//! Basic benchmarks for the `opaque_pool` crate.
#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::alloc::Layout;
use std::collections::VecDeque;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use opaque_pool::OpaquePool;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

type TestItem = usize;
const TEST_VALUE: TestItem = 1024;

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("dp_fill");

    group.bench_function("empty", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            drop(black_box(OpaquePool::new(layout)));
        });
    });

    group.bench_function("one", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = OpaquePool::new(layout);
            // SAFETY: The layout of TestItem matches the pool's layout.
            let pooled = unsafe { pool.insert(TEST_VALUE) };
            pool.remove(pooled);
            drop(pool);
        });
    });

    group.bench_function("ten_thousand", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = OpaquePool::new(layout);
            let mut pooled_items = Vec::with_capacity(10_000);
            for _ in 0..10_000 {
                // SAFETY: The layout of TestItem matches the pool's layout.
                let pooled = unsafe { pool.insert(TEST_VALUE) };
                pooled_items.push(pooled);
            }
            for pooled in pooled_items {
                pool.remove(pooled);
            }
            drop(pool);
        });
    });

    group.bench_function("forward_10_back_5_times_1000", |b| {
        // We add 10 items, remove the first 5 and repeat this 1000 times.
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = OpaquePool::new(layout);
            let mut all_pooled = VecDeque::with_capacity(1000 * 10);
            for _ in 0..1000 {
                for _ in 0..10 {
                    // SAFETY: The layout of TestItem matches the pool's layout.
                    let pooled = unsafe { pool.insert(TEST_VALUE) };
                    all_pooled.push_back(pooled);
                }
                for _ in 0..5 {
                    let pooled = all_pooled.pop_front().unwrap();
                    pool.remove(pooled);
                }
            }
            // Clean up remaining pooled items.
            while let Some(pooled) = all_pooled.pop_front() {
                pool.remove(pooled);
            }
        });
    });

    group.finish();

    let mut group = c.benchmark_group("dp_read");

    group.bench_function("one", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = OpaquePool::new(layout);
            // SAFETY: The layout of TestItem matches the pool's layout.
            let pooled = unsafe { pool.insert(TEST_VALUE) };
            // SAFETY: We just inserted the value and are reading the correct type.
            let value = unsafe { pooled.ptr().cast::<TestItem>().read() };
            black_box(value);
            pool.remove(pooled);
        });
    });

    group.bench_function("ten_thousand", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = OpaquePool::new(layout);
            let mut pooled_items = Vec::with_capacity(10_000);
            for _ in 0..10_000 {
                // SAFETY: The layout of TestItem matches the pool's layout.
                let pooled = unsafe { pool.insert(TEST_VALUE) };
                pooled_items.push(pooled);
            }
            let last_pooled = pooled_items.last().unwrap();
            // SAFETY: We inserted the value above and are reading the correct type.
            let value = unsafe { last_pooled.ptr().cast::<TestItem>().read() };
            black_box(value);
            for pooled in pooled_items {
                pool.remove(pooled);
            }
        });
    });

    group.finish();

    let mut group = c.benchmark_group("dp_insert");

    group.bench_function("single_insert", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = OpaquePool::new(layout);
            // SAFETY: The layout of TestItem matches the pool's layout.
            let pooled = unsafe { pool.insert(TEST_VALUE) };
            black_box(&pooled);
            pool.remove(pooled);
        });
    });

    group.bench_function("insert_with_read", |b| {
        b.iter(|| {
            let layout = Layout::new::<TestItem>();
            let mut pool = OpaquePool::new(layout);
            // SAFETY: The layout of TestItem matches the pool's layout.
            let pooled = unsafe { pool.insert(TEST_VALUE) };
            // SAFETY: We just inserted the value and are reading the correct type.
            let value = unsafe { pooled.ptr().cast::<TestItem>().read() };
            black_box((value, &pooled));
            pool.remove(pooled);
        });
    });

    group.finish();

    let mut group = c.benchmark_group("dp_remove");

    group.bench_function("single_remove", |b| {
        let layout = Layout::new::<TestItem>();
        b.iter_batched(
            || {
                let mut pool = OpaquePool::new(layout);
                // SAFETY: The layout of TestItem matches the pool's layout.
                let pooled = unsafe { pool.insert(TEST_VALUE) };
                (pool, pooled)
            },
            |(mut pool, pooled)| {
                pool.remove(pooled);
                black_box(pool);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function("batch_remove_100", |b| {
        let layout = Layout::new::<TestItem>();
        b.iter_batched(
            || {
                let mut pool = OpaquePool::new(layout);
                let mut pooled_items = Vec::with_capacity(100);
                for _ in 0..100 {
                    // SAFETY: The layout of TestItem matches the pool's layout.
                    let pooled = unsafe { pool.insert(TEST_VALUE) };
                    pooled_items.push(pooled);
                }
                (pool, pooled_items)
            },
            |(mut pool, pooled_items)| {
                for pooled in pooled_items {
                    pool.remove(pooled);
                }
                black_box(pool);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}
