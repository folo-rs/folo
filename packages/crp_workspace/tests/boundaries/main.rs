//! Real workspace, Git, Cargo and filesystem boundaries.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use crp_workspace::testing as git_fixture;

mod artifact_path;
mod cache;
mod command;
mod git;
mod git_history_tests;
mod live;
mod manifest;
mod metadata;
mod metadata_dependency_tests;
mod metadata_discovery_tests;
mod metadata_installation_tests;
mod snapshot;
mod snapshot_command;

::testing::set_allocator!();

fn with_io_test(test: impl FnOnce() + Send + 'static) {
    crp_workspace::testing::with_io_slot(|| {
        // These isolated Git/process cases normally finish in seconds. The last-chance budget
        // leaves a large margin for cold or instrumented hosts, never an expected failure timeout.
        testing::with_watchdog_timeout(std::time::Duration::from_mins(5), test);
    });
}
