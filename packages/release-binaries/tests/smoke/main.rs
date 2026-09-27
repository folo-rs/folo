//! Bootstrap plan/run protocol, GitHub adapter boundaries and one native wiring smoke.

#![cfg(not(miri))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute), coverage(off))]
#![allow(
    clippy::indexing_slicing,
    reason = "Fixture JSON has an explicitly asserted shape"
)]

pub(crate) use harness::{
    Fixture, SMOKE_WATCHDOG, assert_success, command, compile_tool, run, write,
};

mod github;
mod harness;
mod packaging;
mod protocol;

::testing::set_allocator!();
