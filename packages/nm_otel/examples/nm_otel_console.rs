//! Example: Export nm metrics to OpenTelemetry with console output.
//!
//! This example demonstrates how to use `nm_otel` to export nm metrics
//! to an OpenTelemetry console exporter.
//!
//! The publisher runs as an async task, periodically collecting nm metrics
//! and exporting them to the configured OpenTelemetry provider.

use std::time::Duration;

use nm::{Event, Magnitude};
use nm_otel::Publisher;
use opentelemetry::global;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use tick::Clock;

/// Response time histogram buckets in milliseconds.
const RESPONSE_TIME_BUCKETS_MS: &[Magnitude] = &[1, 5, 10, 50, 100, 500, 1000];

thread_local! {
    /// Tracks HTTP request counts.
    static HTTP_REQUESTS: Event = Event::builder()
        .name("http_requests")
        .build();

    /// Tracks HTTP response times with histogram.
    static HTTP_RESPONSE_TIME_MS: Event = Event::builder()
        .name("http_response_time_ms")
        .histogram(RESPONSE_TIME_BUCKETS_MS)
        .build();

    /// Tracks bytes transferred.
    static BYTES_TRANSFERRED: Event = Event::builder()
        .name("bytes_transferred")
        .build();
}

/// Simulates some HTTP activity for demonstration purposes.
fn simulate_http_activity() {
    // Simulate some requests with various response times.
    for i in 0..10 {
        HTTP_REQUESTS.with(Event::observe_once);

        // Vary response times across the histogram buckets.
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "safe demonstration code with small values"
        )]
        let response_time = (i * 15) + 5;
        HTTP_RESPONSE_TIME_MS.with(|e| e.observe(response_time));

        // Simulate varying payload sizes.
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "safe demonstration code with small values"
        )]
        let bytes = (i + 1) * 1024;
        BYTES_TRANSFERRED.with(|e| e.observe(bytes));
    }
}

#[tokio::main]
async fn main() {
    // Create a simple stdout exporter for demonstration.
    let exporter = opentelemetry_stdout::MetricExporter::default();

    // Set up the periodic reader with a short interval for demo purposes.
    let reader = PeriodicReader::builder(exporter)
        .with_interval(Duration::from_secs(5))
        .build();

    // Build the meter provider.
    let provider = SdkMeterProvider::builder().with_reader(reader).build();

    // Register as global provider.
    global::set_meter_provider(provider.clone());

    // Create a clock for timing.
    let clock = Clock::new_tokio();

    // Simulate some activity before the first collection.
    simulate_http_activity();

    println!("nm_otel example: Collecting and exporting metrics...");
    println!();
    println!("Metrics are exported every 2 seconds.");
    println!("Press Ctrl+C to exit.");
    println!();

    // Create the nm-to-OpenTelemetry publisher.
    let mut nm_publisher = Publisher::builder()
        .provider(provider)
        .clock(clock)
        .interval(Duration::from_secs(2))
        .build();

    // Spawn a task to simulate ongoing activity.
    tokio::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            simulate_http_activity();
        }
    });

    // Run the publisher forever - this is the primary API for production use.
    // The publisher will periodically collect nm metrics and export them.
    nm_publisher.publish_forever().await;
}
