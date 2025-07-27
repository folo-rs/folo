//! Basic benchmarks demonstrating the performance difference between single-threaded
//! and multi-threaded atomic operations using `par_bench`.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, criterion_group, criterion_main};
use many_cpus::ProcessorSet;
use new_zealand::nz;
use par_bench::{Run, ThreadPool};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("atomic_increments");

    let single_processor = ProcessorSet::builder().take(nz!(1)).expect(
        "at least one processor must be available because this code is currently executing",
    );
    let mut single_thread_pool = ThreadPool::new(&single_processor);
    let mut multi_thread_pool = ThreadPool::default();

    atomic_increments(&mut single_thread_pool, &mut group, "single_thread");
    atomic_increments(&mut multi_thread_pool, &mut group, "multi_thread");

    group.finish();
}

/// Benchmarks the time taken to perform atomic increments.
fn atomic_increments(pool: &mut ThreadPool, group: &mut BenchmarkGroup<'_, WallTime>, name: &str) {
    // Shared atomic counter that all threads will increment.
    let counter = Arc::new(AtomicU64::new(0));

    Run::new()
        .prepare_thread_fn({
            let counter = Arc::clone(&counter);
            move |_run_meta| Arc::clone(&counter)
        })
        .prepare_iter_fn(|_run_meta, counter| Arc::clone(counter))
        .iter_fn(|counter: Arc<AtomicU64>| {
            // Increment the atomic counter and use black_box to prevent optimization.
            black_box(counter.fetch_add(1, Ordering::Relaxed));
        })
        .execute_criterion_on(pool, group, name);
}
