#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Classifies publishable packages against version anchors and applies plans.

#[doc(hidden)]
pub use crp_impl::{CheckFormat, RunInput, RunOutcome, run};
pub use crp_impl::{Cli, EarlyExit};
