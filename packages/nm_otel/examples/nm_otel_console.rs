//! Example: Export nm metrics to OpenTelemetry with console output.
//!
//! This example demonstrates how to use `nm_otel` to export nm metrics
//! to an OpenTelemetry console exporter.
//!
//! The publisher periodically collects nm metrics and records OpenTelemetry
//! instruments. The provider's reader independently exports those values.

use std::time::Duration;

use nm::{Event, Magnitude};
use nm_otel::Publisher;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_stdout::MetricExporter;
use tick::Clock;
use tokio::spawn;
use tokio::time::sleep;

/// Keeps the demonstration responsive while leaving several recordings per export.
const NM_RECORDING_INTERVAL: Duration = Duration::from_secs(2);

/// Makes each console export contain multiple nm recordings.
const OTEL_EXPORT_INTERVAL: Duration = Duration::from_secs(5);

/// Generates activity between successive nm recordings.
const ACTIVITY_INTERVAL: Duration = Duration::from_millis(500);

/// Spans the simulated response-time range and separates representative samples.
const RESPONSE_TIME_BUCKETS_MS: &[Magnitude] = &[1, 5, 10, 50, 100, 500, 1000];

thread_local! {
    /// Tracks HTTP request counts.
    static HTTP_REQUESTS: Event = Event::builder()
        .name("http_requests")
        .build();

    /// Tracks HTTP response times using a histogram.
    static HTTP_RESPONSE_TIME_MS: Event = Event::builder()
        .name("http_response_time_ms")
        .histogram(RESPONSE_TIME_BUCKETS_MS)
        .build();

    /// Tracks bytes transferred.
    static BYTES_TRANSFERRED: Event = Event::builder()
        .name("bytes_transferred")
        .build();
}

/// Records representative HTTP activity for the demonstration.
fn simulate_http_activity() {
    for i in 0..10 {
        HTTP_REQUESTS.with(Event::observe_once);

        // This sequence crosses several histogram boundaries in every batch.
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "The bounded demonstration range keeps the arithmetic within i64."
        )]
        let response_time = (i * 15) + 5;
        HTTP_RESPONSE_TIME_MS.with(|event| event.observe(response_time));

        // Increasing payloads make the sum metric visibly differ from the count.
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "The bounded demonstration range keeps the arithmetic within i64."
        )]
        let bytes = (i + 1) * 1024;
        BYTES_TRANSFERRED.with(|event| event.observe(bytes));
    }
}

#[tokio::main]
async fn main() {
    let exporter = MetricExporter::default();
    let reader = PeriodicReader::builder(exporter)
        .with_interval(OTEL_EXPORT_INTERVAL)
        .build();
    let meter_provider = SdkMeterProvider::builder().with_reader(reader).build();
    let clock = Clock::new_tokio();

    simulate_http_activity();

    println!("nm_otel example: recording and exporting metrics.");
    println!();
    println!("nm collection and instrument recording interval: {NM_RECORDING_INTERVAL:?}.");
    println!("OpenTelemetry reader export interval: {OTEL_EXPORT_INTERVAL:?}.");
    println!("Press Ctrl+C to exit.");
    println!();

    let mut publisher = Publisher::builder()
        .provider(meter_provider)
        .clock(clock)
        .interval(NM_RECORDING_INTERVAL)
        .build();

    _ = spawn(async {
        loop {
            sleep(ACTIVITY_INTERVAL).await;
            simulate_http_activity();
        }
    });

    publisher.publish_forever().await;
}
