//! Benchmarks for the bucket magnitude lookup in isolation.
//!
//! These benchmarks measure the performance of `find_bucket_index`, which is
//! the hot-path operation that determines which histogram bucket an observed
//! magnitude falls into.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use nm::Magnitude;
use nm::private::find_bucket_index;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

const SMALL_BUCKETS: &[Magnitude] = &[0, 10, 100, 1000, 10000];

const LARGE_BUCKETS: &[Magnitude] = &[
    0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072,
    262144, 524288, 1048576, 2097152, 4194304, 8388608, 16777216, 33554432, 67108864, 134217728,
    268435456, 536870912, 1073741824,
];

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("bucket_lookup");

    // Small bucket array (5 elements) - hit first bucket.
    group.bench_function("small_5_hit_first", |b| {
        b.iter(|| find_bucket_index(black_box(-1), black_box(SMALL_BUCKETS)));
    });

    // Small bucket array (5 elements) - hit last bucket.
    group.bench_function("small_5_hit_last", |b| {
        b.iter(|| find_bucket_index(black_box(5000), black_box(SMALL_BUCKETS)));
    });

    // Small bucket array (5 elements) - miss (exceeds all).
    group.bench_function("small_5_miss", |b| {
        b.iter(|| find_bucket_index(black_box(Magnitude::MAX), black_box(SMALL_BUCKETS)));
    });

    // Large bucket array (32 elements) - hit first bucket.
    group.bench_function("large_32_hit_first", |b| {
        b.iter(|| find_bucket_index(black_box(-1), black_box(LARGE_BUCKETS)));
    });

    // Large bucket array (32 elements) - hit middle bucket.
    group.bench_function("large_32_hit_middle", |b| {
        b.iter(|| find_bucket_index(black_box(100), black_box(LARGE_BUCKETS)));
    });

    // Large bucket array (32 elements) - hit last bucket.
    group.bench_function("large_32_hit_last", |b| {
        b.iter(|| find_bucket_index(black_box(1073741824), black_box(LARGE_BUCKETS)));
    });

    // Large bucket array (32 elements) - miss (exceeds all).
    group.bench_function("large_32_miss", |b| {
        b.iter(|| find_bucket_index(black_box(Magnitude::MAX), black_box(LARGE_BUCKETS)));
    });

    group.finish();
}
