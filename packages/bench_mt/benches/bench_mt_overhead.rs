//! Benchmark demonstrating `bench_mt` overhead with empty `iter_fn`.
//!
//! This benchmark measures the overhead of the `bench_mt` framework by using an empty `iter_fn`
//! that does nothing. The purpose is to demonstrate that there is no surprising overhead
//! from the framework itself when benchmarking trivial operations.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use bench_mt::{Run, ThreadPool};
use criterion::{Criterion, criterion_group, criterion_main};

criterion_group!(benches, bench_mt_overhead);
criterion_main!(benches);

fn bench_mt_overhead(c: &mut Criterion) {
    let thread_pool = ThreadPool::default();

    c.bench_function("bench_mt_overhead", |b| {
        b.iter_custom(|iters| {
            // A new benchmark run is constructed for every call into this callback,
            // as per the standard mechanism for using bench_mt with Criterion.
            let run = Run::builder()
                .iter_fn(|()| {
                    // Empty iter_fn - does absolutely nothing.
                    // We use black_box to prevent the compiler from optimizing this away.
                    black_box(());
                })
                .build();

            let stats = run.execute_on(&thread_pool, iters);
            stats.mean_duration()
        });
    });
}
