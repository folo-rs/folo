#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Maintain a long-lived history of benchmark results and analyze it for trends
//! that snapshot-only tooling cannot see.
//!
//! The tool stores one immutable result set per benchmark run — on the local
//! filesystem or, with the `azure` feature, in an Azure Blob container —
//! partitioned only by the factors that make results
//! fundamentally incomparable — project, engine system, target triple, and (for
//! hardware-dependent engines) a machine key — so that everything else stays
//! visible as a step in the timeline. The `analyze` command then reconstructs
//! per-benchmark series in git first-parent order and looks for regressions and
//! drift.
//!
//! The `run` command executes the workspace's benches once with `cargo bench`,
//! harvests the machine-readable output of every supported engine (Callgrind via
//! Gungraun, and Criterion), and stores one immutable result set per engine per
//! run through the configured storage backend. The `analyze` command reconstructs
//! each benchmark's series and reports notable changes, and the `install` command
//! writes a starter configuration file.

mod analyze;
mod bench;
mod bench_output;
mod cli;
mod commands;
mod comparability;
mod config;
mod config_writer;
mod context;
mod dispatch;
mod git;
mod git_history;
mod host;
mod machine;
mod model;
mod probe;
mod process;
mod report;
mod storage;
mod text;
mod types;
mod wiring;

pub use cli::Cli;
pub use comparability::{ComparabilityKey, EngineSystem, resolve_target_triple};
pub use config::{
    Config, ConfigError, ProjectConfig, StorageConfig, default_template, parse_config,
};
pub use context::{
    CiInfo, CiProvider, GitInfo, RunContext, Timestamps, ToolchainInfo, detect_ci,
    resolve_effective_time,
};
pub use dispatch::{run, run_with_overrides};
pub use model::{BenchmarkId, Metric, MetricKind, ResultRecord, ResultSet, SCHEMA_VERSION};
pub use storage::{LocalStorage, Storage, StorageError};
pub use types::{
    AnalyzeOptions, BackfillOptions, Command, InstallOptions, RunError, RunOptions, RunOutcome,
};
