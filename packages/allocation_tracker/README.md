# allocation_tracker

Memory allocation tracking utilities for benchmarks and performance analysis.

This crate provides utilities to track memory allocations during code execution,
enabling measurement of memory usage patterns in benchmarks and performance tests.

The core functionality includes:
- `MemoryTracker` - An implementation of `tracking_allocator::AllocationTracker`
- `MemoryDeltaTracker` - Tracks memory allocation changes over a time period
- `AverageMemoryDelta` - Calculates average memory allocation per operation
- `MemoryUsageResults` - Collects and displays memory usage measurements

## Example

```rust
use allocation_tracker::{MemoryTracker, MemoryDeltaTracker};
use tracking_allocator::{AllocationRegistry, Allocator};
use std::alloc::System;

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

fn main() {
    // Set up tracking
    AllocationRegistry::set_global_tracker(MemoryTracker).unwrap();
    AllocationRegistry::enable_tracking();
    
    // Track a single operation
    let tracker = MemoryDeltaTracker::new();
    let data = vec![1, 2, 3, 4, 5]; // This allocates memory
    let bytes_allocated = tracker.to_delta();
    
    println!("Allocated {} bytes", bytes_allocated);
    
    // Clean up
    AllocationRegistry::disable_tracking();
}
```

More details in the [package documentation](https://docs.rs/allocation_tracker/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
