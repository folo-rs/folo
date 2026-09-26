#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, doc(hidden))]

//! Internal wiring for the `cargo-release-plan` executable and maintainer tests.
//!
//! This package does not provide a supported Rust library API. Use its command line.

pub use crp_impl::{CheckFormat, Cli, EarlyExit, RunInput, RunOutcome, run};

#[cfg(test)]
::testing::set_allocator!();
