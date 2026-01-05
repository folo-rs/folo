//! This example verifies that the code in README.md works correctly.

use std::time::Duration;

use nm_otel::Publisher;
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;

#[tokio::main]
async fn main() {
    // Exit early if running in a testing environment.
    if std::env::var("IS_TESTING").is_ok() {
        println!("Running in testing mode - exiting immediately");
        return;
    }

    // Basic usage example from README (just showing the pattern).
    // This would run forever, so we just show the construction.
    let _unused_future = async {
        Publisher::new()
            .interval(Duration::from_secs(10))
            .publish_forever()
            .await;
    };
    drop(_unused_future); // Explicitly drop to avoid no_effect_underscore_binding warning

    // Custom meter provider example from README.
    let meter_provider = SdkMeterProvider::builder().build();
    let meter = meter_provider.meter("my_app");

    let publisher = Publisher::with_meter(meter).interval(Duration::from_secs(10));

    // Run one iteration to verify it works.
    publisher.run_once_iteration().await;

    println!("README examples verified successfully!");
}
