//! End-to-end tests against hermetic Git fixtures.
//!
//! Each test drives [`cargo_release_plan::run`] directly, except for
//! [`cli_binary`] and [`artifact_commands`], which drive the binary as a process. Git
//! configuration is pinned by [`fixture::Fixture`] so tests do not depend on
//! host or user settings. Integer literals assigned to unused locals in
//! generated Rust sources are arbitrary byte-change markers.
//!
//! The suite is split into one topic module per area of behavior over a shared
//! [`harness`]; this file is the crate root that ties the modules together.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

mod apply;
mod artifact_commands;
mod baseline;
mod captured;
mod cli_binary;
mod compatibility;
mod evidence;
mod expand;
mod fixture;
mod groups;
mod harness;
mod history;
mod inspect_plan;
mod lockfile;
mod native_binaries;
mod nesting;
mod packaging;
mod path_case;
mod preview;
mod preview_safety;
mod propose;
mod publication;
mod report;
mod status;

::testing::set_allocator!();
