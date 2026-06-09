//! Callgrind benchmarks for the nm-to-OpenTelemetry export hot path.
//!
//! Paired with `nm_otel_export.rs` which covers the same operations under wall-clock
//! measurement. Callgrind is the primary signal for the per-bucket export cost because
//! the per-bucket delta from this optimization is on the order of tens of instructions
//! per bucket and would be invisible under Criterion noise on a shared machine.
//!
//! All scenarios drive [`Publisher::run_one_iteration_with_report`] on a pre-built
//! [`Report`] (assembled via [`Report::fake`] + [`Histogram::fake`] +
//! [`EventMetrics::fake`]) so the measurement is decoupled from the global nm
//! registry and reproducible run-to-run.
//!
//! Scenarios:
//!
//! * `small_steady_state` — one histogram event with 7 explicit buckets + the synthetic
//!   `+Inf` bucket (8 buckets total). The measured call sees strictly higher cumulative
//!   counts than the warmup, so every bucket has a positive delta and exercises the
//!   `add_bucket_delta` hot path.
//! * `small_no_delta` — same 8-bucket histogram, but the measured call sees the same
//!   cumulative counts as the warmup. All bucket deltas are zero and the `if delta > 0`
//!   branch in `add_bucket_delta` short-circuits. Guards against regressions on the
//!   empty path.
//! * `large_steady_state` — same shape as `small_steady_state` but with 31 explicit
//!   buckets + `+Inf` (32 buckets total). Lets the per-bucket cost be read off by
//!   comparing against the 8-bucket scenario.

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

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only. On other platforms this
    // bench target compiles to a no-op so `cargo build --all-targets` still works.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;

    use gungraun::prelude::*;
    use nm::{EventMetrics, Histogram, Magnitude, Report};
    use nm_otel::Publisher;
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
    use tick::Clock;

    const EVENT_NAME: &str = "cg_export_histogram";

    // 7 explicit buckets + the synthetic `Magnitude::MAX` (`+Inf`) bucket = 8 buckets total.
    // Spacing is illustrative; the bucket bound values do not affect the export hot path.
    const SMALL_BUCKETS: &[Magnitude] = &[1, 10, 50, 100, 500, 1_000, 5_000];

    // 31 explicit buckets + the synthetic `Magnitude::MAX` bucket = 32 buckets total. Mirrors
    // the bucket count used by `nm_observe_cg::LARGE_HISTOGRAM_BUCKETS` for consistency.
    const LARGE_BUCKETS: &[Magnitude] = &[
        1,
        2,
        4,
        8,
        16,
        32,
        64,
        128,
        256,
        512,
        1_024,
        2_048,
        4_096,
        8_192,
        16_384,
        32_768,
        65_536,
        131_072,
        262_144,
        524_288,
        1_048_576,
        2_097_152,
        4_194_304,
        8_388_608,
        16_777_216,
        33_554_432,
        67_108_864,
        134_217_728,
        268_435_456,
        536_870_912,
        1_073_741_824,
    ];

    // Per-bucket non-cumulative counts injected on the warmup pass. Using ones makes the
    // resulting cumulative sequence the bucket index, which keeps the arithmetic legible.
    const WARMUP_PER_BUCKET_COUNT: u64 = 1;
    const WARMUP_PLUS_INF_COUNT: u64 = 1;

    // For the steady-state scenarios the measured pass uses strictly larger per-bucket
    // counts than the warmup, so every bucket sees a positive delta and the
    // `add_bucket_delta` hot path is exercised on every bucket.
    const STEADY_STATE_PER_BUCKET_COUNT: u64 = 2;
    const STEADY_STATE_PLUS_INF_COUNT: u64 = 2;

    /// Inputs for one measured export call.
    ///
    /// `Publisher` and `Report` are paired so the bench function can run a single export
    /// against a steady-state-initialized publisher without any allocator activity beyond
    /// what the export itself performs.
    type ExportInputs = (Publisher, Report);

    fn build_publisher() -> Publisher {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter).build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();

        Publisher::builder()
            .provider(provider)
            .clock(Clock::new_frozen())
            .build()
    }

    fn make_report(buckets: &'static [Magnitude], per_bucket: u64, plus_inf: u64) -> Report {
        let histogram = Histogram::fake(buckets, vec![per_bucket; buckets.len()], plus_inf);
        // `count` and `sum` are not on the bucket hot path; pick values that match the
        // bucket sequence so deltas behave intuitively.
        let total_buckets = buckets.len().saturating_add(1);
        let total_buckets_u64 = u64::try_from(total_buckets).unwrap_or(u64::MAX);
        let count = per_bucket.saturating_mul(total_buckets_u64);
        let sum: Magnitude = 0;
        let event = EventMetrics::fake(EVENT_NAME, count, sum, Some(histogram));
        Report::fake(vec![event])
    }

    /// Builds a publisher with one warmup export already applied.
    ///
    /// After this returns, the `Publisher` has registered the histogram event, allocated
    /// its bucket-state vector, and constructed the precomputed `[KeyValue; 1]`
    /// attribute slices. The next `run_one_iteration_with_report` call exercises only
    /// the steady-state export path.
    fn warm_up(buckets: &'static [Magnitude]) -> Publisher {
        let mut publisher = build_publisher();
        let warmup_report = make_report(buckets, WARMUP_PER_BUCKET_COUNT, WARMUP_PLUS_INF_COUNT);
        publisher.run_one_iteration_with_report(&warmup_report);
        publisher
    }

    fn setup_small_steady_state() -> ExportInputs {
        let publisher = warm_up(SMALL_BUCKETS);
        let report = make_report(
            SMALL_BUCKETS,
            STEADY_STATE_PER_BUCKET_COUNT,
            STEADY_STATE_PLUS_INF_COUNT,
        );
        (publisher, report)
    }

    fn setup_small_no_delta() -> ExportInputs {
        let publisher = warm_up(SMALL_BUCKETS);
        // Same per-bucket counts as the warmup ⇒ identical cumulative sequence ⇒ zero
        // deltas. Exercises the `if delta > 0` short-circuit in `add_bucket_delta`.
        let report = make_report(
            SMALL_BUCKETS,
            WARMUP_PER_BUCKET_COUNT,
            WARMUP_PLUS_INF_COUNT,
        );
        (publisher, report)
    }

    fn setup_large_steady_state() -> ExportInputs {
        let publisher = warm_up(LARGE_BUCKETS);
        let report = make_report(
            LARGE_BUCKETS,
            STEADY_STATE_PER_BUCKET_COUNT,
            STEADY_STATE_PLUS_INF_COUNT,
        );
        (publisher, report)
    }

    #[library_benchmark]
    #[bench::small_steady_state(setup_small_steady_state())]
    #[bench::small_no_delta(setup_small_no_delta())]
    #[bench::large_steady_state(setup_large_steady_state())]
    fn export_run_one_iteration(inputs: ExportInputs) -> ExportInputs {
        let (mut publisher, report) = inputs;
        publisher.run_one_iteration_with_report(black_box(&report));
        (publisher, report)
    }

    library_benchmark_group!(name = export, benchmarks = [export_run_one_iteration]);
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::export;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = export
);
