//! Callgrind benchmarks for the storage codec.
//!
//! Paired with `cbh_core_codec.rs` (the Criterion benches for the same two
//! operations) which measures the codec under wall-clock timing. These benches
//! isolate the per-call instruction count of `compress` and `decompress` so a
//! regression — or the future 0-allocation reusable-compressor tuning — can be
//! tracked at instruction-level granularity.
//!
//! # Scope and caveats
//!
//! Unlike the `fast_time` clock benches, the codec performs no syscalls and
//! spawns no threads, and gzip is deterministic, so these instruction counts are
//! stable run-to-run — there is no flaky-benchmark caveat. Allocator traffic
//! (the high-level `flate2` API allocates per call) shows up directly in the
//! counts, which is exactly the quantity the allocation-reuse tuning would move.

#![allow(
    missing_docs,
    reason = "no need for API documentation on benchmark code"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Tracking issue drafts live at \
          c:/Source/gungraun-lint-issues/ pending upstream filing."
    )
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::needless_pass_by_value,
        reason = "Gungraun's #[bench] setup contract hands the owned setup value to the \
          benchmark function by value; the measured body only borrows it, but the signature \
          must own it so the input's construction stays outside the measured region."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Valgrind is Linux-only. On other platforms this bench target compiles
    // to a no-op so `cargo build --all-targets` still works.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;

    use cargo_bench_history_core::codec;
    use cargo_bench_history_core::model::{
        BenchmarkId, BenchmarkResult, EnvironmentInfo, GitInfo, Metric, MetricKind, Run,
        RunContext, ToolchainInfo,
    };
    use gungraun::prelude::*;
    use nonempty::nonempty;

    /// Result count of the `small` payload (a serialized `Run` of roughly 9 KB).
    const SMALL_RESULTS: usize = 20;

    /// Result count of the `large` payload (a serialized `Run` of roughly 89 KB).
    const LARGE_RESULTS: usize = 200;

    /// Builds a realistic serialized `Run` carrying `result_count` results,
    /// mirroring the helper in the paired Criterion bench so both views exercise
    /// the same payload shape.
    fn sample_run_json(result_count: usize) -> Vec<u8> {
        let epoch = "2024-01-01T00:00:00Z".parse().unwrap();
        let context = RunContext::new(
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

    /// Compresses a freshly built payload, for the decompress setup.
    fn compressed_run_json(result_count: usize) -> Vec<u8> {
        codec::compress(&sample_run_json(result_count))
    }

    #[library_benchmark]
    #[bench::run(sample_run_json(SMALL_RESULTS))]
    fn compress_small(payload: Vec<u8>) -> Vec<u8> {
        black_box(codec::compress(black_box(&payload)))
    }

    #[library_benchmark]
    #[bench::run(sample_run_json(LARGE_RESULTS))]
    fn compress_large(payload: Vec<u8>) -> Vec<u8> {
        black_box(codec::compress(black_box(&payload)))
    }

    #[library_benchmark]
    #[bench::run(compressed_run_json(SMALL_RESULTS))]
    fn decompress_small(compressed: Vec<u8>) -> Vec<u8> {
        black_box(codec::decompress(black_box(&compressed)).expect("payload round-trips"))
    }

    #[library_benchmark]
    #[bench::run(compressed_run_json(LARGE_RESULTS))]
    fn decompress_large(compressed: Vec<u8>) -> Vec<u8> {
        black_box(codec::decompress(black_box(&compressed)).expect("payload round-trips"))
    }

    library_benchmark_group!(
        name = compress,
        benchmarks = [compress_small, compress_large]
    );

    library_benchmark_group!(
        name = decompress,
        benchmarks = [decompress_small, decompress_large]
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{compress, decompress};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = compress, decompress
);
