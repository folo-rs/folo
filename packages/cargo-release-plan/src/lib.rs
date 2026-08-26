#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! A Cargo subcommand that classifies publishable packages against version anchors
//! and applies increment plans.

mod anchor;
mod apply;
mod check;
mod classify;
mod cli;
mod command;
mod errors;
mod git;
mod groups;
mod inherited;
mod manifest;
mod metadata;
mod packaging;
mod plan;
mod report;
mod run;
mod types;
mod verbose;

pub use cli::{Cli, EarlyExit};
pub(crate) use errors::*;
pub use run::run;
pub use types::*;
