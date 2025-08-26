Memory allocation tracking utilities for benchmarks and performance analysis.

This package provides utilities to track memory allocations during code execution,
enabling analysis of allocation patterns in benchmarks and performance tests.

## Features

- `panic_on_next_alloc`: Enables the `panic_on_next_alloc` function for debugging
  unexpected allocations. This feature adds some overhead to allocations, so it's optional.

## Basic usage

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

## Debugging unexpected allocations

The package also provides a panic-on-allocation feature to help track down unexpected
allocations in performance-critical code. This requires enabling the `panic_on_next_alloc` feature:

```toml
[dependencies]
alloc_tracker = { version = "0.5", features = ["panic_on_next_alloc"] }
```

```rust
use alloc_tracker::{panic_on_next_alloc, Allocator};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn main() {
    // Enable panic on allocation
    panic_on_next_alloc(true);
    
    // Any allocation attempt will now panic with a descriptive message
    // let _vec = vec![1, 2, 3]; // This would panic!
    
    // Disable to allow allocations again
    panic_on_next_alloc(false);
    let _vec = vec![1, 2, 3]; // This is safe
}
```

More details in the [package documentation](https://docs.rs/alloc_tracker/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
