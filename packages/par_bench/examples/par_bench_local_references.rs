//! Example demonstrating the ability to reference local variables from the caller
//! in benchmark closures.

#![allow(missing_docs, reason = "No need for API documentation in example code")]
#![allow(
    clippy::arithmetic_side_effects,
    reason = "Example code with safe small arithmetic operations"
)]
#![allow(
    clippy::cast_sign_loss,
    reason = "Example code with known positive values"
)]

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use many_cpus::ProcessorSet;
use par_bench::{Run, ThreadPool};

fn main() {
    let mut pool = ThreadPool::new(&ProcessorSet::default());

    // Local data that we want to reference in our benchmark
    let local_data = vec![1, 2, 3, 4, 5];
    let multiplier = 42;
    let global_counter = Arc::new(AtomicU64::new(0));

    // Create a benchmark that references local variables
    let run = Run::new()
        .prepare_thread({
            // We can capture local variables directly
            let local_data = &local_data;
            let counter = &global_counter;
            move |_meta| (local_data.clone(), Arc::clone(counter))
        })
        .prepare_iter(|args| {
            let (data, counter) = args.thread_state();

            // Reference the local data and counter in each iteration setup
            (data.clone(), Arc::clone(counter), multiplier)
        })
        .iter(|mut args| {
            let (data, counter, mult): (Vec<i32>, Arc<AtomicU64>, i32) = args.take_iter_state();

            // The benchmark work that uses local references
            let sum: i32 = data.iter().map(|x| x * mult).sum();

            // Update the global counter
            counter.fetch_add(sum as u64, Ordering::Relaxed);

            // Use black_box to prevent optimization
            black_box(sum);
        });

    // Execute the benchmark
    let results = run.execute_on(&mut pool, 1000);

    println!("Benchmark completed!");
    println!("Mean duration: {:?}", results.mean_duration());

    let final_count = global_counter.load(Ordering::Relaxed);
    let expected_sum = local_data.iter().map(|x| x * multiplier).sum::<i32>() as u64;
    let expected_total = expected_sum * 1000 * pool.thread_count().get() as u64;

    println!("Global counter final value: {final_count}");
    println!("Expected total: {expected_total}");

    assert_eq!(
        final_count, expected_total,
        "Counter should equal expected total based on local data"
    );

    println!("✓ Successfully demonstrated local variable references in benchmarks!");
    println!("✓ The benchmark closures accessed:");
    println!("  - local_data vector: {local_data:?}");
    println!("  - multiplier: {multiplier}");
    println!("  - global_counter: shared between all threads");
}
