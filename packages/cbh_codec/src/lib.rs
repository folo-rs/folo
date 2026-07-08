#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![doc(hidden)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! Hosts the gzip byte format for stored objects, split out of
//! `cargo-bench-history-core` so its fast, I/O-free test suite is cheap to
//! mutation-test in isolation. The `cargo-bench-history` storage backends and the
//! stress harness call the same [`compress`]/[`decompress`] pair, so they can
//! never drift out of lockstep.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod codec;

pub use codec::*;
