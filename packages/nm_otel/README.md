# nm_otel

OpenTelemetry bridge for nm metrics.

This crate provides a bridge between [`nm`](https://crates.io/crates/nm) metrics and OpenTelemetry,
enabling export of nm-collected metrics to any OpenTelemetry-compatible backend.

## Example

```rust
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
```

## Exported metrics

Each `nm::Event` is exported as one or more OpenTelemetry metrics:

| nm data | OpenTelemetry type | Metric name |
|---------|--------------------|-------------|
| count | counter | `{event}` |
| sum | gauge | `{event}_sum` |
| histogram | counter per bucket | `{event}_bucket` with `le` attribute |

## Histogram format

Histograms are represented by cumulative bucket counters, an observation-count counter, and an
observation-sum gauge.

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

See the [package documentation](https://docs.rs/nm_otel/) for API details.

The package's user-visible behavior and internal architecture are described in the
[design](docs/design.md) and [implementation](docs/implementation.md) documents.

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
