//! Basic benchmarks demonstrating the performance difference between single-threaded
//! and multi-threaded atomic operations using `par_bench`.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::sync::atomic::{AtomicU64, Ordering};

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use par_bench::{Run, ThreadPool};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("atomic_increments");

    let mut single_thread_pool = ThreadPool::new(&ProcessorSet::single());
    let mut multi_thread_pool = ThreadPool::new(&ProcessorSet::default());

    atomic_increments(&mut single_thread_pool, &mut group, "single_thread");
    atomic_increments(&mut multi_thread_pool, &mut group, "multi_thread");

    group.finish();
}

/// Benchmarks the time taken to perform atomic increments.
fn atomic_increments(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>, name: &str) {
    let counter = AtomicU64::new(0);

    Run::new()
        .iter(|_| counter.fetch_add(1, Ordering::Relaxed))
        .execute_criterion_on(pool, group, name);
}
