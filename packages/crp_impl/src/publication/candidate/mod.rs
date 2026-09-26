//! Verifies immutable release-source candidates using the application's shared version rules.

pub use command::{capture, git};
pub use metadata::Metadata;
pub use repository::Repository;
pub use request::CandidateRequest;
pub(crate) use request::package_identifier;
pub use verify::verify;

mod command;
mod metadata;
mod repository;
mod request;
mod verification_repository;
mod verify;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod verification_tests;
