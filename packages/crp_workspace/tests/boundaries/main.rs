//! Real workspace, Git, Cargo and filesystem boundaries.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use crp_workspace::testing as git_fixture;

mod artifact_path;
mod command;
mod git;
mod git_history_tests;
mod manifest;
mod metadata;
mod metadata_dependency_tests;
mod metadata_discovery_tests;
mod metadata_installation_tests;

::testing::set_allocator!();
