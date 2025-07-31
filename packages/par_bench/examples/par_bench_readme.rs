//! Example code from the README.md file.
//!
//! This file contains the Rust code examples present in the README to verify that
//! the example code actually works and compiles correctly.

#![allow(missing_docs, reason = "No need for API documentation in example code")]
#![allow(
    clippy::redundant_closure_for_method_calls,
    reason = "Example code prioritizes clarity over micro-optimizations"
)]

use std::num::NonZero;
use std::sync::atomic::{AtomicU64, Ordering};

use many_cpus::ProcessorSet;
use par_bench::{Run, ThreadPool};

fn main() {
    println!("README example verification");
    println!("===========================");

    // Basic usage example from README.
    basic_usage_example();

    println!("All README examples executed successfully!");
}

const PROCESSOR_COUNT: NonZero<usize> = NonZero::new(2).unwrap();

fn basic_usage_example() {
    println!("Running basic usage example...");

    let two_processors = ProcessorSet::builder().take(PROCESSOR_COUNT);

    let Some(two_processors) = two_processors else {
        println!("No multi-threading available, this example cannot run.");
        return;
    };

    let mut two_threads = ThreadPool::new(&two_processors);

    // Shared atomic counter that all threads will increment.
    let counter = AtomicU64::new(0);

    // Execute 10,000 iterations across all threads.
    let stats = Run::new()
        .iter(|_| counter.fetch_add(1, Ordering::Relaxed))
        .execute_on(&mut two_threads, 10_000);

    // Get the mean duration for benchmark reporting.
    let duration = stats.mean_duration();

    println!("Basic usage completed in: {duration:?}");

    // Verify the counter was incremented the expected number of times.
    let expected_total = 10_000_u64
        .checked_mul(two_threads.thread_count().get() as u64)
        .expect("iteration count overflow");

    let actual_total = counter.load(Ordering::Relaxed);
    assert_eq!(
        actual_total, expected_total,
        "Expected {expected_total} increments but got {actual_total}"
    );

    println!("Counter verification passed: {actual_total} increments");
}
