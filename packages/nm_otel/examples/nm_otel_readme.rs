//! Example from the `nm_otel` README.md file.
//!
//! This example demonstrates the basic usage pattern for exporting nm metrics
//! to OpenTelemetry.

use std::time::Duration;

use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use tick::Clock;

#[tokio::main]
async fn main() {
    // Set up an OpenTelemetry meter provider with your exporter of choice.
    let exporter = opentelemetry_stdout::MetricExporter::default();
    let reader = PeriodicReader::builder(exporter).build();
    let my_meter_provider = SdkMeterProvider::builder().with_reader(reader).build();

    // Create and run the nm-to-OpenTelemetry publisher.
    let mut nm_publisher = nm_otel::publisher()
        .provider(my_meter_provider)
        .clock(Clock::new_tokio())
        .interval(Duration::from_secs(60))
        .build();

    nm_publisher.publish_forever().await;
}
