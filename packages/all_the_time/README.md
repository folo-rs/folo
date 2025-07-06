Processor time tracking utilities for benchmarks and performance analysis.

This package provides utilities to track processor time during code execution,
enabling analysis of processor usage patterns in benchmarks and performance tests.

```rust
use all_the_time::Session;

fn main() {
    let mut session = Session::new();

    // Track multiple iterations efficiently
    {
        let operation = session.operation("my_operation");
        let iterations = 1000;
        let _span = operation.iterations(iterations).measure_thread();
        
        for _ in 0..iterations {
            let mut sum = 0;
            for j in 0..100 {
                sum += j * j;
            }
            std::hint::black_box(sum);
        }
    } // Total time measured once and divided by iteration count for mean

    session.print_to_stdout();
}
```

More details in the [package documentation](https://docs.rs/all_the_time/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
