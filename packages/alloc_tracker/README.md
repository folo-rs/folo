Memory allocation tracking utilities for benchmarks and performance analysis.

This package provides utilities to track memory allocations during code execution,
enabling analysis of allocation patterns in benchmarks and performance tests.

```rust
use alloc_tracker::{Allocator, Session, Span};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn main() {
    let session = Session::new();

    // Track a single operation
    {
        let span = Span::new(&session);
        let data = vec![1, 2, 3, 4, 5]; // This allocates memory
        let delta = span.to_delta();
        println!("Allocated {delta} bytes");
    }

    // Session automatically cleans up when dropped
}
```

More details in the [package documentation](https://docs.rs/alloc_tracker/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
