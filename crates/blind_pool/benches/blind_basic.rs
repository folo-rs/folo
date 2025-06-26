//! Basic benchmarks for the `blind_pool` crate.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use blind_pool::BlindPool;
use criterion::{Criterion, criterion_group, criterion_main};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

type TestItem = usize;
const TEST_VALUE: TestItem = 1024;

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("bp_fill");

    group.bench_function("empty", |b| {
        b.iter(|| {
            drop(black_box(BlindPool::new()));
        });
    });

    group.bench_function("one", |b| {
        b.iter(|| {
            let mut pool = BlindPool::new();
            let pooled = pool.insert(TEST_VALUE);
            pool.remove(pooled);
            drop(pool);
        });
    });

    group.bench_function("ten_thousand", |b| {
        b.iter(|| {
            let mut pool = BlindPool::new();
            let mut pooled_items = Vec::with_capacity(10_000);
            for _ in 0..10_000 {
                let pooled = pool.insert(TEST_VALUE);
                pooled_items.push(pooled);
            }
            for pooled in pooled_items {
                pool.remove(pooled);
            }
            drop(pool);
        });
    });

    group.finish();

    let mut mixed_group = c.benchmark_group("bp_mixed");

    mixed_group.bench_function("mixed_types", |b| {
        b.iter(|| {
            let mut pool = BlindPool::new();
            let mut handles = Vec::new();
            for i in 0_u32..1_000 {
                handles.push(pool.insert(i).erase());
                handles.push(pool.insert(u64::from(i)).erase());
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "Intentional precision loss for benchmark data"
                )]
                let f32_val = i as f32;
                handles.push(pool.insert(f32_val).erase());
                handles.push(pool.insert(i64::from(i)).erase());
            }
            for handle in handles {
                pool.remove(handle);
            }
            drop(pool);
        });
    });

    mixed_group.finish();
}
