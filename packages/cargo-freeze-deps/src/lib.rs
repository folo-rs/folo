#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! A Cargo subcommand that freezes every floating dependency version in a Cargo.toml file
//! to its literal `=X.Y.Z` form.

mod freeze;
mod run;
mod types;
mod version;

pub use run::run;
pub use types::*;
