#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! A Cargo tool to detect the package that a file belongs to, passing the package name
//! to a subcommand.
//!
//! This crate provides the core logic for package detection, exposed via the [`run`](fn@run) function.
//! The binary entry point is in `main.rs`.

mod cli;
mod detection;
mod execution;
mod pal;
mod run;
mod types;
mod workspace;

pub use cli::{Cli, EarlyExit};
pub use run::run;
pub use types::*;
