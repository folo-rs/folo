# nm_otel

OpenTelemetry bridge for [nm](https://crates.io/crates/nm) metrics.

This crate provides a bridge between the `nm` metrics collection system and OpenTelemetry, allowing you to export metrics collected by `nm` to OpenTelemetry-compatible backends.

## Features

- Periodic export of `nm` metrics to OpenTelemetry
- Automatic mapping of `nm` events to OpenTelemetry instruments:
  - Events with histogram buckets → `Histogram`
  - Events without histogram buckets → `UpDownCounter`
- Instruments are created and destroyed on-demand as events appear/disappear
- Testable design using the `tick` crate for time control

## Metric Type Mapping

The `nm` crate does not assign types to events - events are flexible and can collect data of any magnitude at any time. The only metadata available is whether an event has histogram buckets configured. Therefore, we make mapping decisions based solely on metadata:

- Events **with histogram buckets** → OpenTelemetry `Histogram<f64>`
- Events **without histogram buckets** → OpenTelemetry `UpDownCounter<i64>`

We use `UpDownCounter` for non-histogram events because it can handle arbitrary positive and negative magnitudes, matching the flexibility of `nm` events.

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

    Publisher::meter(meter)
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
