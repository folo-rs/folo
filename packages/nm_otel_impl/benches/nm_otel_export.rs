//! Benchmarks for the nm-to-OpenTelemetry export path.
//!
//! Drives [`Publisher::run_one_iteration_with_report`] with a synthetic [`Report`] built
//! from [`Report::fake`] so the steady-state export cost can be measured
//! without depending on the global `nm` registry. Memory allocations are tracked via
//! `alloc_tracker` so changes in the export pipeline's allocation profile are visible
//! at a glance.

#![allow(missing_docs, reason = "benchmark code")]

use std::cell::RefCell;

use alloc_tracker::{Allocator, Session as AllocSession};
use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::SystemHardware;
use new_zealand::nz;
use nm::{EventMetrics, Histogram, Magnitude, Report};
use nm_otel::Publisher;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use par_bench::{ResourceUsageExt, Run, ThreadPool};
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

const EVENT_COUNT: usize = 8;
const HISTOGRAM_BUCKETS: &[Magnitude] = &[10, 50, 100, 500];
const HISTOGRAM_NON_CUMULATIVE: [u64; 4] = [5, 12, 8, 3];
const HISTOGRAM_PLUS_INFINITY: u64 = 2;
const HISTOGRAM_COUNT: u64 = 30;
const HISTOGRAM_SUM: Magnitude = 4567;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
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
            let report = make_fake_report(EVENT_COUNT);
            let mut publisher = build_publisher();

            // First call initializes histogram bucket storage and creates
            // OpenTelemetry instruments. The measured iterations exercise the
            // steady-state path where this initialization is already done.
            publisher.run_one_iteration_with_report(&report);

            (RefCell::new(publisher), report)
        })
        .measure_resource_usage("nm_otel_export/steady_state_8_events", |measure| {
            measure.allocs(&allocs)
        })
        .iter(|args| {
            let (publisher, report) = args.thread_state();
            publisher.borrow_mut().run_one_iteration_with_report(report);
        })
        .execute_criterion_on(&mut one_thread, &mut group, "steady_state_8_events");

    group.finish();

    allocs.print_to_stdout();
}

fn build_publisher() -> Publisher {
    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter).build();
    let provider = SdkMeterProvider::builder().with_reader(reader).build();

    Publisher::builder()
        .provider(provider)
        .clock(Clock::new_frozen())
        .build()
}

fn make_fake_report(event_count: usize) -> Report {
    let events = (0..event_count)
        .map(|i| {
            let name = format!("bench_event_{i}");
            let histogram = Histogram::fake(
                HISTOGRAM_BUCKETS,
                HISTOGRAM_NON_CUMULATIVE.to_vec(),
                HISTOGRAM_PLUS_INFINITY,
            );
            EventMetrics::fake(name, HISTOGRAM_COUNT, HISTOGRAM_SUM, Some(histogram))
        })
        .collect();
    Report::fake(events)
}
