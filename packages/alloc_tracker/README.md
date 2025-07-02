# alloc_tracker

Memory allocation tracking utilities for benchmarks and performance analysis.

This crate provides utilities to track memory allocations during code execution,
enabling analysis of allocation patterns in benchmarks and performance tests.

The core functionality includes:
- `Allocator` - A Rust memory allocator wrapper that enables allocation tracking
- `Session` - Configures allocation tracking and provides access to tracking data
- `Span` - Tracks memory allocation changes over a time period
- `Operation` - Calculates average memory allocation per operation

## Example

```rust
use std::alloc::System;

use alloc_tracker::{Allocator, Session, Span};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

fn main() {
    let session = Session::new();

    // Track a single operation
    let span = Span::new(&session);
    let data = vec![1, 2, 3, 4, 5]; // This allocates memory
    let delta = span.to_delta();
    println!("Allocated {} bytes", delta);

    // Session automatically cleans up when dropped
}
```

More details in the [package documentation](https://docs.rs/alloc_tracker/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
