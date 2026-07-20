//! Benchmarks for the cheap-but-hot data-model operations that `run` and
//! `analyze` perform once per stored object.
//!
//! These are optimization candidates rather than known hot spots: building a
//! qualified benchmark identity, sanitizing a path segment, assembling a storage
//! key, and parsing one back into its components are each invoked per benchmark
//! result or per stored object, so a regression here scales with history size.
//! Benchmarking them now (while the implementations are simple) gives a baseline
//! to measure any future optimization against.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use cbh_model::{BenchmarkId, DiscriminantSet, Engine, parse_key, sanitize_segment};
use criterion::{Criterion, criterion_group, criterion_main};
use nonempty::nonempty;

criterion_group!(benches, identity, storage_key);
criterion_main!(benches);

fn identity(c: &mut Criterion) {
    let mut group = c.benchmark_group("cbh_model/identity");

    let id = BenchmarkId::new(nonempty![
        "fast_time".to_owned(),
        "a::group".to_owned(),
        "capture".to_owned(),
        "two_instants".to_owned(),
    ]);
    group.bench_function("qualified", |b| {
        b.iter(|| black_box(black_box(&id).qualified()));
    });

    group.finish();
}

fn storage_key(c: &mut Criterion) {
    let mut group = c.benchmark_group("cbh_model/storage_key");

    group.bench_function("sanitize_segment", |b| {
        b.iter(|| black_box(sanitize_segment(black_box("x86_64-unknown-linux-gnu"))));
    });

    let set = DiscriminantSet::new(Engine::Callgrind, "x86_64-unknown-linux-gnu", "m1");
    group.bench_function("clean_key", |b| {
        b.iter(|| {
            black_box(black_box(&set).clean_key(
                black_box("folo"),
                black_box("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            ))
        });
    });

    let key = "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/\
               deadbeefdeadbeefdeadbeefdeadbeefdeadbeef/clean.json";
    group.bench_function("parse_key", |b| {
        b.iter(|| black_box(parse_key(black_box(key))));
    });

    group.finish();
}
