//! Benchmarks for the storage codec: the gzip transform the storage layer
//! applies to every stored object.
//!
//! The codec runs once per stored object on both the write path
//! (`run`/backfill/stress-seed) and the read path (`analyze`), and it is pure,
//! single-threaded, syscall-free CPU work, so its cost scales directly with
//! history size. Benchmarking it now — while the implementation is the simple
//! high-level `flate2` API — establishes a baseline to measure the documented
//! future 0-allocation reusable-compressor tuning against.
//!
//! Memory allocations are tracked via `alloc_tracker` alongside the wall-clock
//! timings and printed to stdout (and written to the Cargo target directory)
//! after all benchmarks complete, so the current high-level API's per-call
//! allocation churn — the very thing the future tuning targets — is quantified
//! next to its latency.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::time::Instant;

use alloc_tracker::{Allocator, Session};
use cargo_bench_history_core::codec;
use cargo_bench_history_core::model::{
    BenchmarkId, BenchmarkResult, EnvironmentInfo, GitInfo, Metric, MetricKind, Run, RunContext,
    ToolchainInfo,
};
use criterion::{Criterion, criterion_group, criterion_main};
use nonempty::nonempty;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

/// Result count of the `small` payload (a serialized `Run` of roughly 9 KB),
/// representing a modest benchmark suite.
const SMALL_RESULTS: usize = 20;

/// Result count of the `large` payload (a serialized `Run` of roughly 89 KB),
/// representing a large suite — so throughput scaling with object size is
/// visible alongside the small case.
const LARGE_RESULTS: usize = 200;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let allocs = Session::new();

    compress(c, &allocs);
    decompress(c, &allocs);

    // `allocs` prints its summary and writes JSON to the Cargo target directory
    // when it is dropped at the end of this function.
}

/// Builds a realistic serialized `Run` carrying `result_count` results.
///
/// The shape mirrors what the storage layer actually compresses: recurring field
/// names, a stable id structure, and a handful of metrics per result — the
/// repetition that makes the JSON compressible.
fn sample_run_json(result_count: usize) -> Vec<u8> {
    let epoch = "2024-01-01T00:00:00Z".parse().unwrap();
    let context = RunContext::new(
        epoch,
        epoch,
        GitInfo::default(),
        EnvironmentInfo::default(),
        ToolchainInfo::default(),
        "0.0.1".to_owned(),
    );
    let results = (0..result_count)
        .map(|i| {
            let id = BenchmarkId::new(nonempty![
                "cargo_bench_history".to_owned(),
                "module::group".to_owned(),
                format!("case_{i}"),
                "two_instants".to_owned(),
            ]);
            let metrics = vec![
                Metric::new(MetricKind::InstructionCount, 1_234.0),
                Metric::new(MetricKind::L1CacheHits, 9_876.0),
                Metric::new(MetricKind::EstimatedCycles, 4_321.0),
                Metric::new(MetricKind::WallTime, 26.9).with_dispersion(
                    Some(0.47),
                    Some(26.6),
                    Some(27.2),
                ),
            ];
            BenchmarkResult::new(id, metrics)
        })
        .collect();
    Run::new(context, results)
        .to_json()
        .expect("a constructed Run always serializes")
        .into_bytes()
}

fn compress(c: &mut Criterion, allocs: &Session) {
    let mut group = c.benchmark_group("cbh_core_codec/compress");
    for (name, result_count) in [("small", SMALL_RESULTS), ("large", LARGE_RESULTS)] {
        let payload = sample_run_json(result_count);
        let op = allocs.operation(format!("cbh_core_codec/compress/{name}"));
        group.bench_function(name, |b| {
            b.iter_custom(|iters| {
                let _span = op.measure_thread().iterations(iters);
                let start = Instant::now();
                for _ in 0..iters {
                    black_box(codec::compress(black_box(&payload)));
                }
                start.elapsed()
            });
        });
    }
    group.finish();
}

fn decompress(c: &mut Criterion, allocs: &Session) {
    let mut group = c.benchmark_group("cbh_core_codec/decompress");
    for (name, result_count) in [("small", SMALL_RESULTS), ("large", LARGE_RESULTS)] {
        let compressed = codec::compress(&sample_run_json(result_count));
        let op = allocs.operation(format!("cbh_core_codec/decompress/{name}"));
        group.bench_function(name, |b| {
            b.iter_custom(|iters| {
                let _span = op.measure_thread().iterations(iters);
                let start = Instant::now();
                for _ in 0..iters {
                    black_box(
                        codec::decompress(black_box(&compressed)).expect("payload round-trips"),
                    );
                }
                start.elapsed()
            });
        });
    }
    group.finish();
}
