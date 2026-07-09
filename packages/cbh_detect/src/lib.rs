#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![expect(
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    reason = "this crate's `pub` items form an in-workspace handoff boundary to the \
              cargo-bench-history shell crate (and the cbh_render crate), used only \
              inside this workspace rather than as a stable public API. Exhaustive \
              construction and matching by those in-workspace consumers is intended"
)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! The I/O-free analysis engine: the timeline reconstruction, series building, and
//! change-point detectors that turn already-loaded result sets and git topology
//! (passed in as plain data) into detected findings. Split out of
//! `cargo-bench-history` so this deterministic, Miri-safe analysis math is
//! cheap to mutation-test in isolation. The `cbh_render` crate turns the findings
//! into a rendered report, and the `cargo-bench-history` shell wires storage and git
//! around it.
//!
//! Every type is re-exported flat from the crate root, so consumers write
//! `cbh_detect::Finding` rather than reaching into a submodule.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod detect;

pub use detect::*;

#[cfg(feature = "private-test-util")]
#[cfg_attr(docsrs, doc(cfg(feature = "private-test-util")))]
pub mod testing;
