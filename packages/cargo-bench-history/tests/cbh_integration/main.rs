//! Integration tests exercising the public command surface end to end: parse
//! arguments through `clap`, translate to the typed command, and dispatch through
//! `run` against the real process, filesystem-harvest, and local-storage adapters.
//!
//! The suite is split into one topic module per command area over a shared
//! [`harness`] of scaffolding (the test workspace, seeding helpers, and result-set
//! builders); this file is the crate root that ties the modules together.
#![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]
#![allow(
    clippy::float_cmp,
    reason = "metric values are exact integer-derived counts"
)]

mod analyze;
mod backfill;
mod bless;
mod cli;
mod collect;
mod harness;
mod install;
mod list;
mod prune;
mod storage;
