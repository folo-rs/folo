//! Benchmarks to measure the compute overhead of `all_the_time` logic itself.
//!
//! These benchmarks measure the overhead of the tracking infrastructure by
//! benchmarking empty spans - spans that do not do any actual work but still incur
//! the measurement overhead.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use all_the_time::Session;
use criterion::{Criterion, criterion_group, criterion_main};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("all_the_time_overhead");

    // Baseline measurement - no tracking at all
    group.bench_function("baseline_empty", |b| {
        b.iter(|| {
            // Completely empty - just the black_box call
            black_box(());
        });
    });

    // all_the_time overhead measurements
    {
        let time_session = Session::new();

        let thread_op = time_session.operation("empty_thread_span");
        group.bench_function("thread_span_empty", |b| {
            b.iter(|| {
                let _span = thread_op.measure_thread();
                // Empty span - measures only the overhead of span creation/destruction
                black_box(());
            });
        });

        let process_op = time_session.operation("empty_process_span");
        group.bench_function("process_span_empty", |b| {
            b.iter(|| {
                let _span = process_op.measure_process();
                // Empty span - measures only the overhead of span creation/destruction
                black_box(());
            });
        });

        // Test batch overhead with different iteration counts
        let batch_op_10 = time_session.operation("empty_batch_span_10");
        group.bench_function("batch_span_empty_10_iterations", |b| {
            b.iter(|| {
                let _span = batch_op_10.measure_thread().iterations(10);
                // Empty span with 10 iterations - measures overhead amortized over 10 iterations
                black_box(());
            });
        });

        let batch_op_100 = time_session.operation("empty_batch_span_100");
        group.bench_function("batch_span_empty_100_iterations", |b| {
            b.iter(|| {
                let _span = batch_op_100.measure_thread().iterations(100);
                // Empty span with 100 iterations - measures overhead amortized over 100 iterations
                black_box(());
            });
        });

        let batch_op_1000 = time_session.operation("empty_batch_span_1000");
        group.bench_function("batch_span_empty_1000_iterations", |b| {
            b.iter(|| {
                let _span = batch_op_1000.measure_thread().iterations(1000);
                // Empty span with 1000 iterations - measures overhead amortized over 1000 iterations
                black_box(());
            });
        });
    }

    group.finish();
}
