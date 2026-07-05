Processor time tracking utilities for benchmarks and performance analysis.

This package provides utilities to track processor time during code execution,
enabling analysis of processor usage patterns in benchmarks and performance tests.

The recommended pattern drives measurement from Criterion's `iter_custom`, feeding
its chosen iteration count into `iterations` so each recorded span covers a whole
sample rather than a single iteration:

```rust
use std::hint::black_box;
use std::time::Instant;

use all_the_time::Session;
use criterion::Criterion;

fn main() {
    let session = Session::new();
    let operation = session.operation("my_operation");

    let mut criterion = Criterion::default();
    criterion.bench_function("my_operation", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            let _span = operation.measure_thread().iterations(iters);
            for _ in 0..iters {
                black_box(42_u64.wrapping_mul(2));
            }
            start.elapsed()
        });
    });

    // When `session` is dropped it prints a human-readable summary to stdout and
    // writes machine-readable JSON files (one per operation) into the Cargo
    // target directory: target/all_the_time/<operation>.json.
}
```

When the iteration count is only known after the measured work has run, capture
the measurement first and finalize it with `complete(n)` once the count is known.

## See also

More details in the [package documentation](https://docs.rs/all_the_time/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
