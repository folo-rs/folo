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
use many_cpus::SystemHardware;
use par_bench::{Run, ThreadPool};

criterion_group!(benches, par_bench_overhead);
criterion_main!(benches);

fn par_bench_overhead(c: &mut Criterion) {
    let mut thread_pool = ThreadPool::new(SystemHardware::current().processors());

    let mut group = c.benchmark_group("overhead");

    // A new benchmark run is constructed for every call into this callback,
    // as per the standard mechanism for using par_bench with Criterion.
    Run::new()
        .iter(|_| {
            // Empty iter_fn - does absolutely nothing.
            // We use black_box to try prevent the compiler from optimizing this away.
            black_box(());
        })
        .execute_criterion_on(&mut thread_pool, &mut group, "par_bench_overhead");
}
