//! Memory allocation tracking benchmarks demonstrating the `alloc_tracker` crate.
//!
//! This benchmark demonstrates how to track memory allocations (bytes per iteration)
//! in Criterion benchmarks using the `alloc_tracker` utilities.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::alloc::System;
use std::hint::black_box;
use std::time::Instant;

use alloc_tracker::{Allocator, Operation, Session};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

fn entrypoint(c: &mut Criterion) {
    let alloc_tracker_session = Session::new();
    let mut all_operations = Vec::new();

    let mut group = c.benchmark_group("alloc_tracker");

    let mut operation = Operation::new("string_formatting");
    group.bench_function("string_formatting", |b| {
        b.iter(|| {
            let _span = operation.span(&alloc_tracker_session);

            let part1 = black_box("Hello, ");
            let part2 = black_box("world!");
            let s = format!("{part1}{part2}!");
            black_box(s);
        });
    });
    all_operations.push(operation);

    let mut operation = Operation::new("string_formatting_batched");
    group.bench_function("string_formatting_batched", |b| {
        b.iter_batched_ref(
            || (),
            |()| {
                let _span = operation.span(&alloc_tracker_session);

                let part1 = black_box("Hello, ");
                let part2 = black_box("world!");
                let s = format!("{part1}{part2}!");
                black_box(s);
            },
            BatchSize::SmallInput,
        );
    });
    all_operations.push(operation);

    let mut operation = Operation::new("string_formatting_custom");
    group.bench_function("string_formatting_custom", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();

            for _ in 0..iters {
                let _span = operation.span(&alloc_tracker_session);

                let part1 = black_box("Hello, ");
                let part2 = black_box("world!");
                let s = format!("{part1}{part2}!");
                drop(black_box(s));
            }

            start.elapsed()
        });
    });
    all_operations.push(operation);

    group.finish();

    println!("Memory allocation statistics:");

    for operation in all_operations {
        println!("{operation}");
    }
}
