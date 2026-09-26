#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, doc(hidden))]
#![allow(
    missing_docs,
    reason = "This package is private application and maintainer-test wiring, not a supported Rust API."
)]

//! Diagnostic implementation for the cargo-release-plan application.

pub use report::*;
pub use text::*;

mod report;
mod text;

#[cfg(test)]
::testing::set_allocator!();
