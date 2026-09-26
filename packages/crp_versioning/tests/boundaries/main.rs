//! Real boundaries of release assessment, preparation and application.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use crp_workspace::testing as git_fixture;

mod apply;
mod classify;
mod classify_dependency_tests;
mod classify_discovery_tests;
mod classify_installation_tests;
mod expand;
mod preview;
mod prospective;
mod resolved;

::testing::set_allocator!();
