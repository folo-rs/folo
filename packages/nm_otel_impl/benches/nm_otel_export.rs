//! Benchmarks for the nm-to-OpenTelemetry export path.
//!
//! The bucket-cardinality scenarios measure one warm publisher export and pair with
//! `nm_otel_export_cg.rs`. The multi-event scenario separately tracks allocations while
//! routing several events through the export pipeline. None of these scenarios include
//! collection from the global `nm` registry.

#![allow(missing_docs, reason = "Benchmark code does not expose a public API.")]

use std::cell::RefCell;
use std::hint::black_box;
use std::iter;

use alloc_tracker::{Allocator, Session as AllocSession};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use many_cpus::SystemHardware;
use new_zealand::nz;
use nm::{EventMetrics, Histogram, Magnitude, Report};
use nm_otel::Publisher;
use nm_otel_impl::{EventState, create_test_provider};
use par_bench::{ResourceUsageExt, Run, ThreadPool};
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

// This workload represents routing several events rather than scaling one histogram.
const MULTI_EVENT_COUNT: usize = 8;
// Several uneven bounds keep the multi-event workload representative without dominating it.
const MULTI_EVENT_BUCKET_BOUNDS: &[Magnitude] = &[10, 50, 100, 500];
// Uneven observations exercise cumulative conversion without favoring one bucket.
const MULTI_EVENT_NON_CUMULATIVE_COUNTS: [u64; 4] = [5, 12, 8, 3];
// A populated overflow bucket keeps every histogram export branch reachable.
const MULTI_EVENT_PLUS_INFINITY_BUCKET_COUNT: u64 = 2;
// The event total matches the observations represented by all histogram buckets.
const MULTI_EVENT_COUNT_TOTAL: u64 = 30;
// A nonzero sum keeps scalar export work representative of populated events.
const MULTI_EVENT_SUM: Magnitude = 4567;

// Low cardinality represents the common compact histogram configuration.
const LOW_CARDINALITY_BUCKET_BOUNDS: &[Magnitude] = &[1, 10, 50, 100, 500, 1_000, 5_000];
// High cardinality makes the per-bucket scaling cost visible.
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
// A stable synthetic name avoids introducing registry-dependent setup.
const HISTOGRAM_EVENT_NAME: &str = "criterion_export_histogram";

/// Carries a warm publisher together with the report for one measured export.
type ExportInputs = (Publisher, Report);

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    benchmark_bucket_cardinality(c);
    benchmark_delta_computation(c);
    benchmark_multi_event_allocations(c);
}

fn benchmark_bucket_cardinality(c: &mut Criterion) {
    let mut group = c.benchmark_group("nm_otel_export/export");

    group.bench_function("low_bucket_cardinality_positive_delta", |b| {
        b.iter_batched_ref(
            setup_low_bucket_cardinality_positive_delta,
            run_export,
            BatchSize::SmallInput,
        );
    });
    group.bench_function("low_bucket_cardinality_zero_delta", |b| {
        b.iter_batched_ref(
            setup_low_bucket_cardinality_zero_delta,
            run_export,
            BatchSize::SmallInput,
        );
    });
    group.bench_function("high_bucket_cardinality_positive_delta", |b| {
        b.iter_batched_ref(
            setup_high_bucket_cardinality_positive_delta,
            run_export,
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn benchmark_delta_computation(c: &mut Criterion) {
    let mut group = c.benchmark_group("nm_otel_export/delta");

    group.bench_function("low_bucket_cardinality_positive_delta", |b| {
        b.iter_batched_ref(
            || setup_delta(LOW_CARDINALITY_BUCKET_BOUNDS),
            run_delta,
            BatchSize::SmallInput,
        );
    });
    group.bench_function("high_bucket_cardinality_positive_delta", |b| {
        b.iter_batched_ref(
            || setup_delta(HIGH_CARDINALITY_BUCKET_BOUNDS),
            run_delta,
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn benchmark_multi_event_allocations(c: &mut Criterion) {
    let allocs = AllocSession::new();
    let mut one_thread = ThreadPool::new(
        SystemHardware::current()
            .processors()
            .to_builder()
            .take(nz!(1))
            .unwrap(),
    );
    let mut group = c.benchmark_group("nm_otel_export/export");

    Run::new()
        .prepare_thread(|_| {
            let report = make_multi_event_report(MULTI_EVENT_COUNT);
            let mut publisher = build_publisher();

            // Initialize instruments and retained state outside the measured region.
            publisher.run_one_iteration_with_report(&report);

            (RefCell::new(publisher), report)
        })
        .measure_resource_usage("nm_otel_export/export/steady_state_8_events", |measure| {
            measure.allocs(&allocs)
        })
        .iter(|args| {
            let (publisher, report) = args.thread_state();
            publisher.borrow_mut().run_one_iteration_with_report(report);
        })
        .execute_criterion_on(&mut one_thread, &mut group, "steady_state_8_events");

    group.finish();

    // Dropping the session emits the allocation report after every measured span closes.
}

fn build_publisher() -> Publisher {
    // Attaching the manual reader activates instruments without autonomous collection activity.
    let (provider, _) = create_test_provider();

    Publisher::builder()
        .provider(provider)
        .clock(Clock::new_frozen())
        .build()
}

fn make_multi_event_report(event_count: usize) -> Report {
    let events = (0..event_count)
        .map(|event_index| {
            let name = format!("bench_event_{event_index}");
            let histogram = Histogram::fake(
                MULTI_EVENT_BUCKET_BOUNDS,
                MULTI_EVENT_NON_CUMULATIVE_COUNTS.to_vec(),
                MULTI_EVENT_PLUS_INFINITY_BUCKET_COUNT,
            );
            EventMetrics::fake(
                name,
                MULTI_EVENT_COUNT_TOTAL,
                MULTI_EVENT_SUM,
                Some(histogram),
            )
        })
        .collect();
    Report::fake(events)
}

fn make_histogram_report(
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
    let event = EventMetrics::fake(
        HISTOGRAM_EVENT_NAME,
        event_count,
        event_sum,
        Some(histogram),
    );
    Report::fake(vec![event])
}

fn warm_publisher(bucket_bounds: &'static [Magnitude]) -> Publisher {
    let mut publisher = build_publisher();
    let warm_report = make_histogram_report(
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
    let report = make_histogram_report(bucket_bounds, per_bucket_count, plus_infinity_bucket_count);
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

fn run_export(inputs: &mut ExportInputs) {
    let (publisher, report) = inputs;
    publisher.run_one_iteration_with_report(black_box(report));
}

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

fn run_delta(inputs: &mut DeltaInputs) {
    let DeltaInputs {
        state,
        bucket_bounds,
        counts,
    } = inputs;

    for delta in state.histogram_deltas(
        bucket_bounds
            .iter()
            .copied()
            .chain(iter::once(Magnitude::MAX)),
        black_box(counts).iter().copied(),
    ) {
        black_box(delta);
    }
}
