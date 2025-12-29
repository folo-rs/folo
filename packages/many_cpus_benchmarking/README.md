[Criterion][1] benchmark harness designed to compare different modes of distributing work in a
many-processor system with multiple memory regions. This helps highlight the performance impact of
cross-memory-region data transfers, cross-processor data transfers and multi-threaded logic.

```rust
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
fn benchmark(c: &mut criterion::Criterion) {
    execute_runs::<MyBenchmark, 1>(c, WorkDistribution::all());
}
```

## See also

More details in the [package documentation](https://docs.rs/many_cpus_benchmarking/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.

[1]: https://bheisler.github.io/criterion.rs/book/index.html