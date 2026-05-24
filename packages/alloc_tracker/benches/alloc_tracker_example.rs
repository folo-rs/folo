//! Memory allocation tracking benchmarks demonstrating the `alloc_tracker` package.
//!
//! This benchmark demonstrates how to track memory allocations (bytes per iteration)
//! in Criterion benchmarks using the `alloc_tracker` utilities.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn entrypoint(c: &mut Criterion) {
    let allocs = Session::new();

    let mut group = c.benchmark_group("alloc_tracker");

    let string_op = allocs.operation("string_formatting");
    group.bench_function("string_formatting", |b| {
        b.iter(|| {
            let _span = string_op.measure_process();

            let part1 = black_box("Hello, ");
            let part2 = black_box("world!");
            let s = format!("{part1}{part2}!");
            black_box(s);
        });
    });

    let vector_op = allocs.operation("vector_creation");
    group.bench_function("vector_creation", |b| {
        b.iter(|| {
            let _span = vector_op.measure_process();

            let data = (1..=100).collect::<Vec<_>>();
            black_box(data);
        });
    });

    group.finish();

    allocs.print_to_stdout();
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
