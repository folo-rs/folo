//! Benchmarks for the cheap-but-hot analysis operations that `analyze` performs
//! once per stored object.
//!
//! Parsing a storage key back into its components is invoked per stored object
//! when scanning history, so a regression here scales with history size.
//! Benchmarking it now (while the implementation is simple) gives a baseline to
//! measure any future optimization against.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use cbh_analysis::parse_key;
use criterion::{Criterion, criterion_group, criterion_main};

criterion_group!(benches, storage_key);
criterion_main!(benches);

fn storage_key(c: &mut Criterion) {
    let mut group = c.benchmark_group("cbh_analysis/storage_key");

    let key = "v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/\
               deadbeefdeadbeefdeadbeefdeadbeefdeadbeef/clean.json";
    group.bench_function("parse_key", |b| {
        b.iter(|| black_box(parse_key(black_box(key))));
    });

    group.finish();
}
