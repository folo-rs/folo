#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, doc(hidden))]
#![allow(
    missing_docs,
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    reason = "Private application and maintainer-test wiring has no supported Rust API."
)]

//! Git, Cargo and source observations for cargo-release-plan.

pub(crate) use errors::*;

pub mod artifact_path;
pub mod command;
mod errors;
pub mod git;
pub mod identity;
pub mod inherited;
pub mod lockfile;
pub mod manifest;
pub mod metadata;
pub mod packaging;

#[cfg(any(test, feature = "private-test-util"))]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod testing;

/// Internal algorithm driver for the owner-local benchmark.
#[cfg(any(test, feature = "private-test-util"))]
pub mod __private {
    pub use crate::lockfile::benchmark_lockfile_closures;
}

#[cfg(test)]
::testing::set_allocator!();
