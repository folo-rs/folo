//! CPU time tracking benchmarks demonstrating the `cpu_time_tracker` crate.
//!
//! This benchmark demonstrates how to track CPU time (duration per iteration)
//! in Criterion benchmarks using the `cpu_time_tracker` utilities.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use cpu_time_tracker::Session;
use criterion::{Criterion, criterion_group, criterion_main};

fn entrypoint(c: &mut Criterion) {
    let mut cpu_times = Session::new();

    let mut group = c.benchmark_group("cpu_time_tracker");

    let string_op = cpu_times.operation("string_formatting");
    group.bench_function("string_formatting", |b| {
        b.iter(|| {
            let _span = string_op.thread_span();

            let part1 = black_box("Hello, ");
            let part2 = black_box("world!");
            let s = format!("{part1}{part2}!");
            black_box(s);
        });
    });

    let computation_op = cpu_times.operation("computation");
    group.bench_function("computation", |b| {
        b.iter(|| {
            let _span = computation_op.thread_span();

            let mut sum = 0_u64;
            for i in 0..1000_u64 {
                sum = sum
                    .checked_add(
                        i.checked_mul(i)
                            .expect("multiplication should not overflow for small test values"),
                    )
                    .expect("addition should not overflow for small test values");
            }
            black_box(sum);
        });
    });

    group.finish();

    cpu_times.print_to_stdout();
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
