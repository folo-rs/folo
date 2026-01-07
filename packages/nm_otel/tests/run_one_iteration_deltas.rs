//! Integration test for delta computation across multiple iterations.
//!
//! This test is in a separate binary to establish controlled circumstances - no other tests
//! will have recorded nm events, so we can verify the exact metrics exported.

#![allow(
    clippy::indexing_slicing,
    reason = "integration tests with known-valid indices verified by earlier assertions"
)]

use nm::Event;
use nm_otel::publisher;
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader, SdkMeterProvider, data::Sum,
};
use tick::Clock;

thread_local! {
    static TEST_EVENT: Event = Event::builder()
        .name("delta_test_event")
        .build();
}

fn create_test_provider() -> (SdkMeterProvider, InMemoryMetricExporter) {
    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter.clone()).build();
    let provider = SdkMeterProvider::builder().with_reader(reader).build();
    (provider, exporter)
}

#[test]
fn run_one_iteration_computes_deltas_across_iterations() {
    // Record initial events.
    TEST_EVENT.with(|event| {
        event.batch(5).observe(50);
    });

    let (provider, exporter) = create_test_provider();

    let mut pub_instance = publisher()
        .provider(provider.clone())
        .clock(Clock::new_frozen())
        .build();

    // First iteration.
    pub_instance.run_one_iteration();
    provider.force_flush().unwrap();

    // Verify first iteration exported count of 5.
    let metrics = exporter.get_finished_metrics().unwrap();
    let mut first_count = None;
    for resource_metrics in &metrics {
        for scope_metrics in &resource_metrics.scope_metrics {
            for metric in &scope_metrics.metrics {
                if metric.name == "delta_test_event" {
                    let sum = metric.data.as_any().downcast_ref::<Sum<u64>>().unwrap();
                    first_count = Some(sum.data_points[0].value);
                }
            }
        }
    }
    assert_eq!(first_count, Some(5));

    // Record more events.
    TEST_EVENT.with(|event| {
        event.batch(3).observe(30);
    });

    // Second iteration.
    pub_instance.run_one_iteration();
    provider.force_flush().unwrap();

    // Verify second iteration shows cumulative count of 8.
    let metrics = exporter.get_finished_metrics().unwrap();
    let mut second_count = None;
    for resource_metrics in &metrics {
        for scope_metrics in &resource_metrics.scope_metrics {
            for metric in &scope_metrics.metrics {
                if metric.name == "delta_test_event" {
                    let sum = metric.data.as_any().downcast_ref::<Sum<u64>>().unwrap();
                    second_count = Some(sum.data_points[0].value);
                }
            }
        }
    }
    assert_eq!(second_count, Some(8));
}
