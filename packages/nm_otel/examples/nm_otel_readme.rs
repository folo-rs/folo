//! Example from the `nm_otel` README.md file.
//!
//! This example demonstrates the basic usage pattern for exporting nm metrics
//! to OpenTelemetry.

use std::error::Error;
use std::time::Duration;

use nm::Event;
use nm_otel::Publisher;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use tick::Clock;

thread_local! {
    static REQUESTS: Event = Event::builder().name("requests").build();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter.clone()).build();
    let meter_provider = SdkMeterProvider::builder().with_reader(reader).build();
    let publisher_provider = meter_provider.clone();

    REQUESTS.with(Event::observe_once);

    let mut publisher = Publisher::builder()
        .provider(publisher_provider)
        .clock(Clock::new_tokio())
        .interval(Duration::from_millis(10))
        .build();

    let publisher_task = tokio::spawn(async move {
        publisher.publish_forever().await;
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
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
