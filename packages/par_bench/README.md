# par_bench

Mechanisms for multithreaded benchmarking, designed for integration with Criterion or a similar benchmark framework.

## Features

- **Low-overhead design**: Minimizes framework overhead when benchmarking operations
- **Thread pool reuse**: Pre-warmed threads reduce benchmark harness overhead
- **Flexible configuration**: Support for complex scenarios with different workloads on different threads
- **Group-based execution**: Divide threads into groups for sophisticated benchmark scenarios
- **Integration ready**: Designed to work seamlessly with Criterion and other benchmark frameworks

## Basic usage

Here's a simple example that demonstrates measuring atomic operations across different thread counts:

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
        move |_group_info| Arc::clone(&counter)
    })
    .prepare_iter_fn(|_group_info, counter| Arc::clone(counter))
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

## Integration with Criterion

par_bench is designed to work seamlessly with Criterion:

```rust
use criterion::{Criterion, criterion_group, criterion_main};
use par_bench::{Run, ThreadPool};

fn my_benchmark(c: &mut Criterion) {
    let thread_pool = ThreadPool::default();

    c.bench_function("my_operation", |b| {
        b.iter_custom(|iters| {
            let run = Run::builder()
                .iter_fn(|()| {
                    // Your operation here.
                    std::hint::black_box(());
                })
                .build();

            let stats = run.execute_on(&thread_pool, iters);
            stats.mean_duration()
        });
    });
}

criterion_group!(benches, my_benchmark);
criterion_main!(benches);
```

## Thread pools

- `ThreadPool::default()` - Creates a pool with one thread per available processor
- `ThreadPool::new(&processor_set)` - Creates a pool with threads for specific processors

## Builder pattern

The `Run::builder()` provides a fluent API for configuring benchmark runs:

- `.groups(n)` - Divide threads into equal groups
- `.prepare_thread_fn(f)` - Set up per-thread state
- `.prepare_iter_fn(f)` - Set up per-iteration state
- `.measure_wrapper_fns(begin, end)` - Wrap measured execution
- `.iter_fn(f)` - Define the operation to benchmark
- `.build()` - Complete the run configuration

## Examples

See the `examples/` directory for complete usage examples:

- `basic_usage.rs` - Demonstrates single vs multi-threaded atomic operations
