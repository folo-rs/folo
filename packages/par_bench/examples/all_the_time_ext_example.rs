//! Example demonstrating processor time tracking with `par_bench`.

use all_the_time::Session;
use many_cpus::ProcessorSet;
use par_bench::{AllTheTimeExt, Run, ThreadPool};

fn main() {
    // Create a processor time tracking session
    let processor_time = Session::new();

    // Create a thread pool for running benchmarks
    let mut pool = ThreadPool::new(ProcessorSet::default());

    println!("Running processor time tracking example...");

    // Run a benchmark that tracks processor time
    let results = Run::new()
        .measure_processor_time(&processor_time, "cpu_intensive_work")
        .iter(|_| {
            // Perform processor-intensive work
            let mut sum = 0_u64;
            for i in 0_u64..10000 {
                sum = sum.wrapping_add(i.wrapping_mul(i)).wrapping_mul(17);
            }
            std::hint::black_box(sum);
        })
        .execute_on(&mut pool, 100);

    println!(
        "Benchmark completed with mean duration: {:?}",
        results.mean_duration()
    );

    // Print processor time statistics
    println!("\nProcessor time tracking results:");
    processor_time.print_to_stdout();

    // Access measurement outputs
    let measure_count = results.measure_outputs().count();
    println!("\nCollected {measure_count} processor time measurement reports");
}
