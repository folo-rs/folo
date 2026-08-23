//! Callgrind benchmarks for the nm-to-OpenTelemetry export hot path.
//!
//! Paired with `nm_otel_export.rs`, which covers the same export scenarios under wall-clock
//! measurement. Each scenario supplies a synthetic report directly, so collection from the
//! global `nm` registry is outside the measured operation.
//!
//! Low bucket cardinality represents a compact histogram used on ordinary event paths. High
//! bucket cardinality exposes per-bucket scaling. Publishers are warm before measurement so
//! instrument creation and retained-state allocation do not obscure the recurring export cost.

#![allow(missing_docs, reason = "Benchmark code does not expose a public API.")]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "These lints originate in Gungraun macro expansion and cannot be addressed in \
                  this benchmark."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is available only on Linux.
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig, main};
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes", "--collect-bus=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    ),
    library_benchmark_groups = [export, delta]
);

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;
    use std::iter;

    use gungraun::prelude::*;
    use nm::{EventMetrics, Histogram, Magnitude, Report};
    use nm_otel::Publisher;
    use nm_otel_impl::{EventState, create_test_provider};
    use tick::Clock;

    // A stable synthetic name avoids introducing registry-dependent setup.
    const EVENT_NAME: &str = "cg_export_histogram";
    // Low cardinality represents the compact histogram configuration.
    const LOW_CARDINALITY_BUCKET_BOUNDS: &[Magnitude] = &[1, 10, 50, 100, 500, 1_000, 5_000];
    // High cardinality makes growth in per-bucket work visible.
    const HIGH_CARDINALITY_BUCKET_BOUNDS: &[Magnitude] = &[
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
    // Warm values establish instruments and retained bucket state before measurement.
    const WARM_PER_BUCKET_COUNT: u64 = 1;
    const WARM_PLUS_INFINITY_BUCKET_COUNT: u64 = 1;
    // Larger measured values force the positive-delta branch for every bucket.
    const POSITIVE_DELTA_PER_BUCKET_COUNT: u64 = 2;
    const POSITIVE_DELTA_PLUS_INFINITY_BUCKET_COUNT: u64 = 2;

    /// Inputs for one warm-state export.
    type ExportInputs = (Publisher, Report);

    fn build_publisher() -> Publisher {
        // Attaching the manual reader activates instruments without collection in the measured
        // region.
        let (provider, _) = create_test_provider();

        Publisher::builder()
            .provider(provider)
            .clock(Clock::new_frozen())
            .build()
    }

    fn make_report(
        bucket_bounds: &'static [Magnitude],
        per_bucket_count: u64,
        plus_infinity_bucket_count: u64,
    ) -> Report {
        let histogram = Histogram::fake(
            bucket_bounds,
            vec![per_bucket_count; bucket_bounds.len()],
            plus_infinity_bucket_count,
        );
        let total_buckets = bucket_bounds.len().saturating_add(1);
        let total_buckets = u64::try_from(total_buckets).unwrap_or(u64::MAX);
        let event_count = per_bucket_count.saturating_mul(total_buckets);
        let event_sum = Magnitude::default();
        let event = EventMetrics::fake(EVENT_NAME, event_count, event_sum, Some(histogram));
        Report::fake(vec![event])
    }

    fn warm_publisher(bucket_bounds: &'static [Magnitude]) -> Publisher {
        let mut publisher = build_publisher();
        let warm_report = make_report(
            bucket_bounds,
            WARM_PER_BUCKET_COUNT,
            WARM_PLUS_INFINITY_BUCKET_COUNT,
        );
        publisher.run_one_iteration_with_report(&warm_report);
        publisher
    }

    fn setup_export(
        bucket_bounds: &'static [Magnitude],
        per_bucket_count: u64,
        plus_infinity_bucket_count: u64,
    ) -> ExportInputs {
        let publisher = warm_publisher(bucket_bounds);
        let report = make_report(bucket_bounds, per_bucket_count, plus_infinity_bucket_count);
        (publisher, report)
    }

    fn setup_low_bucket_cardinality_positive_delta() -> ExportInputs {
        setup_export(
            LOW_CARDINALITY_BUCKET_BOUNDS,
            POSITIVE_DELTA_PER_BUCKET_COUNT,
            POSITIVE_DELTA_PLUS_INFINITY_BUCKET_COUNT,
        )
    }

    fn setup_low_bucket_cardinality_zero_delta() -> ExportInputs {
        setup_export(
            LOW_CARDINALITY_BUCKET_BOUNDS,
            WARM_PER_BUCKET_COUNT,
            WARM_PLUS_INFINITY_BUCKET_COUNT,
        )
    }

    fn setup_high_bucket_cardinality_positive_delta() -> ExportInputs {
        setup_export(
            HIGH_CARDINALITY_BUCKET_BOUNDS,
            POSITIVE_DELTA_PER_BUCKET_COUNT,
            POSITIVE_DELTA_PLUS_INFINITY_BUCKET_COUNT,
        )
    }

    fn run_export(inputs: ExportInputs) -> ExportInputs {
        let (mut publisher, report) = inputs;
        publisher.run_one_iteration_with_report(black_box(&report));
        (publisher, report)
    }

    #[library_benchmark]
    #[bench::default(setup_low_bucket_cardinality_positive_delta())]
    fn export_low_bucket_cardinality_positive_delta(inputs: ExportInputs) -> ExportInputs {
        run_export(inputs)
    }

    #[library_benchmark]
    #[bench::default(setup_low_bucket_cardinality_zero_delta())]
    fn export_low_bucket_cardinality_zero_delta(inputs: ExportInputs) -> ExportInputs {
        run_export(inputs)
    }

    #[library_benchmark]
    #[bench::default(setup_high_bucket_cardinality_positive_delta())]
    fn export_high_bucket_cardinality_positive_delta(inputs: ExportInputs) -> ExportInputs {
        run_export(inputs)
    }

    #[library_benchmark]
    #[bench::default(setup_delta(LOW_CARDINALITY_BUCKET_BOUNDS))]
    fn delta_low_bucket_cardinality_positive_delta(inputs: DeltaInputs) -> DeltaInputs {
        run_delta(inputs)
    }

    #[library_benchmark]
    #[bench::default(setup_delta(HIGH_CARDINALITY_BUCKET_BOUNDS))]
    fn delta_high_bucket_cardinality_positive_delta(inputs: DeltaInputs) -> DeltaInputs {
        run_delta(inputs)
    }

    library_benchmark_group!(
        name = export,
        benchmarks = [
            export_low_bucket_cardinality_positive_delta,
            export_low_bucket_cardinality_zero_delta,
            export_high_bucket_cardinality_positive_delta
        ]
    );

    library_benchmark_group!(
        name = delta,
        benchmarks = [
            delta_low_bucket_cardinality_positive_delta,
            delta_high_bucket_cardinality_positive_delta
        ]
    );

    /// Carries warm delta state and the next collection's non-cumulative bucket counts.
    #[derive(Debug)]
    struct DeltaInputs {
        state: EventState,
        bucket_bounds: &'static [Magnitude],
        counts: Vec<u64>,
    }

    fn setup_delta(bucket_bounds: &'static [Magnitude]) -> DeltaInputs {
        let mut state = EventState::default();
        let bucket_count = bucket_bounds
            .len()
            .checked_add(1)
            .expect("the benchmark bucket count fits in usize");
        let initial_counts = vec![WARM_PER_BUCKET_COUNT; bucket_count];
        _ = state
            .histogram_deltas(
                bucket_bounds
                    .iter()
                    .copied()
                    .chain(iter::once(Magnitude::MAX)),
                initial_counts,
            )
            .count();

        DeltaInputs {
            state,
            bucket_bounds,
            counts: vec![POSITIVE_DELTA_PER_BUCKET_COUNT; bucket_count],
        }
    }

    fn run_delta(mut inputs: DeltaInputs) -> DeltaInputs {
        for delta in inputs.state.histogram_deltas(
            inputs
                .bucket_bounds
                .iter()
                .copied()
                .chain(iter::once(Magnitude::MAX)),
            black_box(&inputs.counts).iter().copied(),
        ) {
            black_box(delta);
        }
        inputs
    }
}
