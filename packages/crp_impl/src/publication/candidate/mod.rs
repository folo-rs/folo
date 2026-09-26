//! Verifies immutable release-source candidates using the application's shared version rules.

pub(crate) use cli::{Cli as CandidateRequest, package_identifier};
pub use command::{capture, git};
pub use metadata::Metadata;
pub use repository::Repository;
pub use run::run;
pub(crate) use verify::verify;

mod cli;
mod command;
mod metadata;
mod repository;
mod run;
mod verification_repository;
mod verify;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod verification_tests;
