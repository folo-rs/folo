//! Compatibility entry point for the shared cargo-release-plan native binary engine.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]

pub use crp_impl::publication::binaries::run;

#[cfg(test)]
::testing::set_allocator!();
