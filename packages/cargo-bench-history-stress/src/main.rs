//! On-demand stress harness for `cargo-bench-history`'s `analyze` command.
//!
//! The harness fabricates a large synthetic benchmark history — by default a
//! thousand benchmarks across a thousand `main` commits over twelve months in
//! each of six discriminant sets ({windows, linux, macos} × {x64, arm}), plus a
//! short feature branch with dirty snapshots and a few blessings — seeds it into a
//! configured storage backend, then times each analysis mode (`history`, `branch`,
//! `tip`) over it. The dataset is invented, not measured: its only purpose is to
//! put the real `analyze` data-loading and detection path under a realistic,
//! large-scale load so the per-mode wall-clock cost can be observed against either
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
mod scenario;
mod seed;
mod target;

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use jiff::Timestamp;
use tempfile::TempDir;

use crate::cli::{Cli, StorageKind};
use crate::error::{Error, fail};
use crate::logging::Logger;
use crate::report::Phases;
use crate::scenario::Scenario;
use crate::target::StorageTarget;

/// Environment variable supplying the default Azure storage account name.
const ACCOUNT_ENV: &str = "BENCH_HISTORY_AZURE_ACCOUNT";

/// Seconds added after the newest commit to anchor the analysis clock, so no
/// seeded commit sits in the analysis's "future" relative to `now`.
const CLOCK_LEAD_SECONDS: i64 = 60 * 60;

/// The dataset anchor: a fixed instant (~2025-06-15) the history ends near, so a
/// given seed and sizes reproduce a byte-identical dataset regardless of when the
/// harness runs.
const ANCHOR_UNIX: i64 = 1_750_000_000;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Builds the dataset, seeds it, and measures each mode. Split out from `main` so
/// every failure path renders one diagnostic and yields a non-zero exit.
#[tokio::main]
async fn run() -> Result<(), Error> {
    let cli = Cli::parse();
    let logger = Logger::new(cli.verbose);

    let scenario = Scenario {
        benchmarks: cli.benchmarks,
        commits: cli.commits,
        branch_commits: cli.branch_commits,
        dirty_runs: cli.dirty_runs,
        seed: cli.seed,
    };
    if scenario.benchmarks == 0 {
        return Err(fail("--benchmarks must be at least 1"));
    }
    if scenario.commits == 0 {
        return Err(fail("--commits must be at least 1"));
    }

    let anchor = Timestamp::from_second(ANCHOR_UNIX)
        .map_err(|error| fail(format!("invalid dataset anchor: {error}")))?;
    let (main_times, feature_times) =
        scenario::commit_times(anchor, scenario.commits, scenario.branch_commits);
    let clock = analysis_clock(&main_times, &feature_times)?;
    let sets = scenario::discriminant_sets();

    let mut target = build_target(&cli)?;
    logger.step(&format!("storage backend: {}", target.label()));

    // The git repository and the workspace (config + import marks) live in
    // separate temporary directories from the storage root: the repository must
    // present a clean working tree to `analyze`, and local storage uses its own
    // absolute path, so neither writes into the other.
    let repo_dir = TempDir::new()
        .map_err(|error| fail(format!("failed to create a repo temp dir: {error}")))?;
    let workspace_dir = TempDir::new()
        .map_err(|error| fail(format!("failed to create a workspace temp dir: {error}")))?;

    let repo_started = Instant::now();
    let marks_path = workspace_dir.path().join("fast-import-marks.txt");
    let repo = repo::build_repo(
        repo_dir.path(),
        &marks_path,
        &main_times,
        &feature_times,
        logger,
    )
    .await?;
    let repo_elapsed = repo_started.elapsed();

    write_config(workspace_dir.path(), &target, logger).await?;
    target.provision(logger).await?;

    let seed_started = Instant::now();
    let stats = seed::seed(target.seed_root(), scenario, &sets, &repo, logger)?;
    let seed_elapsed = seed_started.elapsed();

    let upload_started = Instant::now();
    target.upload(logger).await?;
    let upload_elapsed = upload_started.elapsed();

    let mut results = Vec::with_capacity(cli.modes.len());
    for mode in &cli.modes {
        let result = measure::measure(
            workspace_dir.path(),
            repo_dir.path(),
            *mode,
            clock,
            cli.repeat,
            logger,
        )
        .await?;
        results.push(result);
    }

    report::print(
        &target.label(),
        scenario,
        sets.len(),
        stats,
        Phases {
            repo: repo_elapsed,
            seed: seed_elapsed,
            upload: upload_elapsed,
        },
        &results,
    );

    target.cleanup(cli.keep, logger).await?;
    Ok(())
}

/// Resolves the `now` instant the analysis is anchored to: just after the newest
/// seeded commit, so the default look-back window covers the whole history and no
/// commit is dated in the analysis's future.
fn analysis_clock(
    main_times: &[Timestamp],
    feature_times: &[Timestamp],
) -> Result<Timestamp, Error> {
    let newest = feature_times
        .last()
        .or_else(|| main_times.last())
        .map_or(ANCHOR_UNIX, |time| time.as_second());
    Timestamp::from_second(newest.saturating_add(CLOCK_LEAD_SECONDS))
        .map_err(|error| fail(format!("invalid analysis clock: {error}")))
}

/// Builds the storage target from the CLI, resolving Azure defaults.
fn build_target(cli: &Cli) -> Result<StorageTarget, Error> {
    match cli.storage {
        StorageKind::Local => StorageTarget::local(cli.dir.clone()),
        StorageKind::Azure => {
            let account = cli
                .account
                .clone()
                .or_else(|| std::env::var(ACCOUNT_ENV).ok())
                .ok_or_else(|| {
                    fail(format!(
                        "Azure storage needs an account: pass --account or set {ACCOUNT_ENV}"
                    ))
                })?;
            let container = cli.container.clone().unwrap_or_else(default_container);
            StorageTarget::azure(account, container)
        }
    }
}

/// A unique default Azure container name (`bh-stress-<unix-seconds>`), so repeated
/// runs do not collide and cleanup can delete the whole container.
fn default_container() -> String {
    format!("bh-stress-{}", Timestamp::now().as_second())
}

/// Writes the seeded configuration into the workspace's `.cargo/` directory.
async fn write_config(
    workspace: &Path,
    target: &StorageTarget,
    logger: Logger,
) -> Result<(), Error> {
    let path = measure::config_path(workspace);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| fail(format!("failed to create {}: {error}", parent.display())))?;
    }
    tokio::fs::write(&path, target.config_toml())
        .await
        .map_err(|error| fail(format!("failed to write {}: {error}", path.display())))?;
    logger.detail(&format!(
        "wrote analyze configuration to {}",
        path.display()
    ));
    Ok(())
}
