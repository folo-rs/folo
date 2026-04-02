//! Integration test for `Publisher::run_one_iteration()`.
//!
//! This test is in a separate binary to establish controlled circumstances - no other tests
//! will have recorded nm events, so we can verify the exact metrics exported.

#![allow(
    clippy::indexing_slicing,
    reason = "integration tests with known-valid indices verified by earlier assertions"
)]

use nm::Event;
use nm_otel::Publisher;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use tick::Clock;

thread_local! {
    static TEST_EVENT: Event = Event::builder()
        .name("integration_test_event")
        .build();
}

fn create_test_provider() -> (SdkMeterProvider, InMemoryMetricExporter) {
    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter.clone()).build();
    let provider = SdkMeterProvider::builder().with_reader(reader).build();
    (provider, exporter)
}

// OpenTelemetry SDK uses system time calls not available under Miri isolation.
#[cfg_attr(miri, ignore)]
#[test]
fn run_one_iteration_exports_recorded_events() {
    // Record some events before creating the publisher.
    TEST_EVENT.with(|event| {
        event.batch(10).observe(100);
    });

    let (provider, exporter) = create_test_provider();

    let mut pub_instance = Publisher::builder()
        .provider(provider.clone())
        .clock(Clock::new_frozen())
        .build();

    // Run one iteration - this should collect and export our recorded events.
    pub_instance.run_one_iteration();
    provider.force_flush().unwrap();

    let metrics = exporter.get_finished_metrics().unwrap();
    assert!(!metrics.is_empty(), "should have exported some metrics");

    // Find our test event's count metric.
    let mut found_count = false;
    for resource_metrics in &metrics {
        for scope_metrics in resource_metrics.scope_metrics() {
            for metric in scope_metrics.metrics() {
                if metric.name() == "integration_test_event" {
                    found_count = true;

                    // Verify it is a counter with the expected value.
                    if let AggregatedMetrics::U64(metric_data) = metric.data() {
                        if let MetricData::Sum(sum) = metric_data {
                            assert!(sum.is_monotonic());
                            let data_points: Vec<_> = sum.data_points().collect();
                            assert_eq!(data_points.len(), 1);
                            assert_eq!(data_points[0].value(), 10);
                        } else {
                            panic!("expected Sum metric data");
                        }
                    } else {
                        panic!("expected U64 aggregated metrics");
                    }
                }
            }
        }
    }

    assert!(found_count, "should have found our test event count metric");
}
