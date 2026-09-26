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

fn with_io_test(test: impl FnOnce() + Send + 'static) {
    crp_workspace::testing::with_io_slot(|| {
        // Instrumented native toolchain startup needs a conservative last-chance budget.
        testing::with_watchdog_timeout(std::time::Duration::from_mins(5), test);
    });
}
