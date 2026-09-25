//! Exercises process, filesystem and executable boundaries with hermetic Git repositories.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

mod command;
mod fixture;
mod metadata;
mod repository;
mod repository_fixture;
mod scheduling;
mod snapshots;

::testing::set_allocator!();
