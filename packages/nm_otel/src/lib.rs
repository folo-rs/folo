#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! # `nm_otel` - OpenTelemetry bridge for nm metrics
//!
//! This crate provides a bridge between [`nm`] metrics and OpenTelemetry,
//! enabling export of nm-collected metrics to any OpenTelemetry-compatible backend.
//!
//! ## Quick start
//!
//! ```no_run
//! # // This example shows the general pattern but cannot run as a doctest because it
//! # // would loop forever.
//! use std::time::Duration;
//! use nm_otel::Publisher;
//! use tick::Clock;
//! # use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
//!
//! # async fn example() {
//! # let exporter = InMemoryMetricExporter::default();
//! # let reader = PeriodicReader::builder(exporter).build();
//! # let my_meter_provider = SdkMeterProvider::builder().with_reader(reader).build();
//! // In your async runtime, periodically export all nm metrics to OpenTelemetry.
//! Publisher::builder()
//!     .provider(my_meter_provider)
//!     .clock(Clock::new_tokio())
//!     .interval(Duration::from_secs(60))
//!     .build()
//!     .publish_forever()
//!     .await;
//! # }
//! ```
//!
//! ## Exported metrics
//!
//! Each [`nm::Event`] is exported as one or more OpenTelemetry metrics:
//!
//! | nm data | OpenTelemetry type | Metric name |
//! |---------|--------------------|--------------|
//! | count | counter | `{event}` |
//! | sum | gauge | `{event}_sum` |
//! | histogram | counter per bucket | `{event}_bucket` with `le` attribute |
//!
//! ## Histogram format
//!
//! Histograms are exported as separate counter and gauge metrics because the OpenTelemetry
//! Rust SDK does not yet support recording pre-aggregated histogram data. See
//! [opentelemetry-rust#2505](https://github.com/open-telemetry/opentelemetry-rust/issues/2505).
//!
//! The format uses cumulative bucket counts with a `le` (less-than-or-equal) attribute:
//!
//! ```text
//! http_latency_ms_bucket{le="10"}   → observations ≤ 10
//! http_latency_ms_bucket{le="50"}   → observations ≤ 50
//! http_latency_ms_bucket{le="100"}  → observations ≤ 100
//! http_latency_ms_bucket{le="+Inf"} → all observations
//! http_latency_ms                   → total observation count
//! http_latency_ms_sum               → sum of all observed values
//! ```
//!
//! ## Configuration
//!
//! Use [`PublisherBuilder`] to configure the publisher:
//!
//! - `interval()` - how often to collect and export metrics (default: 5 seconds)
//! - `meter_name()` - OpenTelemetry meter name (default: "nm")
//! - `provider()` - custom [`SdkMeterProvider`][opentelemetry_sdk::metrics::SdkMeterProvider]
//!
//! ## Requirements
//!
//! The publisher runs as an infinite async task that should be spawned
//! in your application's async runtime. Pass an appropriate [`tick::Clock`]
//! to the builder for your runtime (e.g., `Clock::new_tokio()` for Tokio).

mod mapping;
mod publisher;
mod state;

pub use publisher::*;
