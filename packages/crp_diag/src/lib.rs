#![cfg_attr(
    all(coverage_nightly, any(test, feature = "private-test-util")),
    feature(coverage_attribute)
)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, doc(hidden))]
#![allow(
    missing_docs,
    clippy::exhaustive_structs,
    reason = "This package is private application and maintainer-test wiring, not a supported Rust API."
)]

//! Diagnostic implementation for the cargo-release-plan application.

pub use report::*;
pub use text::*;

mod report;
mod text;

#[cfg(test)]
::testing::set_allocator!();
