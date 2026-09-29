//! Native release contracts: tagged sources, shared builds and independent publication.

#![cfg(not(miri))]
#![cfg_attr(coverage_nightly, coverage(off))]
#![allow(
    clippy::indexing_slicing,
    reason = "Fixture JSON has an explicitly asserted shape"
)]

pub(crate) use harness::{
    FIXTURE_VERSION, Fixture, SMOKE_WATCHDOG, assert_success, command, compile_tool, run, write,
};

mod artifacts;
mod cancellation;
#[cfg(windows)]
mod cancellation_windows;
mod harness;
mod packaging;
mod sources;
