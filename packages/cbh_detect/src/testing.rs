//! In-workspace test helpers exposed to the `cargo-bench-history` shell crate.
//!
//! Gated behind the `private-test-util` feature, so end-user builds never see it.

#![cfg_attr(coverage_nightly, coverage(off))]

use crate::Spawner;

/// Builds a spawner that runs immediately-ready analysis tasks inline.
#[must_use]
pub fn synchronous_spawner() -> Spawner {
    Spawner::synchronous()
}
