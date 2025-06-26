//! Benchmarks for `BlindPool` performance across different usage patterns.

#![allow(
    missing_docs,
    reason = "Criterion macros generate internal functions that don't need documentation"
)]

use blind_pool::BlindPool;
use criterion::{Criterion, criterion_group, criterion_main};

fn insert_remove_same_type(c: &mut Criterion) {
    c.bench_function("blind_pool insert/remove u64", |b| {
        b.iter(|| {
            let mut pool = BlindPool::new();
            let handles: Vec<_> = (0..1000_u64).map(|i| pool.insert(i)).collect();
            for handle in handles {
                pool.remove(handle);
            }
        });
    });
}

fn insert_remove_different_types(c: &mut Criterion) {
    c.bench_function("blind_pool insert/remove mixed types", |b| {
        b.iter(|| {
            let mut pool = BlindPool::new();

            let mut handles = Vec::new();
            for i in 0_u32..250 {
                handles.push(pool.insert(i).erase());
                handles.push(pool.insert(u64::from(i)).erase());
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "Intentional precision loss for benchmark data"
                )]
                let f32_val = i as f32;
                handles.push(pool.insert(f32_val).erase());
                handles.push(pool.insert(i64::from(i)).erase()); // Changed from String to i64
            }

            for handle in handles {
                pool.remove(handle);
            }
        });
    });
}

fn insert_only_same_type(c: &mut Criterion) {
    c.bench_function("blind_pool insert u64 only", |b| {
        b.iter(|| {
            let mut pool = BlindPool::new();
            for i in 0..1000_u64 {
                let _ = pool.insert(i);
            }
        });
    });
}

fn insert_only_different_types(c: &mut Criterion) {
    c.bench_function("blind_pool insert mixed types only", |b| {
        b.iter(|| {
            let mut pool = BlindPool::new();
            for i in 0_u32..250 {
                let _ = pool.insert(i);
                let _ = pool.insert(u64::from(i));
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "Intentional precision loss for benchmark data"
                )]
                let f32_val = i as f32;
                let _ = pool.insert(f32_val);
                let _ = pool.insert(i64::from(i)); // Changed from String to i64
            }
        });
    });
}

criterion_group!(
    benches,
    insert_remove_same_type,
    insert_remove_different_types,
    insert_only_same_type,
    insert_only_different_types
);
criterion_main!(benches);
