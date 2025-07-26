//! Example code from the README.md file.
//!
//! This file contains the Rust code examples present in the README to verify that
//! the example code actually works and compiles correctly.

#![allow(missing_docs, reason = "No need for API documentation in example code")]
#![allow(
    clippy::redundant_closure_for_method_calls,
    reason = "Example code prioritizes clarity over micro-optimizations"
)]

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use par_bench::{ConfiguredRun, ThreadPool};

fn main() {
    println!("README example verification");
    println!("===========================");

    // Basic usage example from README.
    basic_usage_example();

    println!("All README examples executed successfully!");
}

fn basic_usage_example() {
    println!("Running basic usage example...");

    // Create a thread pool.
    let pool = ThreadPool::default();

    // Shared atomic counter that all threads will increment.
    let counter = Arc::new(AtomicU64::new(0));

    let run = ConfiguredRun::builder()
        .prepare_thread_fn({
            let counter = Arc::clone(&counter);
            move |_run_meta| Arc::clone(&counter)
        })
        .prepare_iter_fn(|_run_meta, counter| Arc::clone(counter))
        .iter_fn(|counter: Arc<AtomicU64>| {
            // Increment the atomic counter and use black_box to prevent optimization.
            black_box(counter.fetch_add(1, Ordering::Relaxed));
        })
        .build();

    // Execute 10,000 iterations across all threads.
    let stats = run.execute_on(&pool, 10_000);

    // Get the mean duration for benchmark reporting.
    let duration = stats.mean_duration();

    println!("Basic usage completed in: {duration:?}");

    // Verify the counter was incremented the expected number of times.
    let expected_total = 10_000_u64
        .checked_mul(pool.thread_count().get() as u64)
        .expect("iteration count overflow");

    let actual_total = counter.load(Ordering::Relaxed);
    assert_eq!(
        actual_total, expected_total,
        "Expected {expected_total} increments but got {actual_total}"
    );

    println!("Counter verification passed: {actual_total} increments");
}
