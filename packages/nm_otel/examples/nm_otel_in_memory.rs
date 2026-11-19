//! Example: Export nm metrics to OpenTelemetry in memory.
//!
//! This example demonstrates how to use `nm_otel` with an in-memory OpenTelemetry exporter.
//!
//! The publisher periodically collects nm metrics and records OpenTelemetry
//! instruments. The provider's reader independently exports those values.

use std::error::Error;
use std::time::Duration;

use nm::{Event, Magnitude};
use nm_otel::Publisher;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use tick::Clock;
use tokio::spawn;
use tokio::time::sleep;

/// Keeps the demonstration responsive while leaving several recordings per export.
const NM_RECORDING_INTERVAL: Duration = Duration::from_secs(2);

/// Makes each export contain multiple nm recordings.
const OTEL_EXPORT_INTERVAL: Duration = Duration::from_secs(5);

/// Allows the publisher enough time to collect the initial activity.
const DEMO_DURATION: Duration = Duration::from_millis(2100);

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
async fn main() -> Result<(), Box<dyn Error>> {
    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter.clone())
        .with_interval(OTEL_EXPORT_INTERVAL)
        .build();
    let meter_provider = SdkMeterProvider::builder().with_reader(reader).build();
    let publisher_provider = meter_provider.clone();
    let clock = Clock::new_tokio();

    simulate_http_activity();

    println!("nm_otel example: recording and exporting metrics.");
    println!();
    println!("nm collection and instrument recording interval: {NM_RECORDING_INTERVAL:?}.");
    println!("OpenTelemetry reader export interval: {OTEL_EXPORT_INTERVAL:?}.");
    println!("The demonstration runs for {DEMO_DURATION:?} and then prints its collected export.");
    println!();

    let mut publisher = Publisher::builder()
        .provider(publisher_provider)
        .clock(clock)
        .interval(NM_RECORDING_INTERVAL)
        .build();

    let publisher_task = spawn(async move {
        publisher.publish_forever().await;
    });

    sleep(DEMO_DURATION).await;
    publisher_task.abort();
    _ = publisher_task.await;

    meter_provider.force_flush()?;
    let metrics = exporter.get_finished_metrics()?;
    meter_provider.shutdown()?;

    println!(
        "Collected {} OpenTelemetry resource metric batches.",
        metrics.len()
    );

    Ok(())
}

::testing::set_allocator!();
