#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![doc(hidden)]
#![expect(
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    reason = "this crate's `pub` items form an in-workspace handoff boundary to the \
              cargo-bench-history shell crate, used only inside this workspace rather \
              than as a stable public API. Exhaustive construction and matching by \
              those in-workspace consumers is intended"
)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! Renders the analysis findings into the tool's terminal and Markdown output
//! (owning `ReportFormat` and the sparkline plotting). Split out of
//! `cargo-bench-history` so the `colored` and `rasciigraph` presentation
//! dependencies are confined here, away from the I/O-free detectors in
//! `cbh_analysis`.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

use cbh_analysis as analyze;
use cbh_model as model;

mod report;

pub use report::*;
