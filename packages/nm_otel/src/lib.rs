#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! # `nm_otel` - OpenTelemetry bridge for nm metrics
//!
//! This crate provides a bridge between the [`nm`] metrics collection system and OpenTelemetry,
//! allowing you to export metrics collected by `nm` to OpenTelemetry-compatible backends.
//!
//! # Features
//!
//! - Periodic export of `nm` metrics to OpenTelemetry
//! - Automatic mapping of `nm` events to OpenTelemetry instruments
//! - Instruments are created and destroyed on-demand as events appear/disappear
//! - Testable design with clock abstraction for unit tests
//!
//! # Metric Type Mapping
//!
//! The `nm` crate does not assign types to events - events are flexible and can collect data
//! of any magnitude at any time. The only metadata available is whether an event has histogram
//! buckets configured. Therefore, we make mapping decisions based solely on metadata:
//!
//! - Events **with histogram buckets** → OpenTelemetry `Histogram<f64>`
//! - Events **without histogram buckets** → OpenTelemetry `UpDownCounter<i64>`
//!
//! We use `UpDownCounter` for non-histogram events because it can handle arbitrary positive
//! and negative magnitudes, matching the flexibility of `nm` events. We cannot assume an event
//! will only have magnitude 1 today just because it did yesterday - the data can change at any time.
//!
//! # Basic usage
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use nm_otel::Publisher;
//!
//! # async fn example() {
//! Publisher::new()
//!     .interval(Duration::from_secs(10))
//!     .publish_forever()
//!     .await;
//! # }
//! ```
//!
//! # Example with custom meter provider
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use nm_otel::Publisher;
//! use opentelemetry_sdk::metrics::MeterProvider;
//!
//! # async fn example() {
//! let meter_provider = MeterProvider::builder().build();
//! let meter = meter_provider.meter("my_app");
//!
//! Publisher::meter(meter)
//!     .interval(Duration::from_secs(10))
//!     .publish_forever()
//!     .await;
//! # }
//! ```

mod publisher;

pub use publisher::*;
