#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![doc(hidden)]
#![expect(
    clippy::exhaustive_structs,
    reason = "this crate's `pub` items form an in-workspace handoff boundary to the \
              cargo-bench-history analysis detectors, not a stable public API. \
              Exhaustive construction and matching by those in-workspace consumers \
              is intended"
)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! Pure statistical primitives (medians, the Pettitt change-point test,
//! Mann–Whitney, Mann–Kendall, Theil–Sen, Benjamini–Hochberg) for the analysis
//! detectors, split out of `cargo-bench-history` so this deterministic,
//! I/O-free, Miri-safe math is cheap to mutation-test in isolation. The
//! `cbh_detect` detectors compose these primitives.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod stats;

pub use stats::*;
