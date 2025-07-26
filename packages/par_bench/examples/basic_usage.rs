//! Basic usage example demonstrating the performance difference between single-threaded
//! and multi-threaded atomic operations.
//!
//! This example showcases how to use `par_bench` to measure the relative performance of
//! incrementing an atomic integer on 1 thread versus all available threads.

#![allow(missing_docs, reason = "No need for API documentation in example code")]

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use many_cpus::ProcessorSet;
use new_zealand::nz;
use par_bench::{Run, ThreadPool};

const ITERATIONS: u64 = 10_000;

fn main() {
    println!("par_bench Basic Usage Example");
    println!("============================");
    println!();
    println!("This example demonstrates the performance difference between");
    println!("single-threaded and multi-threaded atomic operations.");
    println!();

    // Create a single-threaded pool.
    let single_processor = ProcessorSet::builder()
        .take(nz!(1))
        .expect("at least one processor should be available");
    let single_thread_pool = ThreadPool::new(&single_processor);

    // Create a multi-threaded pool using all available processors.
    let multi_thread_pool = ThreadPool::default();

    println!(
        "Running {} iterations on 1 thread vs {} threads",
        ITERATIONS,
        multi_thread_pool.thread_count()
    );
    println!();

    // Test single-threaded performance.
    let single_threaded_duration = measure_atomic_increments(&single_thread_pool);
    println!("Single-threaded (1 thread): {single_threaded_duration:?}");

    // Test multi-threaded performance.
    let multi_threaded_duration = measure_atomic_increments(&multi_thread_pool);
    println!(
        "Multi-threaded ({} threads): {:?}",
        multi_thread_pool.thread_count(),
        multi_threaded_duration
    );

    // Calculate and display the speedup ratio.
    #[allow(
        clippy::cast_precision_loss,
        reason = "precision loss acceptable for display purposes"
    )]
    let speedup =
        single_threaded_duration.as_nanos() as f64 / multi_threaded_duration.as_nanos() as f64;

    println!();
    println!("Performance comparison:");
    println!("Speedup ratio: {speedup:.2}x");

    if speedup > 1.0 {
        println!("Multi-threaded execution was faster!");
    } else if speedup < 1.0 {
        println!("Single-threaded execution was faster (atomic contention overhead)!");
    } else {
        println!("Both approaches performed similarly.");
    }
}

/// Measures the time to perform atomic increments using the given thread pool.
fn measure_atomic_increments(pool: &ThreadPool) -> std::time::Duration {
    // Shared atomic counter that all threads will increment.
    let counter = Arc::new(AtomicU64::new(0));

    let run = Run::new()
        .prepare_thread_fn({
            let counter = Arc::clone(&counter);
            move |_run_meta| Arc::clone(&counter)
        })
        .prepare_iter_fn(|_run_meta, counter| Arc::clone(counter))
        .iter_fn(|counter: Arc<AtomicU64>| {
            // Increment the atomic counter and use black_box to prevent optimization.
            black_box(counter.fetch_add(1, Ordering::Relaxed));
        });

    let stats = run.execute_on(pool, ITERATIONS);

    // Verify that we performed the expected number of increments.
    let expected_total = ITERATIONS
        .checked_mul(pool.thread_count().get() as u64)
        .expect("iteration count overflow");

    let actual_total = counter.load(Ordering::Relaxed);
    assert_eq!(
        actual_total, expected_total,
        "Expected {expected_total} increments but got {actual_total}"
    );

    stats.mean_duration()
}
