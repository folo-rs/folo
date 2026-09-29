//! Development-only executable location for publication boundary tests.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

mod locate;

pub use locate::*;

#[cfg(test)]
::testing::set_allocator!();
