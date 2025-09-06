//! Benchmark comparing insertion performance across all pool types
#![allow(
    dead_code,
    clippy::collection_is_never_read,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::uninlined_format_args,
    missing_docs,
    unused_doc_comments,
    reason = "Benchmark code, relax"
)]

use criterion::{Criterion, criterion_group, criterion_main};
use infinity_pool::*;

fn insertion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("ip_insertion_performance");

    // Box::new() baseline for heap allocation
    group.bench_function("Box::new()", |b| {
        let mut counter = 0_u64;

        b.iter(|| {
            counter += 1;
            Box::new(counter)
        });
    });

    // Vec<T> baseline
    group.bench_function("Vec<T>", |b| {
        let mut vec = Vec::<u64>::new();
        let mut counter = 0_u64;

        b.iter(|| {
            counter += 1;
            vec.push(counter);
        });
    });

    // PinnedPool<T> (thread-safe, reference counted)
    group.bench_function("PinnedPool", |b| {
        let mut pool = PinnedPool::<u64>::new();
        let mut counter = 0_u64;

        b.iter(|| {
            counter += 1;
            pool.insert(counter)
        });
    });

    // LocalPinnedPool<T> (single-threaded, reference counted)
    group.bench_function("LocalPinnedPool", |b| {
        let mut pool = LocalPinnedPool::<u64>::new();
        let mut counter = 0_u64;

        b.iter(|| {
            counter += 1;
            pool.insert(counter)
        });
    });

    // RawPinnedPool<T> (raw, manual lifetime management)
    group.bench_function("RawPinnedPool", |b| {
        let mut pool = RawPinnedPool::<u64>::new();
        let mut counter = 0_u64;

        b.iter(|| {
            counter += 1;
            pool.insert(counter)
        });
    });

    // OpaquePool (thread-safe, reference counted, type-erased)
    group.bench_function("OpaquePool", |b| {
        let mut pool = OpaquePool::with_layout_of::<u64>();
        let mut counter = 0_u64;

        b.iter(|| {
            counter += 1;
            pool.insert(counter)
        });
    });

    // LocalOpaquePool (single-threaded, reference counted, type-erased)
    group.bench_function("LocalOpaquePool", |b| {
        let mut pool = LocalOpaquePool::with_layout_of::<u64>();
        let mut counter = 0_u64;

        b.iter(|| {
            counter += 1;
            pool.insert(counter)
        });
    });

    // RawOpaquePool (raw, manual lifetime management, type-erased)
    group.bench_function("RawOpaquePool", |b| {
        let mut pool = RawOpaquePool::with_layout_of::<u64>();
        let mut counter = 0_u64;

        b.iter(|| {
            counter += 1;
            pool.insert(counter)
        });
    });

    // BlindPool (thread-safe, reference counted, multiple types)
    group.bench_function("BlindPool", |b| {
        let mut pool = BlindPool::new();
        let mut counter = 0_u64;

        b.iter(|| {
            counter += 1;
            pool.insert(counter)
        });
    });

    // LocalBlindPool (single-threaded, reference counted, multiple types)
    group.bench_function("LocalBlindPool", |b| {
        let mut pool = LocalBlindPool::new();
        let mut counter = 0_u64;

        b.iter(|| {
            counter += 1;
            pool.insert(counter)
        });
    });

    // RawBlindPool (raw, manual lifetime management, multiple types)
    group.bench_function("RawBlindPool", |b| {
        let mut pool = RawBlindPool::new();
        let mut counter = 0_u64;

        b.iter(|| {
            counter += 1;
            pool.insert(counter)
        });
    });

    group.finish();
}

/// Criterion benchmark group for insertion performance tests
criterion_group!(benches, insertion_benchmark);
criterion_main!(benches);