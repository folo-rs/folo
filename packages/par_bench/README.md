Mechanisms for multithreaded benchmarking, designed for integration with Criterion or a similar benchmark framework.

This package provides low-overhead utilities for benchmarking operations across multiple threads, with features like thread pool reuse, flexible configuration, and seamless integration with popular benchmark frameworks.

```rust
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use par_bench::{Run, ThreadPool};

// Create a thread pool.
let pool = ThreadPool::default();

// Shared atomic counter that all threads will increment.
let counter = Arc::new(AtomicU64::new(0));

let run = Run::builder()
    .prepare_thread_fn({
        let counter = Arc::clone(&counter);
        move |_run_meta| Arc::clone(&counter)
    })
    .prepare_iter_fn(|_run_meta, counter| Arc::clone(counter))
    .iter_fn(|counter: Arc<AtomicU64>| {
        // Increment the atomic counter and use black_box to prevent optimization.
        black_box(counter.fetch_add(1, Ordering::Relaxed));
    })
    .build();

// Execute 10,000 iterations across all threads.
let stats = run.execute_on(&pool, 10_000);

// Get the mean duration for benchmark reporting.
let duration = stats.mean_duration();
```

More details in the [package documentation](https://docs.rs/par_bench/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
