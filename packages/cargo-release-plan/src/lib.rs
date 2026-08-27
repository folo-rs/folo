#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Classifies publishable packages against version anchors and applies plans.

pub use check::CheckFormat;
pub use cli::{Cli, EarlyExit};
pub(crate) use errors::*;
pub use run::{RunInput, RunOutcome, run};
pub(crate) use text::{quote_path, short_commit};

mod anchor;
mod apply;
mod check;
mod classify;
mod cli;
mod command;
mod diff;
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
mod text;
mod verbose;
