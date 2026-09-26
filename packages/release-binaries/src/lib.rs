//! Compatibility entry point for the shared cargo-release-plan native binary engine.

pub use crp_impl::publication::binaries::run;

#[cfg(test)]
::testing::set_allocator!();
