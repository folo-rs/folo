//! Stand-alone example showcasing `nm_otel` functionality.
//!
//! This example demonstrates how to:
//! 1. Define nm events to track metrics
//! 2. Perform fake work in a loop
//! 3. Publish metrics to OpenTelemetry every 5 seconds
//! 4. Use the console exporter to see the metrics

use std::thread;
use std::time::Duration;

use nm::{Event, Magnitude};
use nm_otel::Publisher;
use opentelemetry::global;
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_stdout::MetricExporterBuilder;

const WORK_DURATION_MS_BUCKETS: &[Magnitude] = &[1, 5, 10, 20, 50, 100, 200, 500];

thread_local! {
    static WORK_ITEMS_PROCESSED: Event = Event::builder()
        .name("work_items_processed")
        .build();

    static WORK_DURATION_MS: Event = Event::builder()
        .name("work_duration_ms")
        .histogram(WORK_DURATION_MS_BUCKETS)
        .build();

    static BYTES_PROCESSED: Event = Event::builder()
        .name("bytes_processed")
        .build();
}

#[tokio::main]
async fn main() {
    // Exit early if running in a testing environment.
    if std::env::var("IS_TESTING").is_ok() {
        println!("Running in testing mode - exiting immediately to prevent infinite loop");
        return;
    }

    // Set up OpenTelemetry with stdout exporter.
    let exporter = MetricExporterBuilder::default().build();
    let reader = opentelemetry_sdk::metrics::PeriodicReader::builder(exporter)
        .with_interval(Duration::from_secs(5))
        .build();

    let meter_provider = SdkMeterProvider::builder().with_reader(reader).build();

    global::set_meter_provider(meter_provider.clone());
    let meter = meter_provider.meter("nm_otel_example");

    // Start the nm_otel publisher in a separate task.
    let publisher = Publisher::with_meter(meter).interval(Duration::from_secs(5));

    tokio::spawn(async move {
        publisher.publish_forever().await;
    });

    // Simulate fake work in the main loop.
    println!("Starting fake work loop. Press Ctrl+C to exit.");
    println!("Metrics will be exported every 5 seconds.");
    println!();

    loop {
        // Simulate processing work items.
        perform_fake_work();

        // Sleep for a bit before the next iteration.
        thread::sleep(Duration::from_secs(1));
    }
}

fn perform_fake_work() {
    // Use a simple counter instead of random for simplicity.
    static mut COUNTER: u8 = 0;

    // Simulate processing 3 work items.
    for _ in 0..3 {
        WORK_ITEMS_PROCESSED.with(Event::observe_once);

        // Simulate work taking some time (10-100ms).
        // SAFETY: Single read of mutable static, single-threaded example.
        let counter = unsafe { COUNTER };
        let new_counter = counter.wrapping_add(1);

        // SAFETY: Single write to mutable static, single-threaded example.
        unsafe {
            COUNTER = new_counter;
        }

        let duration_ms = (new_counter % 91).checked_add(10).expect("safe range");

        thread::sleep(Duration::from_millis(duration_ms.into()));
        WORK_DURATION_MS.with(|e| e.observe(i64::from(duration_ms)));

        // Simulate processing some bytes (100-10000 bytes).
        // SAFETY: Single read of mutable static, single-threaded example.
        let counter = unsafe { COUNTER };
        let new_counter = counter.wrapping_add(17);

        // SAFETY: Single write to mutable static, single-threaded example.
        unsafe {
            COUNTER = new_counter;
        }

        let bytes = u16::from(new_counter)
            .checked_mul(39)
            .and_then(|v| v.checked_add(100))
            .expect("safe range");

        BYTES_PROCESSED.with(|e| e.observe(i64::from(bytes)));
    }
}
