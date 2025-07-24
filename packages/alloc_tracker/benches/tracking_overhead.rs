//! Benchmarks to measure the compute overhead of `alloc_tracker` logic itself.
//!
//! These benchmarks measure the overhead of the tracking infrastructure by
//! benchmarking empty spans - spans that don't do any actual work but still incur
//! the measurement overhead.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("alloc_tracker_overhead");

    // Baseline measurement - no tracking at all
    group.bench_function("baseline_empty", |b| {
        b.iter(|| {
            // Completely empty - just the black_box call
            black_box(());
        });
    });

    // alloc_tracker overhead measurements
    {
        let alloc_session = Session::new();

        let process_op = alloc_session.operation("empty_process_span");
        group.bench_function("process_span_empty", |b| {
            b.iter(|| {
                let _span = process_op.measure_process().iterations(1);
                // Empty span - measures only the overhead of span creation/destruction
                black_box(());
            });
        });

        let thread_op = alloc_session.operation("empty_thread_span");
        group.bench_function("thread_span_empty", |b| {
            b.iter(|| {
                let _span = thread_op.measure_thread().iterations(1);
                // Empty span - measures only the overhead of span creation/destruction
                black_box(());
            });
        });
    }

    group.finish();
}
