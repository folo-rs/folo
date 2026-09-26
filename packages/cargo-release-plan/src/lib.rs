#![cfg_attr(docsrs, feature(doc_cfg))]

//! Classifies publishable packages against version anchors and applies plans.

pub use crp_impl::{CheckFormat, Cli, EarlyExit, RunInput, RunOutcome, run};

#[cfg(test)]
::testing::set_allocator!();
