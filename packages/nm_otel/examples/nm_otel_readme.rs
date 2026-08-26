//! Example from the `nm_otel` README.md file.
//!
//! This example demonstrates the basic usage pattern for exporting nm metrics
//! to OpenTelemetry.

use nm::Event;
use nm_otel::Publisher;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_stdout::MetricExporter;
use tick::Clock;

thread_local! {
    static REQUESTS: Event = Event::builder().name("requests").build();
}

#[tokio::main]
async fn main() {
    let exporter = MetricExporter::default();
    let reader = PeriodicReader::builder(exporter).build();
    let meter_provider = SdkMeterProvider::builder().with_reader(reader).build();

    REQUESTS.with(Event::observe_once);

    let mut publisher = Publisher::builder()
        .provider(meter_provider)
        .clock(Clock::new_tokio())
        .build();

    publisher.publish_forever().await;
}
