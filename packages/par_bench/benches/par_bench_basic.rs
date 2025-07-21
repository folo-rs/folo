//! Basic benchmarks demonstrating the performance difference between single-threaded
//! and multi-threaded atomic operations using par_bench.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use new_zealand::nz;
use par_bench::{Run, ThreadPool};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("atomic_increments");

    // Single-threaded benchmark.
    let single_processor = ProcessorSet::builder().take(nz!(1)).expect(
        "at least one processor must be available because this code is currently executing",
    );
    let single_thread_pool = ThreadPool::new(&single_processor);

    group.bench_function("single_thread", |b| {
        b.iter_custom(|iters| black_box(measure_atomic_increments(&single_thread_pool, iters)));
    });

    // Multi-threaded benchmark.
    let multi_thread_pool = ThreadPool::default();

    group.bench_function("multi_thread", |b| {
        b.iter_custom(|iters| black_box(measure_atomic_increments(&multi_thread_pool, iters)));
    });

    group.finish();
}

/// Measures the time to perform atomic increments using the given thread pool.
fn measure_atomic_increments(pool: &ThreadPool, iterations: u64) -> Duration {
    // Shared atomic counter that all threads will increment.
    let counter = Arc::new(AtomicU64::new(0));

    let run = Run::builder()
        .prepare_thread_fn({
            let counter = Arc::clone(&counter);
            move |_group_info| Arc::clone(&counter)
        })
        .prepare_iter_fn(|_group_info, counter| Arc::clone(counter))
        .iter_fn(|counter: Arc<AtomicU64>| {
            // Increment the atomic counter and use black_box to prevent optimization.
            black_box(counter.fetch_add(1, Ordering::Relaxed));
        })
        .build();

    let stats = run.execute_on(pool, iterations);

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
