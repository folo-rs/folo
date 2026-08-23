#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]

//! Implementation crate for the [`nm_otel`] crate.
//!
//! This crate contains the entire implementation of `nm_otel`. The `nm_otel` crate
//! itself is a thin shell that re-exports the public-API subset of items defined here.
//!
//! **Do not depend on `nm_otel_impl` directly.** Anything beyond what `nm_otel`
//! re-exports is internal to this workspace and may change at any time, including in
//! patch releases. See the [`nm_otel` implementation guide] for its role in the
//! package architecture and [`docs/impl-crate-split.md`] for the broader convention.
//!
//! [`nm_otel`]: https://crates.io/crates/nm_otel
//! [`nm_otel` implementation guide]:
//!     https://github.com/folo-rs/folo/blob/main/packages/nm_otel/docs/implementation.md
//! [`docs/impl-crate-split.md`]: https://github.com/folo-rs/folo/blob/main/docs/impl-crate-split.md

mod mapping;
mod publisher;
mod state;
#[cfg(any(test, feature = "private-test-util"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test_metric_reader;

pub use publisher::*;
pub use state::*;
#[cfg(any(test, feature = "private-test-util"))]
#[doc(hidden)]
pub use test_metric_reader::*;
