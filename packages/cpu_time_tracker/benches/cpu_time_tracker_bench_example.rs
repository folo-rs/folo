//! CPU time tracking benchmarks demonstrating the `cpu_time_tracker` crate.
//!
//! This benchmark demonstrates how to track CPU time (duration per iteration)
//! in Criterion benchmarks using the `cpu_time_tracker` utilities.
#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use cpu_time_tracker::Session;
use criterion::{Criterion, criterion_group, criterion_main};
use std::cell::Cell;
use std::hint::black_box;
use std::time::Instant;

fn entrypoint(c: &mut Criterion) {
    let mut cpu_time = Session::new();
    let mut group = c.benchmark_group("cpu_time_tracker");

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
                        sum =
                            sum.checked_add(i.checked_mul(i).expect(
                                "multiplication should not overflow for small test values",
                            ))
                            .expect("addition should not overflow for small test values");
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
