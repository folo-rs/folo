//! Integration tests for `nm_otel` publisher.
//!
//! These tests verify that the publisher correctly exports nm metrics to OpenTelemetry.

use std::time::Duration;

use nm::{Event, Magnitude};
use nm_otel::Publisher;
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;

/// Test that the publisher can collect and export a simple counter event.
#[tokio::test]
async fn publisher_exports_counter_event() {
    thread_local! {
        static TEST_EVENT: Event = Event::builder()
            .name("test_counter_event_exports")
            .build();
    }

    // Observe some events.
    TEST_EVENT.with(Event::observe_once);
    TEST_EVENT.with(Event::observe_once);
    TEST_EVENT.with(Event::observe_once);

    // Create a publisher and run one iteration.
    let meter_provider = SdkMeterProvider::builder().build();
    let meter = meter_provider.meter("test");
    let publisher = Publisher::with_meter(meter);

    publisher.run_once_iteration().await;

    // We cannot easily assert on the exported metrics without implementing a custom exporter,
    // but we can verify that the operation completed without panicking.
}

/// Test that the publisher can collect and export a gauge event.
#[tokio::test]
async fn publisher_exports_gauge_event() {
    thread_local! {
        static TEST_EVENT: Event = Event::builder()
            .name("test_gauge_event_exports")
            .build();
    }

    // Observe events with different magnitudes.
    TEST_EVENT.with(|e| e.observe(10));
    TEST_EVENT.with(|e| e.observe(20));
    TEST_EVENT.with(|e| e.observe(30));

    // Create a publisher and run one iteration.
    let meter_provider = SdkMeterProvider::builder().build();
    let meter = meter_provider.meter("test");
    let publisher = Publisher::with_meter(meter);

    publisher.run_once_iteration().await;
}

/// Test that the publisher can collect and export a histogram event.
#[tokio::test]
async fn publisher_exports_histogram_event() {
    const BUCKETS: &[Magnitude] = &[10, 50, 100, 500];

    thread_local! {
        static TEST_EVENT: Event = Event::builder()
            .name("test_histogram_event_exports")
            .histogram(BUCKETS)
            .build();
    }

    // Observe events with different magnitudes that will fall into different buckets.
    TEST_EVENT.with(|e| e.observe(5));
    TEST_EVENT.with(|e| e.observe(25));
    TEST_EVENT.with(|e| e.observe(75));
    TEST_EVENT.with(|e| e.observe(150));
    TEST_EVENT.with(|e| e.observe(600));

    // Create a publisher and run one iteration.
    let meter_provider = SdkMeterProvider::builder().build();
    let meter = meter_provider.meter("test");
    let publisher = Publisher::with_meter(meter);

    publisher.run_once_iteration().await;
}

/// Test that the publisher can be configured with a custom interval.
#[tokio::test]
async fn publisher_interval_configuration() {
    let meter_provider = SdkMeterProvider::builder().build();
    let meter = meter_provider.meter("test");

    let publisher = Publisher::with_meter(meter).interval(Duration::from_secs(5));

    // Run one iteration to verify it works.
    publisher.run_once_iteration().await;
}

/// Test that running multiple iterations works correctly.
#[tokio::test]
async fn publisher_multiple_iterations() {
    thread_local! {
        static TEST_EVENT: Event = Event::builder()
            .name("test_multiple_iterations_event")
            .build();
    }

    let meter_provider = SdkMeterProvider::builder().build();
    let meter = meter_provider.meter("test");
    let publisher = Publisher::with_meter(meter);

    // Run first iteration.
    TEST_EVENT.with(Event::observe_once);
    publisher.run_once_iteration().await;

    // Run second iteration with more events.
    TEST_EVENT.with(Event::observe_once);
    TEST_EVENT.with(Event::observe_once);
    publisher.run_once_iteration().await;

    // Run third iteration.
    TEST_EVENT.with(Event::observe_once);
    publisher.run_once_iteration().await;
}
