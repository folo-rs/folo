CPU time tracking utilities for benchmarks and performance analysis.

This package provides utilities to track CPU time during code execution,
enabling analysis of CPU usage patterns in benchmarks and performance tests.

```rust
use cpu_time_tracker::Session;

fn main() {
    let mut session = Session::new();

    // Track a single operation
    {
        let operation = session.operation("my_operation");
        let _span = operation.measure_thread();
        // Perform some CPU-intensive work
        let mut sum = 0;
        for i in 0..10000 {
            sum += i;
        }
    }

    // Print results
    session.print_to_stdout();

    // Session automatically cleans up when dropped
}
```

More details in the [package documentation](https://docs.rs/cpu_time_tracker/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
