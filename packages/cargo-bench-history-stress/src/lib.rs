#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! On-demand stress harness for `cargo-bench-history`'s `analyze` command.
//!
//! The harness fabricates a large synthetic benchmark history — by default a
//! thousand benchmarks across two thousand `main` commits over twelve months
//! (about half of which store a run, the rest left as realistic gaps), spread
//! across every supported engine crossed with the platforms it runs on
//! (Callgrind on Linux only; Criterion, `alloc_tracker`, and `all_the_time` on
//! all of {windows, linux, macos} × {x64, arm}), plus a short feature branch with
//! dirty snapshots and a few blessings — seeds it into a configured storage
//! backend, then times each analysis mode (`history`, `branch`, `tip`) over it.
//! The dataset is invented, not measured: its only purpose is to put the real
//! `analyze` data-loading and detection path under a realistic, large-scale load
//! so the per-mode wall-clock cost can be observed against either
//! local-filesystem or Azure Blob storage.
//!
//! Nothing here changes production code. Generation replicates only the storage
//! *write* layout (the same object keys the backends use); measurement reads the
//! data back through the real public [`run_with_overrides`] entry point, so the
//! measured path is exactly production behavior over synthetic data.
//!
//! [`run_with_overrides`]: cargo_bench_history::run_with_overrides

mod cli;
mod error;
mod logging;
mod measure;
mod repo;
mod report;
mod run;
mod scenario;
mod seed;
mod target;

pub use run::run;
