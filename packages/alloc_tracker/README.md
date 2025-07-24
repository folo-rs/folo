Memory allocation tracking utilities for benchmarks and performance analysis.

This package provides utilities to track memory allocations during code execution,
enabling analysis of allocation patterns in benchmarks and performance tests.

```rust
use alloc_tracker::{Allocator, Session};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn main() {
    let session = Session::new();

    // Track a single operation
    {
        let operation = session.operation("my_operation");
        let _span = operation.measure_process();
        let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
    }

    // Print results
    session.print_to_stdout();

    // Session automatically cleans up when dropped
}
```

More details in the [package documentation](https://docs.rs/alloc_tracker/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
