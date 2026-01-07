//! Integration test for `Publisher::publish_forever()`.
//!
//! This test is in a separate binary to establish controlled circumstances - no other tests
//! will have recorded nm events, so we can verify the exact metrics exported.

#![allow(
    clippy::indexing_slicing,
    reason = "integration tests with known-valid indices verified by earlier assertions"
)]

use std::pin::pin;
use std::task::{Context, Waker};
use std::time::Duration;

use nm::Event;
use nm_otel::publisher;
use opentelemetry_sdk::metrics::data::Sum;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use tick::ClockControl;

thread_local! {
    static TEST_EVENT: Event = Event::builder()
        .name("publish_forever_test_event")
        .build();
}

fn create_test_provider() -> (SdkMeterProvider, InMemoryMetricExporter) {
    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter.clone()).build();
    let provider = SdkMeterProvider::builder().with_reader(reader).build();
    (provider, exporter)
}

#[test]
fn publish_forever_collects_metrics_on_timer_tick() {
    const INTERVAL: Duration = Duration::from_secs(5);

    // Record some events before creating the publisher.
    TEST_EVENT.with(|event| {
        event.batch(10).observe(100);
    });

    let (provider, exporter) = create_test_provider();

    // Create a clock with manual time control.
    let control = ClockControl::new();
    let clock = control.to_clock();

    let mut pub_instance = publisher()
        .provider(provider.clone())
        .clock(clock)
        .interval(INTERVAL)
        .build();

    // Create the future but do not await it - we will poll manually.
    let mut future = pin!(pub_instance.publish_forever());

    // Create a no-op waker for polling.
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);

    // First poll - the future should be pending (waiting for timer).
    let poll_result = future.as_mut().poll(&mut cx);
    assert!(poll_result.is_pending());

    // Advance time past the interval to trigger the timer.
    control.advance(INTERVAL);

    // Poll again - this should execute one iteration and then suspend again.
    let poll_result = future.as_mut().poll(&mut cx);
    assert!(poll_result.is_pending());

    // Flush and check that metrics were exported.
    provider.force_flush().unwrap();

    let metrics = exporter.get_finished_metrics().unwrap();
    assert!(!metrics.is_empty(), "should have exported some metrics");

    // Find our test event's count metric.
    let mut found_count = false;
    for resource_metrics in &metrics {
        for scope_metrics in &resource_metrics.scope_metrics {
            for metric in &scope_metrics.metrics {
                if metric.name == "publish_forever_test_event" {
                    found_count = true;

                    // Verify it is a counter with the expected value.
                    let sum = metric.data.as_any().downcast_ref::<Sum<u64>>().unwrap();
                    assert!(sum.is_monotonic);
                    assert_eq!(sum.data_points.len(), 1);
                    assert_eq!(sum.data_points[0].value, 10);
                }
            }
        }
    }

    assert!(found_count, "should have found our test event count metric");
}
