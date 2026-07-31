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
//! Pure statistical primitives (means, medians and scatter, the Pettitt
//! change-point test, Mann–Whitney, Mann–Kendall, Theil–Sen,
//! Benjamini–Hochberg, the standard normal and Student-t distributions) for the
//! analysis detectors, split out of `cargo-bench-history` so this
//! deterministic, I/O-free, Miri-safe math is cheap to mutation-test in
//! isolation. The `cbh_detect` detectors compose these primitives.
//!
//! Every test here reports a two-sided p-value drawn from a common reportable
//! range, so the results can be fed to a shared false-discovery-rate filter
//! without any of them dominating it by underflowing to zero.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod normal;
mod p_value;
mod stats;
mod student_t;

#[cfg(test)]
mod test_util;

pub(crate) use normal::*;
pub(crate) use p_value::*;
pub use stats::*;
pub use student_t::*;
