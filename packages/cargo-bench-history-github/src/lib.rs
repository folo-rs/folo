#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, doc(hidden))]

//! Unsupported GitHub automation companion for `cargo-bench-history`.
//!
//! The package is published only so the reusable action can install a tested
//! binary. Its library API and command line may change without a semver major
//! release.

mod cli;
mod errors;
mod github;
mod identity;
mod lifecycle;
mod marker;
mod message;
mod model;
mod operations;
mod result;
mod workflow;

#[cfg(any(test, feature = "private-test-util"))]
mod private_test_util;

pub use cli::Cli;
pub use operations::run;

#[cfg(any(test, feature = "private-test-util"))]
#[doc(hidden)]
pub mod __private {
    pub use crate::private_test_util::*;
}
