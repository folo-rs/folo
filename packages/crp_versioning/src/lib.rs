#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, doc(hidden))]
#![allow(
    missing_docs,
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    clippy::module_name_repetitions,
    reason = "Private application and maintainer-test wiring has no supported Rust API."
)]

//! Release assessment, version planning and captured application.

pub use check::*;
pub(crate) use crp_diag::{quote_path, short_commit};
pub(crate) use errors::*;

pub mod analysis_order;
pub mod anchor;
pub mod apply;
mod check;
pub mod classify;
mod diff;
mod errors;
pub mod expand;
pub mod groups;
mod inherited;
pub mod inspect_plan;
pub mod plan;
pub mod preview;
pub mod propose;
pub mod prospective;
pub mod report;
pub mod resolved;
pub mod semver_targets;

/// Internal algorithm driver for the owner-local benchmark.
#[cfg(any(test, feature = "private-test-util"))]
pub mod __private {
    pub use crate::diff::benchmark_patch_rendering;
}

#[cfg(test)]
::testing::set_allocator!();
