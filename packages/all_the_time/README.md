Processor time tracking utilities for benchmarks and performance analysis.

This package provides utilities to track processor time during code execution,
enabling analysis of processor usage patterns in benchmarks and performance tests.

The typical pattern is to drive measurement from Criterion's `iter_custom` function,
feeding its chosen iteration count into `iterations` so each recorded span covers a
whole sample rather than a single iteration:

```rust
use std::hint::black_box;
use std::time::Instant;

use all_the_time::Session;
use criterion::Criterion;

fn bench(c: &mut Criterion) {
    let session = Session::new();

    let operation = session.operation("my_operation");
    c.bench_function("my_operation", |b| {
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

You do not need to specify the iteration count up front, as long as it is provided
before the span is dropped. This allows you to measure work whose extent is not
known at the start.

## See also

More details in the [package documentation](https://docs.rs/all_the_time/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
