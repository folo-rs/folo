//! Supervisor startup, client ownership, console relay, and teardown.

mod ownership;
mod record_writer;
mod relay;
mod session;
mod session_spec;
mod shared;
mod startup;

pub(crate) use session::*;
pub(crate) use session_spec::*;
#[cfg(any(test, feature = "private-test-util"))]
pub(crate) use startup::{InitGuard, Initialized, initialize};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
