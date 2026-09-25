//! Verifies candidate source snapshots before release scripts create missing tags.
//!
//! This compatibility executable delegates source validation to cargo-release-plan's
//! implementation partition. Its integration target retains the executable-boundary coverage.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![allow(
    missing_docs,
    reason = "This nonpublished library exposes implementation operations to its integration target, not a public API"
)]

pub use crp_impl::publication::candidate::{Metadata, Repository, capture, git, run};

#[cfg(test)]
::testing::set_allocator!();
