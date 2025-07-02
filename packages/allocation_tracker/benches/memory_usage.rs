//! Memory usage tracking benchmarks demonstrating the allocation_tracker crate.
//!
//! This benchmark demonstrates how to track memory usage (bytes per iteration)
//! in Criterion benchmarks using the allocation_tracker utilities.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::alloc::System;
use std::hint::black_box;
use std::time::Instant;

use allocation_tracker::{AverageMemoryDelta, MemoryTracker, MemoryUsageResults};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use tracking_allocator::{AllocationRegistry, Allocator};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

fn entrypoint(c: &mut Criterion) {
    AllocationRegistry::set_global_tracker(MemoryTracker).unwrap();
    AllocationRegistry::enable_tracking();

    let mut memory_results = MemoryUsageResults::new();

    let mut group = c.benchmark_group("memory_usage");

    group.bench_function("string_formatting", |b| {
        let mut average_memory_delta = AverageMemoryDelta::new();

        b.iter(|| {
            let _contributor = average_memory_delta.contribute();

            let part1 = black_box("Hello, ");
            let part2 = black_box("world!");
            let s = format!("{part1}{part2}!");
            black_box(s);
        });

        memory_results.add(
            "string_formatting".to_string(),
            average_memory_delta.average(),
        );
    });

    group.bench_function("string_formatting_batched", |b| {
        let mut average_memory_delta = AverageMemoryDelta::new();

        b.iter_batched_ref(
            || (),
            |()| {
                let _contributor = average_memory_delta.contribute();

                let part1 = black_box("Hello, ");
                let part2 = black_box("world!");
                let s = format!("{part1}{part2}!");
                black_box(s);
            },
            BatchSize::SmallInput,
        );

        memory_results.add(
            "string_formatting_batched".to_string(),
            average_memory_delta.average(),
        );
    });

    group.bench_function("string_formatting_custom", |b| {
        let mut average_memory_delta = AverageMemoryDelta::new();

        b.iter_custom(|iters| {
            let start = Instant::now();

            for _ in 0..iters {
                let _contributor = average_memory_delta.contribute();

                let part1 = black_box("Hello, ");
                let part2 = black_box("world!");
                let s = format!("{part1}{part2}!");
                drop(black_box(s));
            }

            start.elapsed()
        });

        memory_results.add(
            "string_formatting_custom".to_string(),
            average_memory_delta.average(),
        );
    });

    group.bench_function("vector_allocation", |b| {
        let mut average_memory_delta = AverageMemoryDelta::new();

        b.iter(|| {
            let _contributor = average_memory_delta.contribute();

            let size = black_box(100);
            let data = vec![0u64; size];
            black_box(data);
        });

        memory_results.add(
            "vector_allocation".to_string(),
            average_memory_delta.average(),
        );
    });

    group.bench_function("box_allocation", |b| {
        let mut average_memory_delta = AverageMemoryDelta::new();

        b.iter(|| {
            let _contributor = average_memory_delta.contribute();

            let data = Box::new([0u8; 1000]);
            black_box(data);
        });

        memory_results.add("box_allocation".to_string(), average_memory_delta.average());
    });

    group.finish();

    AllocationRegistry::disable_tracking();

    println!("{memory_results}");
}
