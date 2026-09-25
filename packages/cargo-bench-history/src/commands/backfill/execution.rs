//! The `backfill` command: replay `collect` across a range of historical commits.
//!
//! Backfilling bootstraps a history for a repository that adopted the tool late,
//! fills gaps left by a heterogeneous CI machine pool, and supports ad-hoc "what
//! did this look like N commits ago" investigations. It checks out each commit of
//! a range in a dedicated git **worktree** (never the primary checkout) and runs
//! the configured engines there exactly as the `collect` command does, except that
//! the worktree's own `rust-toolchain.toml` governs the build rather than the
//! toolchain this tool was launched with. A backfilled run carries no commit
//! timestamp of its own; its position on the timeline is where its commit sits in
//! git history, resolved live at analyze time (see the `backfill` command in
//! `DESIGN.md`).
//!
//! The range is walked **newest commit first**. The newest gaps are the ones
//! current comparisons draw on, so bounded passes prioritize those commits.
//! `--max-commits` limits replay attempts after the skip pre-check, completing each
//! attempted commit before stopping normally. The endpoints remain independent of
//! that limit: `--from` names the oldest commit and `--to` the newest, both inclusive.
//!
//! Like `collect`, the orchestration is generic over small ports so the loop logic is
//! exercised with in-memory fakes (Miri-safe): a [`BackfillGit`] port for the git
//! topology and worktree lifecycle, and a [`CommitRunner`] port that runs and
//! stores one commit. The production [`execute`] wires the real adapters; the real
//! [`CommitRunner`] reuses the `collect` pipeline ([`run_engines`]) against a
//! probe, engine runner, and output source rooted at the selected project within
//! each worktree.
//!
//! Before any commit is benchmarked, the commits that already have a stored
//! (clean) result **in the partition this run would write to** are listed once from
//! storage. In the default skip-existing mode a commit already present is skipped
//! outright, so its (expensive) benchmark execution never runs; this makes a
//! backfill resumable and cheap to re-run. A commit with a clean result for only
//! some engines is still skipped — use `--overwrite` to re-benchmark every commit
//! (for example after adding a new bench), which replaces results in place rather
//! than colliding with them.

use std::collections::HashSet;
use std::future::Future;
use std::io;
use std::num::NonZeroUsize;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use cbh_config::{
    load_config, resolve_config_path, resolve_local_path, resolve_project_id, resolve_repo,
    storage_env,
};
use cbh_diag::{Reporter, ReporterExt, StderrReporter, count_noun};
use cbh_engines::FsBenchOutputSource;
use cbh_git::{GitHistory, SystemGitHistory, TokioBenchRunner, capture};
use cbh_probe::SystemProbe;
use cbh_storage::{Storage, build_storage};
use ohno::AppError;
use tick::Clock;

use crate::commands::collect::{
    CollectDeps, CollectSummary, Partition, default_bench_command, partition_selection_summary,
    probe_partition, run_engines,
};
use crate::errors::{
    AddWorktreeFailedError, BenchFailure, FirstParentWalkFailedError, MissingProjectDirectoryError,
    RemoveWorktreeFailedError, ResetWorktreeFailedError, ResolveRefFailedError,
};
use crate::model::{Engine, StorageKey, parse_key};
use crate::{
    BackfillError, BackfillOptions, CollectOptions, DuplicateResultError, RunOutcome,
    finish_with_flush,
};

/// Read access to a repository's commit topology plus the worktree lifecycle a
/// backfill needs to check out each commit in isolation.
pub(crate) trait BackfillGit {
    /// Resolves a ref (branch, tag, `HEAD`, or commit ID) to its full commit ID, or
    /// `Ok(None)` when it does not resolve.
    fn resolve(&self, reference: &str) -> impl Future<Output = io::Result<Option<String>>>;

    /// The first-parent ancestry of `reference`, **oldest commit first**.
    fn first_parent(&self, reference: &str) -> impl Future<Output = io::Result<Vec<String>>>;

    /// Adds a detached worktree at `path` checked out to `commit`.
    fn add_worktree(&self, path: &Path, commit: &str) -> impl Future<Output = io::Result<()>>;

    /// Resets the worktree at `path` to `commit`: a forced detached checkout,
    /// `reset --hard`, and `clean -fd` (ignored build artifacts are preserved for
    /// incremental speed).
    fn reset_to(&self, path: &Path, commit: &str) -> impl Future<Output = io::Result<()>>;

    /// Removes the worktree at `path`.
    fn remove_worktree(&self, path: &Path) -> impl Future<Output = io::Result<()>>;
}

/// Runs and stores the configured engines for one already-checked-out commit.
pub(crate) trait CommitRunner {
    /// The set of commits (by full commit ID) that already have a stored clean
    /// result **in the partition this backfill writes to** — this host's target
    /// triple and auto-detected machine key, across every engine. A commit in that
    /// set has already been backfilled here, so the
    /// default skip-existing mode skips it without benchmarking. Probed once per
    /// backfill.
    ///
    /// Other target triples and machine keys are independent data sets: a commit
    /// measured on another platform or another machine pool leaves this
    /// partition's gap open, so it must not count as recorded. Engines, by
    /// contrast, are unioned — no rule says which engines a run produces (Callgrind
    /// records nothing off Linux), so requiring all of them would never skip
    /// anything.
    ///
    /// Implementations announce the partition they scanned before benchmarking.
    /// This makes the skip decision's inputs visible without verbose logging or
    /// waiting for the final summary.
    fn recorded_commits(&self) -> impl Future<Output = Result<HashSet<String>, AppError>>;

    /// Runs the engines in `worktree` and reports the outcome. Recoverable
    /// build/bench failures are reported as [`CommitOutcome::BenchFailed`];
    /// infrastructure failures (storage, git, I/O, configuration) propagate as
    /// `Err` so the backfill aborts regardless of `--ignore-errors`.
    fn run(
        &self,
        worktree: &Path,
        commit: &str,
    ) -> impl Future<Output = Result<CommitOutcome, AppError>>;
}

/// What happened when a single commit was processed.
#[derive(Debug)]
pub(crate) enum CommitOutcome {
    /// Results were stored; `cases` benchmark cases were harvested.
    Stored {
        /// Number of harvested benchmark cases.
        cases: usize,
    },
    /// A result was already stored for this commit (write-once collision); the
    /// commit was left as-is, which makes backfill resumable.
    SkippedExisting,
    /// The engines ran but harvested no benchmark cases, so nothing was stored.
    SkippedEmpty,
    /// The commit failed to build or benchmark (a recoverable, per-commit error).
    BenchFailed(AppError),
}

/// The real `backfill`: load configuration, wire the production adapters, and
/// orchestrate the range.
pub(crate) async fn execute(
    options: &BackfillOptions,
    workspace_dir: &Path,
    bench_command: Option<Vec<String>>,
) -> Result<RunOutcome, AppError> {
    // `--repo` selects the repository to backfill (where git history is read and
    // worktrees are created), relative to the ambient base; it defaults to the
    // base directory itself.
    let base = resolve_repo(workspace_dir, options.repo.as_deref());
    let base = base.as_path();

    let config_path = resolve_config_path(base, options.config_path.as_deref());
    let config = load_config(&config_path, options.config_path.is_some()).await?;

    let project_id = resolve_project_id(&config, base);
    let local = resolve_local_path(options.local.as_ref(), storage_env().as_deref())?;
    let storage = build_storage(local.as_deref(), &config, base, None)?;
    let bench_command = bench_command.unwrap_or_else(default_bench_command);

    let git = SystemBackfillGit::new(base);
    let project_relative_dir = git.project_relative_dir().await?;
    let worktree = worktree_path();
    let reporter = StderrReporter::new(options.verbose);
    let runner = SystemCommitRunner {
        project_id: &project_id,
        storage: &storage,
        tool_version: env!("CARGO_PKG_VERSION"),
        options,
        bench_command: &bench_command,
        worktree: &worktree,
        project_relative_dir: &project_relative_dir,
        reporter: &reporter,
    };

    let result = execute_backfill(options, &git, &runner, &worktree, &reporter).await;
    // Flush the cache-invalidation marker once for the whole range: where a
    // `backfill --overwrite` replaces an already-stored object it arms the shared
    // backend, and a single coalesced bump invalidates other machines' caches.
    // Filling a gap with a brand-new object is additive and never arms it, so an
    // append-only backfill is a cheap no-op.
    let flush = storage
        .flush_pending_invalidation(&project_id, &reporter)
        .await;
    finish_with_flush(result, flush)
}

/// Plans and runs the backfill against injected ports.
///
/// Validation precedes any worktree work, so a precondition failure leaves the
/// repository untouched. The worktree is always torn down — on success and on
/// failure — and a stop after a per-commit failure surfaces as
/// a [`BackfillError`] (a non-zero exit) carrying the partial summary.
pub(crate) async fn execute_backfill<G, C>(
    options: &BackfillOptions,
    git: &G,
    runner: &C,
    worktree: &Path,
    reporter: &dyn Reporter,
) -> Result<RunOutcome, AppError>
where
    G: BackfillGit,
    C: CommitRunner,
{
    let commits = plan_commits(options, git).await?;
    // Seeding the worktree at the first commit that will be processed saves it one
    // checkout; `run_commits` resets the worktree for every commit it runs anyway,
    // so this is an optimization rather than a precondition.
    let first = commits
        .first()
        .expect("the planned range is inclusive of both endpoints, so it is never empty");

    git.add_worktree(worktree, first)
        .await
        .map_err(|error| AddWorktreeFailedError::caused_by(worktree, first, error))?;
    let result = run_commits(options, git, runner, worktree, &commits, reporter).await;
    let teardown = git.remove_worktree(worktree).await;

    let mut report = result?;
    teardown.map_err(|error| RemoveWorktreeFailedError::caused_by(worktree, error))?;

    let message = report.render(commits.len());
    if let Some(error) = report.take_stopping_error() {
        Err(BackfillError::caused_by(message, error).into())
    } else {
        Ok(RunOutcome::Completed { message })
    }
}

/// Validates the request and resolves the inclusive commit range, **newest commit
/// first**.
///
/// Requires both endpoints to resolve and requires `--from` to be a first-parent
/// ancestor of `--to`. The range is derived purely from `--to`'s first-parent
/// history, so backfilling does not depend on the current checkout.
///
/// `--from` names the oldest commit of the range and `--to` the newest; the
/// returned order is the reverse, so a run cut short has filled the most recent
/// gaps.
pub(crate) async fn plan_commits<G: BackfillGit>(
    options: &BackfillOptions,
    git: &G,
) -> Result<Vec<String>, AppError> {
    let from = resolve_required(git, &options.from, "--from").await?;
    let to = resolve_required(git, &options.to, "--to").await?;

    let mut ancestry = git
        .first_parent(&to)
        .await
        .map_err(|error| FirstParentWalkFailedError::caused_by(&to, error))?;
    let start = ancestry
        .iter()
        .position(|commit| commit == &from)
        .ok_or_else(|| {
            BackfillError::new(format!(
                "--from ({}) is not a first-parent ancestor of --to ({})",
                options.from, options.to
            ))
        })?;

    // The ancestry arrives oldest-first, which is what locating `--from` above
    // needs; the reversal therefore happens only once that endpoint has been found.
    let mut range = ancestry.split_off(start);
    range.reverse();
    Ok(range)
}

/// Resolves `reference` to a commit ID, mapping an absent ref to a clear error.
async fn resolve_required<G: BackfillGit>(
    git: &G,
    reference: &str,
    flag: &str,
) -> Result<String, AppError> {
    git.resolve(reference)
        .await
        .map_err(|error| ResolveRefFailedError::caused_by(reference, error))?
        .ok_or_else(|| {
            BackfillError::new(format!("cannot resolve {flag} ({reference}) to a commit")).into()
        })
}

/// Runs each commit of the range in the worktree, aggregating a [`BackfillReport`].
///
/// `commits` arrives newest-first, so the most recent gaps are filled first.
/// The optional attempt limit applies after the skip pre-check and before resetting
/// the next commit. A per-commit build/bench failure stops the loop unless
/// `--ignore-errors` is set — which, in this order, means the run stops at the
/// *newest* failing commit and leaves the older ones untouched; an infrastructure
/// error always aborts (propagated as `Err`).
///
/// The pre-check and attempt budget are announced before any expensive replay, so
/// operators can distinguish a bounded pass from a range with nothing to measure.
pub(crate) async fn run_commits<G, C>(
    options: &BackfillOptions,
    git: &G,
    runner: &C,
    worktree: &Path,
    commits: &[String],
    reporter: &dyn Reporter,
) -> Result<BackfillReport, AppError>
where
    G: BackfillGit,
    C: CommitRunner,
{
    let mut report = BackfillReport::default();
    // In the default skip-existing mode, list the already-recorded commits once so
    // commits that were backfilled before are skipped without being benchmarked
    // again. `--overwrite` re-benchmarks every commit, so the list is not needed.
    let recorded = if options.overwrite {
        HashSet::new()
    } else {
        runner.recorded_commits().await?
    };
    // Classify the entire range before limiting attempts: an older recorded commit
    // is already covered, not deferred work, even if the loop stops before it.
    let mut pending = Vec::new();
    for commit in commits {
        if recorded.contains(commit) {
            reporter.note_with(|| {
                format!(
                    "skipping {}: a clean result for it is already stored in this partition",
                    short(commit)
                )
            });
            report.skipped_existing.push(commit.clone());
        } else {
            pending.push(commit);
        }
    }
    report.deferred = pending.len();
    reporter.announce(&scan_outcome_summary(
        commits.len(),
        report.skipped_existing.len(),
        options.overwrite,
        options.max_commits,
    ));

    let attempts = options.max_commits.map_or(pending.len(), NonZeroUsize::get);
    for commit in pending.into_iter().take(attempts) {
        git.reset_to(worktree, commit)
            .await
            .map_err(|error| ResetWorktreeFailedError::caused_by(worktree, commit, error))?;
        report.deferred = report
            .deferred
            .checked_sub(1)
            .expect("each attempt consumes one distinct pending commit");
        match runner.run(worktree, commit).await? {
            CommitOutcome::Stored { cases } => report.stored.push((commit.clone(), cases)),
            CommitOutcome::SkippedExisting => report.skipped_existing.push(commit.clone()),
            CommitOutcome::SkippedEmpty => report.skipped_empty.push(commit.clone()),
            CommitOutcome::BenchFailed(error) => {
                let failure_index = report.failures.len();
                report.failures.push(FailedCommit {
                    commit: commit.clone(),
                    error,
                });
                if !options.ignore_errors {
                    report.stopped_failure = Some(failure_index);
                    break;
                }
            }
        }
    }
    Ok(report)
}

/// Builds the always-on line stating what the skip pre-check decided for a range
/// of `total` commits, `already_recorded` of which the pre-check found stored in
/// this partition.
///
/// It names the rule that produced the split and the flag that changes it, so a
/// run whose measured count is surprising can be diagnosed from this one line,
/// including the optional bound on replay attempts.
///
/// A pure formatter so the wording is unit-tested without a store.
pub(crate) fn scan_outcome_summary(
    total: usize,
    already_recorded: usize,
    overwrite: bool,
    max_commits: Option<NonZeroUsize>,
) -> String {
    let range = format!("backfilling {}, newest first", count_noun(total, "commit"));
    let pending = total.saturating_sub(already_recorded);
    let summary = if overwrite {
        format!(
            "{range}: --overwrite disables the skip pre-check, so every commit is \
             eligible for re-measurement and replacement"
        )
    } else {
        format!(
            "{range}: {already_recorded} already recorded in this partition and skipped \
             without benchmarking (pass --overwrite to re-measure them), {pending} pending"
        )
    };
    if let Some(limit) = max_commits {
        format!(
            "{summary}; --max-commits {limit} allows at most {} this invocation",
            count_noun(pending.min(limit.get()), "replay attempt"),
        )
    } else {
        summary
    }
}

/// The per-commit outcomes a backfill accumulated, rendered into a summary.
#[derive(Debug, Default)]
pub(crate) struct BackfillReport {
    /// Commits whose results were stored, with the harvested case count.
    pub(crate) stored: Vec<(String, usize)>,
    /// Commits skipped because a result already existed.
    pub(crate) skipped_existing: Vec<String>,
    /// Commits skipped because they harvested no cases.
    pub(crate) skipped_empty: Vec<String>,
    /// Commits that failed to build or benchmark.
    pub(crate) failures: Vec<FailedCommit>,
    /// The entry in `failures` that stopped the run without `--ignore-errors`.
    pub(crate) stopped_failure: Option<usize>,
    /// Eligible commits left unattempted by the limit or a stopping benchmark failure.
    pub(crate) deferred: usize,
}

/// A commit and its typed benchmark failure.
///
/// The error remains intact until summary rendering and, when it stopped the run,
/// is then moved into the returned [`BackfillError`] source chain.
#[derive(Debug)]
pub(crate) struct FailedCommit {
    /// The commit that could not be benchmarked.
    pub(crate) commit: String,
    /// The recoverable benchmark failure.
    pub(crate) error: AppError,
}

impl BackfillReport {
    /// Renders the multi-line summary for a range of `total` commits.
    pub(crate) fn render(&self, total: usize) -> String {
        let reason = if self.stopped_failure.is_some() {
            "Stopped on benchmark failure."
        } else if self.deferred > 0 {
            "Commit limit reached."
        } else {
            "Range exhausted."
        };
        let mut lines = vec![format!(
            "Backfill range of {}: {} stored, {} skipped (existing), \
             {} skipped (empty), {} failed, {} deferred. {reason}",
            count_noun(total, "commit"),
            self.stored.len(),
            self.skipped_existing.len(),
            self.skipped_empty.len(),
            self.failures.len(),
            self.deferred,
        )];
        for (commit, cases) in &self.stored {
            lines.push(format!(
                "  stored {} ({})",
                short(commit),
                count_noun(*cases, "case")
            ));
        }
        for commit in &self.skipped_existing {
            lines.push(format!("  skipped {} (already stored)", short(commit)));
        }
        for commit in &self.skipped_empty {
            lines.push(format!("  skipped {} (no benchmark cases)", short(commit)));
        }
        for failure in &self.failures {
            let reason = BenchFailure::find(&failure.error)
                .expect("failures contains only errors classified by BenchFailure::find")
                .render();
            lines.push(format!("  failed {} ({reason})", short(&failure.commit)));
        }
        if let Some(failure_index) = self.stopped_failure {
            let failure = self
                .failures
                .get(failure_index)
                .expect("stopped_failure is assigned only after inserting that failure");
            lines.push(format!(
                "  stopped at {} (pass --ignore-errors to continue past failures)",
                short(&failure.commit)
            ));
        }
        lines.join("\n")
    }

    /// Takes the failure that stopped the run for the returned error chain.
    pub(crate) fn take_stopping_error(&mut self) -> Option<AppError> {
        let failure_index = self.stopped_failure.take()?;
        Some(self.failures.remove(failure_index).error)
    }
}

/// Abbreviates a commit ID for display, falling back to the full value.
fn short(commit_id: &str) -> &str {
    commit_id.get(..12).unwrap_or(commit_id)
}

/// A unique scratch path for the backfill worktree, under the system temp dir.
pub(crate) fn worktree_path() -> PathBuf {
    /// Distinguishes worktree paths created within the same process at the same
    /// clock tick. The wall clock alone is not enough: several backfills (or
    /// parallel tests) in one process can request a worktree on the same coarse
    /// timestamp and would otherwise collide on the path.
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "cargo-bench-history-worktree-{}-{nanos}-{unique}",
        std::process::id()
    ))
}

/// Parses Git's project prefix without allowing a join to escape the worktree.
pub(crate) fn parse_project_prefix(output: &str) -> Result<PathBuf, AppError> {
    // Only remove the command's line ending; whitespace can belong to directory names.
    let prefix = output
        .strip_suffix("\r\n")
        .or_else(|| output.strip_suffix('\n'))
        .unwrap_or(output);
    let relative = Path::new(prefix);
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(BackfillError::new(format!(
            "git returned a project directory that is not relative to the repository: {prefix:?}"
        ))
        .into());
    }
    Ok(relative.to_path_buf())
}

/// Maps a per-commit `collect` result to a [`CommitOutcome`].
///
/// A stored set (or several) is success; a duplicate is a resumable skip; an
/// empty harvest is a non-fatal skip; a build/bench failure is recoverable;
/// everything else (storage, configuration, I/O) is infrastructure and aborts.
pub(crate) fn map_collect_result(
    result: Result<CollectSummary, AppError>,
) -> Result<CommitOutcome, AppError> {
    let error = match result {
        Ok(summary) if summary.stored > 0 => {
            return Ok(CommitOutcome::Stored {
                cases: summary.harvested,
            });
        }
        Ok(_) => return Ok(CommitOutcome::SkippedEmpty),
        Err(error) => error,
    };

    if error.find_source::<DuplicateResultError>().is_some() {
        return Ok(CommitOutcome::SkippedExisting);
    }
    if BenchFailure::find(&error).is_some() {
        Ok(CommitOutcome::BenchFailed(error))
    } else {
        Err(error)
    }
}

/// Separates historical project absence from failures to inspect the worktree.
pub(crate) fn check_project_directory(
    path: &Path,
    directory: io::Result<bool>,
) -> Result<(), AppError> {
    match directory {
        Ok(true) => Ok(()),
        Ok(false) => Err(MissingProjectDirectoryError::new(path).into()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Err(MissingProjectDirectoryError::new(path).into())
        }
        Err(error) => Err(BackfillError::caused_by(
            format!(
                "failed to inspect historical project directory {}",
                path.display()
            ),
            error,
        )
        .into()),
    }
}

/// The commits that already have a stored clean result in `partition`.
///
/// Scans one narrow listing per engine — the engine is the outermost discriminant
/// segment, so a machine's partitions do not form a single prefix — and unions the
/// results, because no rule says which engines a run must produce (Callgrind
/// records nothing off Linux, so an intersection would never be satisfied there).
///
/// A listing matches a plain string prefix, so every key it returns is re-parsed
/// rather than sliced by hand: only a key that decomposes into a clean object of
/// this partition contributes its commit. Dirty snapshots and blessing sidecars are
/// not backfilled results and are ignored.
pub(crate) async fn recorded_commits_in<S: Storage>(
    storage: &S,
    project_id: &str,
    partition: &Partition,
    reporter: &dyn Reporter,
) -> Result<HashSet<String>, AppError> {
    let mut recorded = HashSet::new();
    for engine in Engine::ALL {
        let prefix = partition
            .discriminant_set(engine)
            .partition_prefix(project_id);
        let keys = storage.list(&prefix).await?;
        let before = recorded.len();
        let clean_commits: Vec<String> = keys
            .iter()
            .filter_map(|key| parse_key(key))
            .filter(StorageKey::is_clean)
            .map(|parsed| parsed.commit)
            .collect();
        let clean = clean_commits.len();
        recorded.extend(clean_commits);
        reporter.note_with(|| {
            format!(
                "listed {prefix}: {} objects, {clean} of them clean results, \
                 adding {} commit(s) not already contributed by another engine",
                keys.len(),
                recorded.len().saturating_sub(before)
            )
        });
    }
    reporter.note_with(|| {
        format!(
            "{} commit(s) are recorded in this partition across all engines. The engines \
             are unioned rather than intersected because no rule says which engines a run \
             produces (Callgrind records nothing off Linux, so an intersection would never \
             be satisfied there). Only clean results count: a dirty snapshot measures an \
             uncommitted working tree and a blessing is an annotation, so neither fills a \
             gap. No other target triple or machine key is listed, because those are \
             independent data sets whose results say nothing about this one",
            recorded.len()
        )
    });
    Ok(recorded)
}

/// The real [`BackfillGit`], shelling out to `git` in a fixed repository.
struct SystemBackfillGit {
    /// The primary repository working directory.
    repo: PathBuf,
    /// Read-topology delegate reused for `resolve` and `first_parent`.
    history: SystemGitHistory,
}

impl SystemBackfillGit {
    /// Binds a backfill git port to the repository rooted at `repo`.
    fn new(repo: impl Into<PathBuf>) -> Self {
        let repo = repo.into();
        let history = SystemGitHistory::new(&repo);
        Self { repo, history }
    }

    /// Resolves the selected directory relative to its repository's root.
    #[cfg_attr(test, mutants::skip)] // Real git IO; prefix validation is tested separately.
    async fn project_relative_dir(&self) -> Result<PathBuf, AppError> {
        // Let Git resolve its own root and path spelling instead of comparing
        // filesystem paths using assumptions about case or symlink resolution.
        let repo = self.repo.to_string_lossy();
        let output = capture("git", &["-C", &repo, "rev-parse", "--show-prefix"])
            .await
            .map_err(|error| {
                BackfillError::caused_by("failed to resolve the selected project directory", error)
            })?;
        if !output.status.success() {
            return Err(BackfillError::new(format!(
                "failed to resolve the selected project directory in {}",
                self.repo.display()
            ))
            .into());
        }
        parse_project_prefix(&output.stdout)
    }

    /// Runs `git -C <dir> <args>`, erroring on a non-zero exit.
    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; environment IO with no pure logic to assert.
    async fn git_in(&self, dir: &Path, args: &[&str]) -> io::Result<()> {
        let dir = dir.to_string_lossy().into_owned();
        let mut full: Vec<&str> = vec!["-C", dir.as_str()];
        full.extend_from_slice(args);
        let output = capture("git", &full).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("git {args:?} failed in {dir}")))
        }
    }
}

impl BackfillGit for SystemBackfillGit {
    #[cfg_attr(test, mutants::skip)] // Delegates to the git-shelling history port; no pure logic to assert.
    async fn resolve(&self, reference: &str) -> io::Result<Option<String>> {
        self.history.resolve(reference).await
    }

    #[cfg_attr(test, mutants::skip)] // Delegates to the git-shelling history port; no pure logic to assert.
    async fn first_parent(&self, reference: &str) -> io::Result<Vec<String>> {
        // Backfill needs only the commit IDs, not their committer timestamps.
        let commits = self.history.first_parent(reference).await?;
        Ok(commits.into_iter().map(|commit| commit.commit_id).collect())
    }

    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; environment IO with no pure logic to assert.
    async fn add_worktree(&self, path: &Path, commit: &str) -> io::Result<()> {
        let repo = self.repo.to_string_lossy().into_owned();
        let path = path.to_string_lossy().into_owned();
        let output = capture(
            "git",
            &[
                "-C",
                repo.as_str(),
                "worktree",
                "add",
                "--detach",
                "--force",
                path.as_str(),
                commit,
            ],
        )
        .await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "git worktree add failed for {commit}"
            )))
        }
    }

    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; environment IO with no pure logic to assert.
    async fn reset_to(&self, path: &Path, commit: &str) -> io::Result<()> {
        self.git_in(path, &["checkout", "--detach", "--force", commit])
            .await?;
        self.git_in(path, &["reset", "--hard"]).await?;
        self.git_in(path, &["clean", "-fd"]).await?;
        Ok(())
    }

    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; environment IO with no pure logic to assert.
    async fn remove_worktree(&self, path: &Path) -> io::Result<()> {
        let repo = self.repo.to_string_lossy().into_owned();
        let path = path.to_string_lossy().into_owned();
        let output = capture(
            "git",
            &[
                "-C",
                repo.as_str(),
                "worktree",
                "remove",
                "--force",
                path.as_str(),
            ],
        )
        .await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(io::Error::other("git worktree remove failed"))
        }
    }
}

/// The real [`CommitRunner`], wiring the `collect` pipeline against a worktree.
struct SystemCommitRunner<'a, S> {
    /// Resolved project identity for the storage partition.
    project_id: &'a str,
    /// The configured storage backend.
    storage: &'a S,
    /// Version of this tool, recorded with each run.
    tool_version: &'a str,
    /// The backfill options whose scope and overwrite policy are reused.
    options: &'a BackfillOptions,
    /// The benchmark command (`cargo bench` in production) run in each worktree.
    bench_command: &'a [String],
    /// The worktree every commit is checked out into. The pre-check probes it for
    /// the partition, so it reads the same partition each commit then writes to.
    worktree: &'a Path,
    /// The selected project directory relative to the Git root, reused in every checkout.
    project_relative_dir: &'a Path,
    /// Diagnostic sink for the pre-check, and for each per-commit `collect`.
    reporter: &'a dyn Reporter,
}

impl<S: Storage> CommitRunner for SystemCommitRunner<'_, S> {
    #[cfg_attr(test, mutants::skip)] // Probes the real host; the scan is tested separately.
    async fn recorded_commits(&self) -> Result<HashSet<String>, AppError> {
        // The partition comes from the same helper (and the same worktree probe)
        // the store path uses, so the commits treated as already recorded are
        // exactly the ones that would collide were they benchmarked again.
        let project_dir = self.worktree.join(self.project_relative_dir);
        let probe = SystemProbe::in_worktree(project_dir);
        let env = |name: &str| std::env::var(name).ok();
        let partition = probe_partition(&probe, &env).await?;

        // Always-on so the target and machine behind the skip pre-check are
        // visible before expensive replay begins, including without --verbose.
        self.reporter.announce(&partition_selection_summary(
            "scanning for already-backfilled commits",
            partition.target_triple.as_str(),
            "toolchain host of the newest checkout in the range",
            partition.machine_key.as_str(),
        ));
        recorded_commits_in(self.storage, self.project_id, &partition, self.reporter).await
    }

    #[cfg_attr(test, mutants::skip)] // Wires real adapters; the result mapping is tested via `map_collect_result`.
    async fn run(&self, worktree: &Path, _commit: &str) -> Result<CommitOutcome, AppError> {
        // A historical checkout is built and described by the toolchain it pins
        // itself, not by the one that happened to build this tool, so the stored
        // provenance names the compiler that produced the numbers.
        let project_dir = worktree.join(self.project_relative_dir);
        // Historical project absence is a per-commit failure; a missing executable is not.
        // Ref: docs/implementation.md, Backfill project-directory handling.
        let directory = tokio::fs::metadata(&project_dir)
            .await
            .map(|metadata| metadata.is_dir());
        if let Err(error) = check_project_directory(&project_dir, directory) {
            return map_collect_result(Err(error));
        }
        let probe = SystemProbe::in_worktree(&project_dir);
        let runner = TokioBenchRunner::in_worktree(&project_dir);
        let target_root = project_dir.join("target");
        let output = FsBenchOutputSource::new(target_root.clone());
        let clock = Clock::new_tokio();
        let env = |name: &str| std::env::var(name).ok();

        // A backfilled run is always clean (the worktree is a pristine checkout)
        // and takes its timeline position from the commit's committer date.
        let collect_options = CollectOptions {
            config_path: None,
            repo: None,
            local: None,
            packages: self.options.packages.clone(),
            excludes: self.options.excludes.clone(),
            benches: self.options.benches.clone(),
            features: self.options.features.clone(),
            all_features: self.options.all_features,
            no_default_features: self.options.no_default_features,
            no_store: false,
            overwrite: self.options.overwrite,
            skip_existing: false,
            passthrough: self.options.passthrough.clone(),
            verbose: self.options.verbose,
            best_of: self.options.best_of,
        };
        let deps = CollectDeps {
            runner: &runner,
            probe: &probe,
            output: &output,
            storage: Some(self.storage),
            clock: &clock,
            env: &env,
            project_id: self.project_id,
            tool_version: self.tool_version,
            target_root: &target_root,
            bench_command: self.bench_command,
            reporter: self.reporter,
        };

        map_collect_result(run_engines(&collect_options, &deps).await)
    }
}
