//! Processor time tracking benchmarks demonstrating the `all_the_time` package.
//!
//! This benchmark demonstrates how to track processor time (duration per iteration)
//! in Criterion benchmarks using the `all_the_time` utilities.
#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]
#![expect(
    clippy::arithmetic_side_effects,
    reason = "this is benchmark code that doesn't need production-level safety"
)]

use std::cell::Cell;
use std::hint::black_box;
use std::time::Instant;

use all_the_time::Session;
use criterion::{Criterion, criterion_group, criterion_main};

fn entrypoint(c: &mut Criterion) {
    let cpu_time = Session::new();
    let mut group = c.benchmark_group("all_the_time");

    let cell = Cell::new(1234);
    let read_cell_op = cpu_time.operation("read_cell");

    group.bench_function("read_cell", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();

            {
                let _span = read_cell_op.iterations(iters).measure_thread();

                for _ in 0..iters {
                    black_box(cell.get());
                }
            }

            start.elapsed()
        });
    });

    let string_op = cpu_time.operation("string_formatting");

    group.bench_function("string_formatting", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();

            {
                let _span = string_op.iterations(iters).measure_thread();

                for _ in 0..iters {
                    let part1 = black_box("Hello, ");
                    let part2 = black_box("world!");
                    let s = format!("{part1}{part2}!");
                    black_box(s);
                }
            }

            start.elapsed()
        });
    });

    let computation_op = cpu_time.operation("computation");

    group.bench_function("computation", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();

            {
                let _span = computation_op.iterations(iters).measure_thread();

                for _ in 0..iters {
                    let mut sum = 0_u64;

                    for i in 0..1000_u64 {
                        sum += i * i;
                    }

                    black_box(sum);
                }
            }

            start.elapsed()
        });
    });

    group.finish();
    cpu_time.print_to_stdout();
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
