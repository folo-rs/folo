//! Candidate verification's real Git, Cargo and filesystem boundaries.

#![cfg_attr(coverage_nightly, coverage(off))]

mod command;
mod fixture;
mod metadata;
mod repository;
mod repository_fixture;
mod scheduling;
mod snapshots;
