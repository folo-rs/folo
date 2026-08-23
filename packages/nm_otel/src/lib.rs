#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! # `nm_otel` - OpenTelemetry bridge for nm metrics
//!
//! This crate provides a bridge between [`nm`] metrics and OpenTelemetry,
//! enabling export of nm-collected metrics to any OpenTelemetry-compatible backend.
//!
//! ## Quick start
//!
//! ```no_run
//! use nm::Event;
//! use nm_otel::Publisher;
//! use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
//! use opentelemetry_stdout::MetricExporter;
//! use tick::Clock;
//!
//! thread_local! {
//!     static REQUESTS: Event = Event::builder().name("requests").build();
//! }
//!
//! # async fn example() {
//! let exporter = MetricExporter::default();
//! let reader = PeriodicReader::builder(exporter).build();
//! let meter_provider = SdkMeterProvider::builder().with_reader(reader).build();
//!
//! REQUESTS.with(Event::observe_once);
//!
//! let mut publisher = Publisher::builder()
//!     .provider(meter_provider)
//!     .clock(Clock::new_tokio())
//!     .build();
//!
//! publisher.publish_forever().await;
//! # }
//! ```
//!
//! ## Exported metrics
//!
//! Each [`nm::Event`][nm-event] is exported as one or more OpenTelemetry metrics:
//!
//! | nm data | OpenTelemetry type | Metric name |
//! |---------|--------------------|--------------|
//! | count | counter | `{event}` |
//! | sum | gauge | `{event}_sum` |
//! | histogram | counter per bucket | `{event}_bucket` with `le` attribute |
//!
//! ## Histogram format
//!
//! Histograms are represented by cumulative bucket counters, an observation-count counter,
//! and an observation-sum gauge.
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
//! - `interval()` - how often to collect nm data and record OpenTelemetry instruments
//!   (default: 60 seconds)
//! - `meter_name()` - OpenTelemetry meter name (default: "nm")
//! - `provider()` - any [`MeterProvider`][otel-meter-provider] implementation
//!
//! ## Requirements
//!
//! The publisher runs as an infinite async task that should be spawned in your application's
//! async runtime. Pass an appropriate [`tick::Clock`][tick-clock] to the builder for your
//! runtime, such as `Clock::new_tokio()` for Tokio. The OpenTelemetry provider independently
//! determines when its readers collect and export the instrument values recorded by the publisher.
//!
//! [`nm`]: https://crates.io/crates/nm
//! [nm-event]: https://docs.rs/nm/latest/nm/struct.Event.html
//! [otel-meter-provider]:
//!     https://docs.rs/opentelemetry/latest/opentelemetry/metrics/trait.MeterProvider.html
//! [tick-clock]: https://docs.rs/tick/latest/tick/struct.Clock.html

// This explicit list advertises only the public subset and excludes implementation-only items.
// Ref: docs/impl-crate-split.md, "The split"; packages/nm_otel/docs/implementation.md.
pub use nm_otel_impl::{Publisher, PublisherBuilder};
