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
//! - Support for counters, gauges, and histograms
//! - Instruments are created and destroyed on-demand as events appear/disappear
//! - Testable design with clock abstraction for unit tests
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
//! Publisher::with_meter(meter)
//!     .interval(Duration::from_secs(10))
//!     .publish_forever()
//!     .await;
//! # }
//! ```

mod clock;
mod publisher;

pub use clock::*;
pub use publisher::*;
