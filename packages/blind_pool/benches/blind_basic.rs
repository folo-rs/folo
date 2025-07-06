//! Basic benchmarks for the `blind_pool` package.

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

// Custom struct with same size as u64 but different alignment
#[repr(C, align(8))]
struct AlignedU32 {
    value: u32,
    _padding: u32,
}

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
            let _pooled = pool.insert(TEST_VALUE);
            pool
        });
    });

    group.bench_function("ten_thousand", |b| {
        b.iter(|| {
            let mut pool = BlindPool::new();
            for _ in 0..10_000 {
                let _pooled = pool.insert(TEST_VALUE);
            }
            pool
        });
    });

    group.finish();

    let mut mixed_group = c.benchmark_group("bp_types");

    mixed_group.bench_function("single_type", |b| {
        b.iter(|| {
            let mut pool = BlindPool::new();
            for i in 0_u64..1_000 {
                let _pooled = pool.insert(i);
            }
            pool
        });
    });

    mixed_group.bench_function("two_types_same_layout", |b| {
        b.iter(|| {
            let mut pool = BlindPool::new();
            for i in 0_u64..500 {
                let _pooled1 = pool.insert(i);
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "Intentional truncation for benchmark"
                )]
                let _pooled2 = pool.insert(AlignedU32 {
                    value: i as u32,
                    _padding: 0,
                });
            }
            pool
        });
    });

    mixed_group.finish();
}
