use std::path::{Path, PathBuf};
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
use crate::{measure, repo, report, scenario, seed};

/// Environment variable supplying the default Azure storage account name.
const ACCOUNT_ENV: &str = "BENCH_HISTORY_TEST_AZURE_ACCOUNT";

/// Seconds added after the newest commit to anchor the analysis clock, so no
/// seeded commit sits in the analysis's "future" relative to `now`.
const CLOCK_LEAD_SECONDS: i64 = 60 * 60;

/// The dataset anchor: a fixed instant (~2025-06-15) the history ends near, so a
/// given seed and sizes reproduce a byte-identical dataset regardless of when the
/// harness runs.
const ANCHOR_UNIX: i64 = 1_750_000_000;

/// Runs the harness, reporting failures and returning a process exit code.
///
/// Reads the process arguments and measures the requested analysis modes.
// Only the process-argument/runtime wrapper requires a spawned binary. Its execution
// and exit mapping stay in library-tested functions; see docs/implementation.md.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
#[must_use]
#[tokio::main]
pub async fn run() -> ExitCode {
    exit_code(run_harness(Cli::parse()).await)
}

/// Reports the harness outcome without changing its success/failure status.
fn exit_code(result: Result<(), Error>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Builds the dataset, seeds it, and measures each mode. Split out from [`run`] so
/// every failure path renders one diagnostic and yields a non-zero exit.
// Successful execution drives real Git, storage and timed analysis and belongs to
// the seeded binary integration tests. Library tests exercise pre-I/O rejection.
#[cfg_attr(coverage_nightly, coverage(off))]
async fn run_harness(cli: Cli) -> Result<(), Error> {
    let logger = Logger::new(cli.verbose);
    let scenario = build_scenario(&cli)?;

    let anchor = Timestamp::from_second(ANCHOR_UNIX)
        .map_err(|error| fail(format!("invalid dataset anchor: {error}")))?;
    let (main_times, feature_times) =
        scenario::commit_times(anchor, scenario.commits, scenario.branch_commits);
    let clock = analysis_clock(&main_times, &feature_times)?;
    let sets = scenario::discriminant_sets();

    let mut target = build_target(&cli)?;
    logger.step(&format!("storage backend: {}", target.label()));

    // The read-through cache only applies to the cloud backend; a local store is
    // already on disk, so a `--cache` there would measure nothing. Reject the
    // combination rather than silently ignoring it.
    let cache = resolve_cache(&cli)?;
    if let Some(cache) = &cache {
        logger.detail_with(|| {
            format!(
                "measuring analyze against a read-through cache rooted at {}",
                cache.display()
            )
        });
    }

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

    // The target is now provisioned (for Azure, the per-run container exists), so
    // run the remaining fallible work under a guard that always cleans up
    // afterward — even if seeding, upload, or a measured mode fails — so a failed
    // run never orphans its container.
    let measured = async {
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
                measure::StorageInputs {
                    local: target.local_path(),
                    cache: cache.as_deref(),
                },
                clock,
                cli.repeat,
                logger,
            )
            .await?;
            results.push(result);
        }

        println!(
            "{}",
            report::render(
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
            )
        );
        Ok::<(), Error>(())
    }
    .await;

    let cleaned = target.cleanup(cli.keep, logger).await;

    // Surface a measured failure first (it is the root cause); a cleanup failure
    // is reported only when the run itself otherwise succeeded.
    measured?;
    cleaned?;
    Ok(())
}

/// Validates the scenario before any repository or storage resources are created.
fn build_scenario(cli: &Cli) -> Result<Scenario, Error> {
    if cli.benchmarks == 0 {
        return Err(fail("--benchmarks must be at least 1"));
    }
    if cli.commits == 0 {
        return Err(fail("--commits must be at least 1"));
    }
    Ok(Scenario {
        benchmarks: cli.benchmarks,
        commits: cli.commits,
        branch_commits: cli.branch_commits,
        dirty_runs: cli.dirty_runs,
        seed: cli.seed,
    })
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

/// Builds the storage target from the CLI, resolving Azure defaults. The IO edge
/// that selects and constructs a backend; like [`run`], it is reached only through
/// the binary, so it carries `coverage(off)`.
#[cfg_attr(coverage_nightly, coverage(off))]
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

/// Resolves the optional `--cache` directory the measured `analyze` mirrors into.
///
/// The cache is meaningful only against the cloud backend, so it is rejected with
/// `--storage local` rather than silently ignored. A relative path is made
/// absolute so it is independent of the throwaway workspace `analyze` rebases
/// against, letting the same directory back repeated (warm-cache) runs.
///
/// # Errors
///
/// Returns an error if `--cache` is combined with `--storage local`, or if the
/// path cannot be made absolute.
fn resolve_cache(cli: &Cli) -> Result<Option<PathBuf>, Error> {
    match &cli.cache {
        None => Ok(None),
        Some(_) if matches!(cli.storage, StorageKind::Local) => Err(fail(
            "--cache only applies to --storage azure: a local backend's reads are already local",
        )),
        Some(path) => Ok(Some(std::path::absolute(path).map_err(|error| {
            fail(format!(
                "failed to resolve --cache {}: {error}",
                path.display()
            ))
        })?)),
    }
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
    logger.detail_with(|| format!("wrote analyze configuration to {}", path.display()));
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fs;

    use futures::executor::block_on;

    use super::*;

    #[test]
    fn successful_execution_returns_success() {
        assert_eq!(exit_code(Ok(())), ExitCode::SUCCESS);
    }

    #[test]
    fn failed_execution_returns_failure() {
        assert_eq!(exit_code(Err(fail("execution failed"))), ExitCode::FAILURE);
    }

    #[test]
    fn scenario_preserves_requested_sizes_and_seed() {
        // Distinct values expose crossed fields; they are ordinary scenario inputs.
        let cli = Cli::parse_from([
            "cargo-bench-history-stress",
            "--benchmarks",
            "2",
            "--commits",
            "3",
            "--branch-commits",
            "4",
            "--dirty-runs",
            "5",
            "--seed",
            "6",
        ]);
        let scenario = build_scenario(&cli).unwrap();
        assert_eq!(scenario.benchmarks, cli.benchmarks);
        assert_eq!(scenario.commits, cli.commits);
        assert_eq!(scenario.branch_commits, cli.branch_commits);
        assert_eq!(scenario.dirty_runs, cli.dirty_runs);
        assert_eq!(scenario.seed, cli.seed);
    }

    #[test]
    fn scenario_allows_minimal_main_history_without_feature_or_dirty_runs() {
        let cli = Cli::parse_from([
            "cargo-bench-history-stress",
            "--benchmarks",
            "1",
            "--commits",
            "1",
            "--branch-commits",
            "0",
            "--dirty-runs",
            "0",
        ]);
        let scenario = build_scenario(&cli).unwrap();
        assert_eq!(scenario.benchmarks, 1);
        assert_eq!(scenario.commits, 1);
        assert_eq!(scenario.branch_commits, 0);
        assert_eq!(scenario.dirty_runs, 0);
    }

    #[test]
    fn scenario_rejects_zero_benchmarks() {
        let cli = Cli::parse_from(["cargo-bench-history-stress", "--benchmarks", "0"]);
        assert!(build_scenario(&cli).is_err());
    }

    #[test]
    fn scenario_rejects_zero_commits() {
        let cli = Cli::parse_from(["cargo-bench-history-stress", "--commits", "0"]);
        assert!(build_scenario(&cli).is_err());
    }

    #[test]
    fn execution_rejects_invalid_scenarios_before_io() {
        for flag in ["--benchmarks", "--commits"] {
            let cli = Cli::parse_from(["cargo-bench-history-stress", flag, "0"]);
            // No Tokio runtime is needed: validation must finish before the I/O path.
            assert!(block_on(run_harness(cli)).is_err());
        }
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "uses the real filesystem and Tokio runtime")]
    async fn config_write_creates_parents_and_replaces_existing_contents() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("workspace");
        let path = workspace.join(".cargo").join("bench_history.toml");
        let local = StorageTarget::local(None).unwrap();
        let azure = StorageTarget::azure("account".to_owned(), "container".to_owned()).unwrap();

        // Constructing an Azure target only creates staging storage, not a cloud resource.
        // Different backend configurations prove the existing file is replaced.
        for target in [local, azure] {
            write_config(&workspace, &target, Logger::new(false))
                .await
                .unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), target.config_toml());
        }
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "uses the real filesystem and Tokio runtime")]
    async fn config_write_reports_parent_creation_failure() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join(".cargo");
        fs::write(&parent, "not a directory").unwrap();
        let target = StorageTarget::local(None).unwrap();

        assert!(
            write_config(dir.path(), &target, Logger::new(false))
                .await
                .is_err()
        );
        assert_eq!(fs::read_to_string(parent).unwrap(), "not a directory");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "uses the real filesystem and Tokio runtime")]
    async fn config_write_reports_file_write_failure() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".cargo").join("bench_history.toml");
        fs::create_dir_all(&path).unwrap();
        let target = StorageTarget::local(None).unwrap();

        assert!(
            write_config(dir.path(), &target, Logger::new(false))
                .await
                .is_err()
        );
        assert!(path.is_dir());
    }

    #[test]
    fn clock_lead_is_one_hour() {
        assert_eq!(CLOCK_LEAD_SECONDS, 3_600);
    }

    #[test]
    fn analysis_clock_anchors_just_after_the_newest_commit() {
        let main = vec![
            Timestamp::from_second(1_000).expect("in range"),
            Timestamp::from_second(2_000).expect("in range"),
        ];
        let feature = vec![Timestamp::from_second(3_000).expect("in range")];

        // A feature branch is the newest history, so the clock sits one hour past
        // its tip.
        let clock = analysis_clock(&main, &feature).expect("clock");
        assert_eq!(clock.as_second(), 3_000 + 3_600);

        // Without a feature branch it anchors past the newest main commit instead.
        let clock = analysis_clock(&main, &[]).expect("clock");
        assert_eq!(clock.as_second(), 2_000 + 3_600);

        // With no commits at all it falls back to the dataset anchor.
        let clock = analysis_clock(&[], &[]).expect("clock");
        assert_eq!(clock.as_second(), ANCHOR_UNIX + 3_600);
    }

    #[test]
    fn analysis_clock_errors_when_the_lead_overflows_the_supported_range() {
        // A commit dated at the very end of the representable timestamp range leaves
        // no room for the one-hour analysis lead, so anchoring the clock fails loudly
        // rather than wrapping into an invalid instant.
        let error = analysis_clock(&[], &[Timestamp::MAX])
            .expect_err("the lead past the maximum timestamp cannot be represented");
        assert!(
            error.to_string().contains("invalid analysis clock"),
            "{error}"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn default_container_is_prefixed_and_timestamped() {
        let name = default_container();
        let suffix = name
            .strip_prefix("bh-stress-")
            .expect("the default container name carries the bh-stress- prefix");
        assert!(
            suffix.parse::<i64>().is_ok(),
            "the container suffix should be a unix second: {name}"
        );
    }

    #[test]
    fn resolve_cache_is_none_without_a_cache_flag() {
        // No --cache means no mirror, so the measured analyze reads the cloud
        // directly (the harness's default, uncached measurement).
        let cli = Cli::parse_from(["cargo-bench-history-stress", "--storage", "azure"]);
        assert!(
            resolve_cache(&cli)
                .expect("no cache is always valid")
                .is_none()
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "std::path::absolute reads the process working directory"
    )]
    fn resolve_cache_makes_an_azure_cache_path_absolute() {
        // A cache directory against the cloud backend resolves to an absolute path
        // so the mirror location is stable regardless of the process working
        // directory when the measured analyze later runs.
        let cli = Cli::parse_from([
            "cargo-bench-history-stress",
            "--storage",
            "azure",
            "--cache",
            "cache-dir",
        ]);
        let resolved = resolve_cache(&cli)
            .expect("azure + cache is valid")
            .expect("a cache path is returned");
        assert!(resolved.is_absolute(), "{}", resolved.display());
        assert!(resolved.ends_with("cache-dir"), "{}", resolved.display());
    }

    #[test]
    fn resolve_cache_rejects_a_cache_against_local_storage() {
        // The cache only helps the cloud backend, so pairing it with the default
        // local storage is a usage error rather than a silent no-op.
        let cli = Cli::parse_from(["cargo-bench-history-stress", "--cache", "cache-dir"]);
        let error = resolve_cache(&cli).expect_err("a cache against local must fail");
        assert!(
            error
                .to_string()
                .contains("--cache only applies to --storage azure"),
            "{error}"
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "std::path::absolute reads the process working directory"
    )]
    fn resolve_cache_reports_a_path_that_cannot_be_made_absolute() {
        // An empty cache path has no absolute form, so the harness surfaces the
        // resolution failure instead of panicking on the infallible-looking call.
        // The empty path is set directly because clap rejects an empty --cache value.
        let mut cli = Cli::parse_from(["cargo-bench-history-stress", "--storage", "azure"]);
        cli.cache = Some(PathBuf::new());
        let error = resolve_cache(&cli).expect_err("an empty cache path cannot be made absolute");
        assert!(
            error.to_string().contains("failed to resolve --cache"),
            "{error}"
        );
    }
}
