# nm_otel

OpenTelemetry bridge for nm metrics.

This crate provides a bridge between [`nm`](https://crates.io/crates/nm) metrics and OpenTelemetry,
enabling export of nm-collected metrics to any OpenTelemetry-compatible backend.

## Example

```rust
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
    let mut nm_publisher = nm_otel::Publisher::builder()
        .provider(my_meter_provider)
        .clock(Clock::new_tokio())
        .interval(Duration::from_secs(60))
        .build();

    nm_publisher.publish_forever().await;
}
```

## Exported metrics

Each `nm::Event` is exported as one or more OpenTelemetry metrics:

| nm data | OpenTelemetry type | Metric name |
|---------|--------------------|-------------|
| count | counter | `{event}` |
| sum | gauge | `{event}_sum` |
| histogram | counter per bucket | `{event}_bucket` with `le` attribute |

## Histogram format

Histograms are exported as separate counter and gauge metrics because the OpenTelemetry
Rust SDK does not yet support recording pre-aggregated histogram data.

The format uses cumulative bucket counts with a `le` (less-than-or-equal) attribute:

```text
http_latency_ms_bucket{le="10"}   → observations ≤ 10
http_latency_ms_bucket{le="50"}   → observations ≤ 50
http_latency_ms_bucket{le="100"}  → observations ≤ 100
http_latency_ms_bucket{le="+Inf"} → all observations
http_latency_ms                   → total observation count
http_latency_ms_sum               → sum of all observed values
```

## See also

More details in the [package documentation](https://docs.rs/nm_otel/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
