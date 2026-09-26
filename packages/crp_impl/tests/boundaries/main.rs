//! Real Git, Cargo and filesystem boundaries of `crp_impl`.

mod apply;
mod artifact_path;
mod candidate;
mod classify;
mod classify_dependency_tests;
mod classify_discovery_tests;
mod classify_installation_tests;
mod command;
mod expand;
mod git;
mod git_fixture;
mod git_history_tests;
mod http_fixture;
mod manifest;
mod metadata;
mod metadata_dependency_tests;
mod metadata_discovery_tests;
mod metadata_installation_tests;
mod native_binaries;
mod preview;
mod prospective;
mod publication;
mod publication_config;
mod publication_credentials;
mod publication_github;
mod publication_identity;
mod publication_registry;
mod resolved;

::testing::set_allocator!();
