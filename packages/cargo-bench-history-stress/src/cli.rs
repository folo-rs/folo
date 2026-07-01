//! Command-line interface for the stress harness.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};

/// Seeds a giant synthetic benchmark history into a `cargo-bench-history` storage
/// backend, then times each analysis mode (`history`, `branch`, `tip`) over it.
///
/// The dataset is fabricated, not measured: it exists purely to put the `analyze`
/// data-loading and detection path under a realistic, large-scale load so the
/// per-mode wall-clock cost can be observed against either local-filesystem or
/// Azure Blob storage.
#[derive(Debug, Parser)]
#[command(name = "cargo-bench-history-stress", version)]
pub(crate) struct Cli {
    /// Which storage backend to seed and analyze against.
    #[arg(long, value_enum, default_value_t = StorageKind::Local)]
    pub(crate) storage: StorageKind,

    /// Number of distinct benchmark cases per discriminant set.
    #[arg(long, default_value_t = 1000)]
    pub(crate) benchmarks: usize,

    /// Number of first-parent commits on the synthetic `main` history. Roughly
    /// half of them store a benchmark run; the rest are gaps, exercising the
    /// "commit with no run" path.
    #[arg(long, default_value_t = 2000)]
    pub(crate) commits: usize,

    /// Number of commits on the synthetic feature branch (drives `branch` mode).
    #[arg(long, default_value_t = 6)]
    pub(crate) branch_commits: usize,

    /// Number of dirty (uncommitted-tree) snapshots seeded on the feature tip.
    #[arg(long, default_value_t = 3)]
    pub(crate) dirty_runs: usize,

    /// Local-storage root to seed into. When omitted a temporary directory is
    /// used and removed on exit (unless `--keep`). Ignored for Azure storage.
    #[arg(long)]
    pub(crate) dir: Option<PathBuf>,

    /// Azure storage account name. Defaults to the `BENCH_HISTORY_TEST_AZURE_ACCOUNT`
    /// environment variable. Required for `--storage azure`.
    #[arg(long)]
    pub(crate) account: Option<String>,

    /// Azure blob container to create and seed into. Defaults to a unique
    /// `bh-stress-<timestamp>` name. Ignored for local storage.
    #[arg(long)]
    pub(crate) container: Option<String>,

    /// Mirror fetched objects under this directory so the measured `analyze`
    /// exercises the read-through cache. Only valid with `--storage azure` (a
    /// local backend's reads are already local). A relative path is resolved
    /// against the working directory; the directory persists across runs, so a
    /// second invocation measures a warm cache.
    #[arg(long, conflicts_with = "dir")]
    pub(crate) cache: Option<PathBuf>,

    /// Which analysis modes to measure (repeatable / comma-separated).
    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        default_value = "history,branch,tip"
    )]
    pub(crate) modes: Vec<ModeArg>,

    /// How many times to run each mode (the fastest run is reported).
    #[arg(long, default_value_t = 1)]
    pub(crate) repeat: usize,

    /// Keep the seeded data after measuring (the local directory or the Azure
    /// container) instead of cleaning it up.
    #[arg(long)]
    pub(crate) keep: bool,

    /// Emit explanatory diagnostic notes to stderr describing each decision.
    #[arg(long)]
    pub(crate) verbose: bool,

    /// Seed for the deterministic synthetic-value generator. Identical seeds and
    /// sizes produce byte-identical datasets.
    #[arg(long, default_value_t = 0x5715_5c0d_5712_e55e)]
    pub(crate) seed: u64,
}

/// Which storage backend the harness seeds and analyzes against.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum StorageKind {
    /// A local filesystem directory.
    Local,
    /// An Azure Blob Storage container.
    Azure,
}

/// A selectable analysis mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum ModeArg {
    /// Long-range base-branch trend analysis over the whole history.
    History,
    /// Feature-branch comparison of the branch tip against the base branch.
    Branch,
    /// Fast single-commit guard comparing the latest commit to the recent level.
    Tip,
}

impl ModeArg {
    /// The `--mode` keyword the analyze command expects.
    pub(crate) fn keyword(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::Branch => "branch",
            Self::Tip => "tip",
        }
    }
}
