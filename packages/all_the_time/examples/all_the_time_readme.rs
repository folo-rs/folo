//! Example that demonstrates the `iter_custom` pattern shown in the README.md file.
//!
//! This shows the recommended `iter_custom` pattern for tracking processor time,
//! feeding the Criterion-chosen iteration count into each recorded span.
//!
//! Criterion is configured with small measurement and statistical-analysis
//! budgets so the example runs quickly as a smoke test; a real benchmark would
//! use `Criterion::default()`.

use std::hint::black_box;
use std::time::{Duration, Instant};

use all_the_time::Session;
use criterion::Criterion;

fn main() {
    let session = Session::new();
    let operation = session.operation("my_operation");

    let mut criterion = Criterion::default()
        .warm_up_time(Duration::from_millis(10))
        .measurement_time(Duration::from_millis(20))
        .sample_size(10)
        .nresamples(2000)
        .without_plots();
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
    // target directory: target/all_the_time/<operation>.json
}
