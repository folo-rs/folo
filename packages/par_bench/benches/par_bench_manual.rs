//! A form of the `par_bench_basic` example that is manually wired into the Criterion benchmark
//! framework, showcasing how custom logic can be injected for post-processing each run's
//! results (e.g. for additional validation or publishing that Criterion does not perform).

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use par_bench::{Run, ThreadPool};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("atomic_increments");

    let mut single_thread_pool = ThreadPool::new(ProcessorSet::single());

    group.bench_function("single_thread", |b| {
        b.iter_custom(|iters| black_box(measure_atomic_increments(&mut single_thread_pool, iters)));
    });

    let mut multi_thread_pool = ThreadPool::new(ProcessorSet::default());

    group.bench_function("multi_thread", |b| {
        b.iter_custom(|iters| black_box(measure_atomic_increments(&mut multi_thread_pool, iters)));
    });

    group.finish();
}

/// Measures the time to perform atomic increments using the given thread pool.
fn measure_atomic_increments(pool: &mut ThreadPool, iterations: u64) -> Duration {
    let counter = AtomicU64::new(0);

    let stats = Run::new()
        .iter(|_| counter.fetch_add(1, Ordering::Relaxed))
        .execute_on(pool, iterations);

    // Verify that we performed the expected number of increments.
    let expected_total = iterations
        .checked_mul(pool.thread_count().get() as u64)
        .expect("multiplication cannot overflow u64 as no computer can count that high");

    let actual_total = counter.load(Ordering::Relaxed);
    assert_eq!(
        actual_total, expected_total,
        "Expected {expected_total} increments but got {actual_total}"
    );

    stats.mean_duration()
}
