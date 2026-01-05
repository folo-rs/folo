# nm_otel

OpenTelemetry bridge for [nm](https://crates.io/crates/nm) metrics.

This crate provides a bridge between the `nm` metrics collection system and OpenTelemetry, allowing you to export metrics collected by `nm` to OpenTelemetry-compatible backends.

## Features

- Periodic export of `nm` metrics to OpenTelemetry
- Automatic mapping of `nm` events to OpenTelemetry instruments:
  - Simple counters (events with only magnitude 1) → `Counter`
  - Events with arbitrary magnitudes → `UpDownCounter`
  - Events with histograms → `Histogram`
- Instruments are created and destroyed on-demand as events appear/disappear
- Testable design with clock abstraction for unit tests

## Basic Usage

```rust
use std::time::Duration;
use nm_otel::Publisher;

#[tokio::main]
async fn main() {
    Publisher::new()
        .interval(Duration::from_secs(10))
        .publish_forever()
        .await;
}
```

## Custom Meter Provider

```rust
use std::time::Duration;
use nm_otel::Publisher;
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;

#[tokio::main]
async fn main() {
    let meter_provider = SdkMeterProvider::builder().build();
    let meter = meter_provider.meter("my_app");

    Publisher::with_meter(meter)
        .interval(Duration::from_secs(10))
        .publish_forever()
        .await;
}
```

## Example

See the [nm_otel_console](examples/nm_otel_console.rs) example for a complete working example that demonstrates:

- Defining `nm` events
- Collecting metrics during fake work
- Publishing to OpenTelemetry with a console exporter

Run the example with:

```bash
cargo run --example nm_otel_console
```

## License

MIT
