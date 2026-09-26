//! Candidate, registry, credentials and delivery integration boundaries.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use crp_workspace::testing as git_fixture;

mod candidate;
mod http_fixture;
mod native_binaries;
mod publication;
mod publication_config;
mod publication_credentials;
mod publication_github;
mod publication_identity;
mod publication_registry;

::testing::set_allocator!();
