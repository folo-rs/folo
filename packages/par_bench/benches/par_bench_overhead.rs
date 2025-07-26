//! Benchmark demonstrating `par_bench` overhead with empty `iter_fn`.
//!
//! This benchmark measures the overhead of the `par_bench` framework by using an empty `iter_fn`
//! that does nothing. The purpose is to demonstrate that there is no surprising overhead
//! from the framework itself when benchmarking trivial operations.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use par_bench::{ConfiguredRun, ThreadPool};

criterion_group!(benches, par_bench_overhead);
criterion_main!(benches);

fn par_bench_overhead(c: &mut Criterion) {
    let thread_pool = ThreadPool::default();

    c.bench_function("par_bench_overhead", |b| {
        b.iter_custom(|iters| {
            // A new benchmark run is constructed for every call into this callback,
            // as per the standard mechanism for using par_bench with Criterion.
            let run = ConfiguredRun::builder()
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
