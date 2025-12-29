Mechanisms for multithreaded benchmarking, designed for integration with Criterion or a similar benchmark framework.

This package provides low-overhead utilities for benchmarking operations across multiple threads, with features like thread pool reuse, flexible configuration, and seamless integration with popular benchmark frameworks.

```rust
use std::sync::atomic::{AtomicU64, Ordering};

use many_cpus::ProcessorSet;
use par_bench::{Run, ThreadPool};

// Create a thread pool with two processors.
let two_processors = ProcessorSet::builder()
    .take(std::num::NonZero::new(2).unwrap())
    .expect("need at least 2 processors for multi-threaded benchmarks");

let mut pool = ThreadPool::new(&two_processors);

// Shared atomic counter that all threads will increment.
let counter = AtomicU64::new(0);

// Execute 10,000 iterations across all threads.
let stats = Run::new()
    .iter(|_| counter.fetch_add(1, Ordering::Relaxed))
    .execute_on(&mut pool, 10_000);

// Get the mean duration for benchmark reporting.
let duration = stats.mean_duration();
```

## See also

More details in the [package documentation](https://docs.rs/par_bench/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
