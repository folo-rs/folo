//! Example showing how to use the allocation tracker extension trait.
//!
//! Run with: `cargo run --example alloc_tracker_ext_example --features alloc_tracker`

#![allow(missing_docs, reason = "No need for API documentation in example code")]

use alloc_tracker::{Allocator, Session};
use many_cpus::ProcessorSet;
use par_bench::{AllocTrackerExt, Run, ThreadPool};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn main() {
    println!("par_bench Allocation Tracker Extension Example");
    println!("==============================================");
    println!();

    let allocs = Session::new();
    let mut pool = ThreadPool::new(ProcessorSet::default());

    println!("Running allocation tracking benchmark...");

    let results = Run::new()
        .measure_allocs(&allocs, "vector_allocation")
        .iter(|_| {
            // Allocate various sized vectors to generate allocation activity
            let _small = [1, 2, 3].to_vec();
            let _medium = vec![0; 100];
            let _large = String::from("Hello, world!").repeat(10);
        })
        .execute_on(&mut pool, 1000);

    println!("Benchmark completed in {:?}", results.mean_duration());
    println!();

    // Print allocation statistics
    allocs.print_to_stdout();

    println!();
    println!("✓ Successfully demonstrated allocation tracking extension trait");
}
