//! Example demonstrating combined resource usage tracking with `par_bench`.

#[cfg(feature = "all_the_time")]
use all_the_time::Session as TimeSession;
#[cfg(feature = "alloc_tracker")]
use alloc_tracker::{Allocator, Session as AllocSession};
use many_cpus::SystemHardware;
use par_bench::{ResourceUsageExt, Run, ThreadPool};

// Set up global allocator for allocation tracking (if feature is enabled)
#[cfg(feature = "alloc_tracker")]
#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn main() {
    // Create tracking sessions based on available features
    #[cfg(feature = "alloc_tracker")]
    let allocs = AllocSession::new();

    #[cfg(feature = "all_the_time")]
    let processor_time = TimeSession::new();

    // Create a thread pool for running benchmarks
    let mut pool = ThreadPool::new(SystemHardware::current().processors());

    println!("Running combined resource usage tracking example...");

    // Configure which measurements to track based on available features
    let results = Run::new()
        .measure_resource_usage("combined_work", |measure| {
            #[allow(
                unused_mut,
                reason = "Variable is conditionally mutated based on feature flags"
            )]
            let mut measure = measure;

            #[cfg(feature = "alloc_tracker")]
            {
                measure = measure.allocs(&allocs);
            }

            #[cfg(feature = "all_the_time")]
            {
                measure = measure.processor_time(&processor_time);
            }

            measure
        })
        .iter(|_| {
            // Perform work that both allocates memory and uses CPU time
            let mut data = Vec::with_capacity(1000);
            for i in 0_u64..1000 {
                data.push(i.wrapping_mul(i).wrapping_add(17));
            }

            // Additional CPU work
            let mut sum = 0_u64;
            for &value in &data {
                sum = sum.wrapping_add(value);
            }

            std::hint::black_box(sum);
        })
        .execute_on(&mut pool, 100);

    println!(
        "Benchmark completed with mean duration: {:?}",
        results.mean_duration()
    );

    // Print resource usage statistics
    #[cfg(feature = "alloc_tracker")]
    {
        allocs.print_to_stdout();
    }

    #[cfg(feature = "all_the_time")]
    {
        processor_time.print_to_stdout();
    }

    // Access measurement outputs
    let measure_count = results.measure_outputs().count();
    println!("\nCollected {measure_count} resource usage measurement reports");

    // Demonstrate accessing the individual reports
    for (i, output) in results.measure_outputs().enumerate() {
        #[cfg(feature = "alloc_tracker")]
        if let Some(_alloc_report) = output.allocs() {
            println!("Thread {i}: Allocation data available");
        }

        #[cfg(feature = "all_the_time")]
        if let Some(_time_report) = output.processor_time() {
            println!("Thread {i}: Processor time data available");
        }
    }
}
