//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use the `many_cpus_benchmarking` crate for benchmark harnesses.

use many_cpus_benchmarking::{Payload, WorkDistribution, execute_runs};

#[derive(Debug, Default)]
struct MyBenchmark {
    data: Option<Vec<u8>>,
}

impl Payload for MyBenchmark {
    fn new_pair() -> (Self, Self) {
        (Self::default(), Self::default())
    }

    fn prepare(&mut self) {
        self.data = Some(vec![99; 1024]);
    }

    fn process(&mut self) {
        // Your benchmark logic here
        let _sum: u32 = self
            .data
            .as_ref()
            .unwrap()
            .iter()
            .map(|&x| u32::from(x))
            .sum();
    }
}

// Run benchmarks with different work distribution modes
#[expect(dead_code, reason = "function shown for documentation purposes")]
fn benchmark(c: &mut criterion::Criterion) {
    execute_runs::<MyBenchmark, 1>(c, WorkDistribution::all());
}

fn main() {
    println!("=== Many CPUs Benchmarking README Example ===");

    // Create a dummy benchmark instance to demonstrate the structure
    let mut benchmark = MyBenchmark::default();
    benchmark.prepare();
    benchmark.process();

    println!("MyBenchmark prepared and processed successfully");
    println!("In real usage, this would be called by Criterion benchmark framework");
    println!("README example completed successfully!");
}
