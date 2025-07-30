//! Benchmark comparing `fast_time::Clock::now()` with `std::time::Instant::now()`.

#![expect(missing_docs, reason = "benchmarks do not require API documentation")]

use std::hint::black_box;
use std::time::Instant;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use fast_time::Clock;

/// Benchmark group comparing timestamp capture performance.
fn timestamp_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("timestamp_capture");

    // Setup for fast_time clock
    let clock = Clock::new();

    // Benchmark std::time::Instant::now()
    group.bench_with_input(BenchmarkId::new("std_instant", "now"), &(), |b, ()| {
        b.iter(|| {
            let instant = black_box(Instant::now());
            black_box(instant);
        });
    });

    // Benchmark fast_time::Clock::now()
    group.bench_with_input(BenchmarkId::new("fast_time_clock", "now"), &(), |b, ()| {
        b.iter(|| {
            let instant = black_box(clock.now());
            black_box(instant);
        });
    });

    group.finish();
}

criterion_group!(benches, timestamp_comparison);
criterion_main!(benches);
