//! Demonstration of using measure wrapper callbacks to track memory and processor time.
//!
//! This example shows how to use the `measure_wrapper_fns` feature of `par_bench` to capture
//! memory allocation and processor time statistics for the code being benchmarked. It creates
//! tracking sessions and spans in the wrapper start function that remain active during the
//! measured execution, then converts the sessions to reports in the wrapper end function.
//!
//! Run with: `cargo run --example measure_wrapper_tracking`.

#![allow(missing_docs, reason = "No need for API documentation in example code")]

use std::collections::HashMap;
use std::hint::black_box;

use all_the_time::{Report as TimeReport, Session as TimeSession};
use alloc_tracker::{Allocator, Report as AllocReport, Session as AllocSession};
use many_cpus::SystemHardware;
use par_bench::{Run, ThreadPool};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

const ITERATIONS: u64 = 100;

/// Combined tracking state that holds sessions and spans active during measurement.
#[derive(Debug)]
struct TrackingState {
    alloc_session: AllocSession,
    time_session: TimeSession,
    _alloc_span: alloc_tracker::ThreadSpan,
    _time_span: all_the_time::ThreadSpan,
}

/// Combined measurement output containing both allocation and processor time reports.
#[derive(Debug)]
struct MeasurementOutput {
    alloc_report: AllocReport,
    time_report: TimeReport,
}

fn main() {
    println!("par_bench Measure Wrapper Tracking Example");
    println!("==========================================");
    println!();
    println!("This example demonstrates how to use measure wrapper callbacks");
    println!("to track memory allocations and processor time consumption");
    println!("during benchmark execution.");
    println!();

    // Create a multi-threaded pool using all available processors.
    let mut multi_thread_pool = ThreadPool::new(SystemHardware::current().processors());
    println!(
        "Running {} iterations on {} threads with tracking enabled",
        ITERATIONS,
        multi_thread_pool.thread_count()
    );
    println!();

    // Execute the benchmark with tracking enabled.
    let results = run_benchmark_with_tracking(&mut multi_thread_pool);

    // Merge all the allocation reports from different threads.
    let mut merged_alloc_report = results
        .measure_outputs()
        .next()
        .expect("at least one measurement output should be available")
        .alloc_report
        .clone();

    for output in results.measure_outputs().skip(1) {
        merged_alloc_report = AllocReport::merge(&merged_alloc_report, &output.alloc_report);
    }

    // Merge all the processor time reports from different threads.
    let mut merged_time_report = results
        .measure_outputs()
        .next()
        .expect("at least one measurement output should be available")
        .time_report
        .clone();

    for output in results.measure_outputs().skip(1) {
        merged_time_report = TimeReport::merge(&merged_time_report, &output.time_report);
    }

    // Display the combined results.
    merged_alloc_report.print_to_stdout();
    println!();

    merged_time_report.print_to_stdout();
    println!();

    println!("✓ Successfully demonstrated measure wrapper tracking");
    println!(
        "✓ Combined allocation and processor time measurements from {} threads",
        multi_thread_pool.thread_count()
    );
}

/// Runs a benchmark with tracking enabled using measure wrapper callbacks.
fn run_benchmark_with_tracking(pool: &mut ThreadPool) -> par_bench::RunSummary<MeasurementOutput> {
    let run = Run::new()
        .measure_wrapper(
            // Wrapper begin function: creates tracking sessions and spans before the timed execution.
            |args| {
                let alloc_session = AllocSession::new();
                let time_session = TimeSession::new();

                // Create operations and start spans that will be active during all iterations.
                let alloc_operation = alloc_session.operation("benchmark_work");
                let time_operation = time_session.operation("benchmark_work");

                let alloc_span = alloc_operation
                    .measure_thread()
                    .iterations(args.meta().iterations());
                let time_span = time_operation
                    .measure_thread()
                    .iterations(args.meta().iterations());

                TrackingState {
                    alloc_session,
                    time_session,
                    _alloc_span: alloc_span,
                    _time_span: time_span,
                }
            },
            // Wrapper end function: drop spans first, then convert sessions to reports.
            |tracking_state| {
                // Explicitly drop the spans so their measurements are recorded in the sessions.
                drop(tracking_state._alloc_span);
                drop(tracking_state._time_span);

                // Now convert the sessions to reports.
                let alloc_report = tracking_state.alloc_session.to_report();
                let time_report = tracking_state.time_session.to_report();

                MeasurementOutput {
                    alloc_report,
                    time_report,
                }
            },
        )
        .iter(|_| {
            // Simulate some memory-intensive and processor-intensive work.
            simulate_benchmark_work();
        });

    run.execute_on(pool, ITERATIONS)
}

/// Simulates benchmark work that allocates memory and consumes processor time.
fn simulate_benchmark_work() {
    // Allocate various data structures to generate allocation activity.
    let mut data_structures = Vec::new();

    // Create some vectors with different sizes.
    for i in 0_usize..10 {
        let capacity = i
            .checked_mul(10)
            .expect("safe arithmetic: small loop indices cannot overflow");
        let mut vec = Vec::with_capacity(capacity);
        for j in 0..capacity {
            vec.push(format!("item_{i}_{j}"));
        }
        data_structures.push(vec);
    }

    // Create a hash map with string keys and values.
    let mut map = HashMap::new();
    for i in 0..20 {
        let key = format!("key_{i}");
        let value = format!("value_{i}_with_longer_content_to_increase_allocation");
        map.insert(key, value);
    }

    // Do some processor-intensive work on the data.
    let mut processed_count: usize = 0;
    for vec in &data_structures {
        for item in vec {
            // Simulate some processing work.
            let processed = item
                .chars()
                .rev()
                .collect::<String>()
                .to_uppercase()
                .repeat(2);
            processed_count = processed_count
                .checked_add(processed.len())
                .expect("safe arithmetic: small data sizes cannot overflow");
        }
    }

    // Process the hash map entries.
    for (key, value) in &map {
        let combined = format!("{key}:{value}");
        let hash = combined
            .chars()
            .map(|c| c as u32)
            .fold(0_u32, |acc, x| acc.wrapping_mul(31).wrapping_add(x));
        processed_count = processed_count
            .checked_add(hash as usize)
            .expect("safe arithmetic: hash values cannot cause overflow in this context");
    }

    // Use black_box to prevent optimization from removing our work.
    black_box(data_structures);
    black_box(map);
    black_box(processed_count);
}
