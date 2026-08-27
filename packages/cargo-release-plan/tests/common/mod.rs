//! Shared helpers for cargo-release-plan integration tests.
//!
//! Every integration test binary compiles this module separately, so a helper
//! that one binary does not use is dead code only from that binary's point of
//! view.
#![allow(
    dead_code,
    reason = "each test binary uses a different subset of these helpers"
)]

mod fixture;

pub(crate) use fixture::{Fixture, write_package};
