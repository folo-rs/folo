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
//! current comparisons draw on, so a run that is cut short (a CI job timeout) has
//! spent its time on the most valuable commits. The endpoints are independent of
//! that walk: `--from` names the oldest commit of the range and `--to` the newest,
//! both inclusive.
//!
//! Like `collect`, the orchestration is generic over small ports so the loop logic is
//! exercised with in-memory fakes (Miri-safe): a [`BackfillGit`] port for the git
//! topology and worktree lifecycle, and a [`CommitRunner`] port that runs and
//! stores one commit. The production [`execute`] wires the real adapters; the real
//! [`CommitRunner`] reuses the `collect` pipeline ([`run_engines`]) against a
//! worktree-rooted probe, engine runner, and output source.
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
use std::path::{Path, PathBuf};
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

use super::collect::{
    CollectDeps, CollectSummary, Partition, default_bench_command, partition_selection_summary,
    probe_partition, run_engines,
};
use crate::errors::{
    AddWorktreeFailedError, FirstParentWalkFailedError, RemoveWorktreeFailedError,
    ResetWorktreeFailedError, ResolveRefFailedError, is_bench_failure, render_bench_failure,
};
use crate::model::{Engine, StorageKey, parse_key};
use crate::{
    BackfillError, BackfillOptions, CollectOptions, DuplicateResultError, RunOutcome,
    finish_with_flush,
};

/// Read access to a repository's commit topology plus the worktree lifecycle a
/// backfill needs to check out each commit in isolation.
trait BackfillGit {
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
trait CommitRunner {
    /// The set of commits (by full commit ID) that already have a stored clean
    /// result **in the partition this backfill writes to** — this host's target
    /// triple and machine key (or the `--machine-key` override), across every
    /// engine. A commit in that set has already been backfilled here, so the
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
    /// Implementations announce the partition they scanned. A backfill that a job
    /// timeout ends never reaches the summary, so the pre-check has to state its
    /// own inputs to leave any trace of what the run decided to measure.
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
enum CommitOutcome {
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
    BenchFailed { error: AppError },
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
    let worktree = worktree_path();
    let reporter = StderrReporter::new(options.verbose);
    let runner = SystemCommitRunner {
        project_id: &project_id,
        storage: &storage,
        tool_version: env!("CARGO_PKG_VERSION"),
        options,
        bench_command: &bench_command,
        worktree: &worktree,
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
async fn execute_backfill<G, C>(
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
    if let Some(stopped_failure) = report.stopped_failure {
        let error = report.failures.remove(stopped_failure).error;
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
async fn plan_commits<G: BackfillGit>(
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
/// `commits` arrives newest-first, so the most recent gaps are filled before a run
/// is cut short. A per-commit build/bench failure stops the loop unless
/// `--ignore-errors` is set — which, in this order, means the run stops at the
/// *newest* failing commit and leaves the older ones untouched; an infrastructure
/// error always aborts (propagated as `Err`).
///
/// What the skip pre-check decided is announced before the loop rather than left
/// to [`BackfillReport::render`]: a run that is cut short by a job timeout — the
/// designed steady state of a nightly backfill — never reaches the summary, and
/// the pre-check is precisely the step whose misbehaviour would show up as a run
/// that quietly measures nothing.
async fn run_commits<G, C>(
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
    // Only the overlap with this range matters: the listings cover the partition's
    // whole history, which reaches beyond `--from`..`--to`.
    let already_recorded = commits
        .iter()
        .filter(|commit| recorded.contains(*commit))
        .count();
    reporter.announce(&scan_outcome_summary(
        commits.len(),
        already_recorded,
        options.overwrite,
    ));

    for commit in commits {
        if recorded.contains(commit) {
            reporter.note_with(|| {
                format!(
                    "skipping {}: a clean result for it is already stored in this partition",
                    short(commit)
                )
            });
            report.skipped_existing.push(commit.clone());
            continue;
        }
        git.reset_to(worktree, commit)
            .await
            .map_err(|error| ResetWorktreeFailedError::caused_by(worktree, commit, error))?;
        match runner.run(worktree, commit).await? {
            CommitOutcome::Stored { cases } => report.stored.push((commit.clone(), cases)),
            CommitOutcome::SkippedExisting => report.skipped_existing.push(commit.clone()),
            CommitOutcome::SkippedEmpty => report.skipped_empty.push(commit.clone()),
            CommitOutcome::BenchFailed { error } => {
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
/// run whose measured count is surprising can be diagnosed from this one line —
/// which, in a run that a job timeout ends before the summary, is the only record
/// of the decision.
///
/// A pure formatter so the wording is unit-tested without a store.
fn scan_outcome_summary(total: usize, already_recorded: usize, overwrite: bool) -> String {
    let range = format!("backfilling {}, newest first", count_noun(total, "commit"));
    if overwrite {
        return format!(
            "{range}: --overwrite disables the skip pre-check, so every commit is \
             re-measured and its stored result replaced"
        );
    }
    let to_measure = total.saturating_sub(already_recorded);
    format!(
        "{range}: {already_recorded} already recorded in this partition and skipped \
         without benchmarking (pass --overwrite to re-measure them), {to_measure} to measure"
    )
}

/// One failed commit and the typed application error that explains it.
#[derive(Debug)]
struct FailedCommit {
    commit: String,
    error: AppError,
}

/// The per-commit outcomes a backfill accumulated, rendered into a summary.
#[derive(Debug, Default)]
struct BackfillReport {
    /// Commits whose results were stored, with the harvested case count.
    stored: Vec<(String, usize)>,
    /// Commits skipped because a result already existed.
    skipped_existing: Vec<String>,
    /// Commits skipped because they harvested no cases.
    skipped_empty: Vec<String>,
    /// Typed per-commit benchmark failures.
    failures: Vec<FailedCommit>,
    /// Index in `failures` that stopped processing without `--ignore-errors`.
    stopped_failure: Option<usize>,
}

impl BackfillReport {
    /// Renders the multi-line summary for a range of `total` commits.
    fn render(&self, total: usize) -> String {
        let mut lines = vec![format!(
            "Backfill range of {}: {} stored, {} skipped (existing), \
             {} skipped (empty), {} failed.",
            count_noun(total, "commit"),
            self.stored.len(),
            self.skipped_existing.len(),
            self.skipped_empty.len(),
            self.failures.len(),
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
            let reason = render_bench_failure(&failure.error)
                .expect("BackfillReport stores only classified benchmark failures");
            lines.push(format!("  failed {} ({reason})", short(&failure.commit)));
        }
        if let Some(stopped_failure) = self.stopped_failure {
            let commit = &self
                .failures
                .get(stopped_failure)
                .expect("stopped_failure indexes a recorded failure")
                .commit;
            lines.push(format!(
                "  stopped at {} (pass --ignore-errors to continue past failures)",
                short(commit)
            ));
        }
        lines.join("\n")
    }
}

/// Abbreviates a commit ID for display, falling back to the full value.
fn short(commit_id: &str) -> &str {
    commit_id.get(..12).unwrap_or(commit_id)
}

/// A unique scratch path for the backfill worktree, under the system temp dir.
fn worktree_path() -> PathBuf {
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

/// Maps a per-commit `collect` result to a [`CommitOutcome`].
///
/// A stored set (or several) is success; a duplicate is a resumable skip; an
/// empty harvest is a non-fatal skip; a build/bench failure is recoverable;
/// everything else (storage, configuration, I/O) is infrastructure and aborts.
fn map_collect_result(result: Result<CollectSummary, AppError>) -> Result<CommitOutcome, AppError> {
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
    if is_bench_failure(&error) {
        Ok(CommitOutcome::BenchFailed { error })
    } else {
        Err(error)
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
async fn recorded_commits_in<S: Storage>(
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
    /// The backfill options whose scope/triple/machine/overwrite are reused.
    options: &'a BackfillOptions,
    /// The benchmark command (`cargo bench` in production) run in each worktree.
    bench_command: &'a [String],
    /// The worktree every commit is checked out into. The pre-check probes it for
    /// the partition, so it reads the same partition each commit then writes to.
    worktree: &'a Path,
    /// Diagnostic sink for the pre-check, and for each per-commit `collect`.
    reporter: &'a dyn Reporter,
}

impl<S: Storage> CommitRunner for SystemCommitRunner<'_, S> {
    #[cfg_attr(test, mutants::skip)] // Probes the real host; the scan is tested separately.
    async fn recorded_commits(&self) -> Result<HashSet<String>, AppError> {
        // The partition comes from the same helper (and the same worktree probe)
        // the store path uses, so the commits treated as already recorded are
        // exactly the ones that would collide were they benchmarked again — the
        // `--machine-key` override included.
        let probe = SystemProbe::in_worktree(self.worktree);
        let env = |name: &str| std::env::var(name).ok();
        let partition = probe_partition(&probe, &env, self.options.machine_key.as_deref()).await?;

        // Always-on, because a nightly backfill is designed to end in a job
        // timeout and never reaches the summary: this line is what tells a reader
        // which partition the run considered, and therefore what it measured.
        self.reporter.announce(&partition_selection_summary(
            "scanning for already-backfilled commits",
            partition.target_triple.as_str(),
            "toolchain host of the newest checkout in the range",
            partition.machine_key.as_str(),
            self.options.machine_key.is_some(),
        ));
        recorded_commits_in(self.storage, self.project_id, &partition, self.reporter).await
    }

    #[cfg_attr(test, mutants::skip)] // Wires real adapters; the result mapping is tested via `map_collect_result`.
    async fn run(&self, worktree: &Path, _commit: &str) -> Result<CommitOutcome, AppError> {
        // A historical checkout is built and described by the toolchain it pins
        // itself, not by the one that happened to build this tool, so the stored
        // provenance names the compiler that produced the numbers.
        let probe = SystemProbe::in_worktree(worktree);
        let runner = TokioBenchRunner::in_worktree(worktree);
        let target_root = worktree.join("target");
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
            machine_key: self.options.machine_key.clone(),
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};
    use std::future::{Future, ready};

    use cbh_diag::RecordingReporter;
    use cbh_git::FakeGitHistory;
    use cbh_storage::MemoryStorage;
    use futures::executor::block_on;

    use super::*;
    use crate::errors::{EngineFailedError, InvalidCommandError, ParseOutputError};
    use crate::model::{MachineKey, TargetTriple};
    use cbh_storage::StorageError;

    /// A canned per-commit result the fake [`CommitRunner`] returns.
    #[derive(Clone)]
    enum FakeResult {
        Stored(usize),
        SkippedExisting,
        SkippedEmpty,
        BenchFailed(String),
        Infra(String),
    }

    /// In-memory [`BackfillGit`] over a [`FakeGitHistory`], recording worktree ops.
    struct FakeBackfillGit {
        history: FakeGitHistory,
        added: RefCell<Vec<(PathBuf, String)>>,
        resets: RefCell<Vec<(PathBuf, String)>>,
        removed: RefCell<Vec<PathBuf>>,
        fail_add: bool,
        fail_reset: bool,
        fail_remove: bool,
    }

    impl FakeBackfillGit {
        fn new(history: FakeGitHistory) -> Self {
            Self {
                history,
                added: RefCell::new(Vec::new()),
                resets: RefCell::new(Vec::new()),
                removed: RefCell::new(Vec::new()),
                fail_add: false,
                fail_reset: false,
                fail_remove: false,
            }
        }

        fn with_add_failure(mut self) -> Self {
            self.fail_add = true;
            self
        }

        fn with_reset_failure(mut self) -> Self {
            self.fail_reset = true;
            self
        }

        fn with_remove_failure(mut self) -> Self {
            self.fail_remove = true;
            self
        }
    }

    impl BackfillGit for FakeBackfillGit {
        fn resolve(&self, reference: &str) -> impl Future<Output = io::Result<Option<String>>> {
            self.history.resolve(reference)
        }

        fn first_parent(&self, reference: &str) -> impl Future<Output = io::Result<Vec<String>>> {
            let future = self.history.first_parent(reference);
            async move {
                let commits = future.await?;
                Ok(commits.into_iter().map(|commit| commit.commit_id).collect())
            }
        }

        fn add_worktree(&self, path: &Path, commit: &str) -> impl Future<Output = io::Result<()>> {
            self.added
                .borrow_mut()
                .push((path.to_owned(), commit.to_owned()));
            ready(if self.fail_add {
                Err(io::Error::other("injected add-worktree failure"))
            } else {
                Ok(())
            })
        }

        fn reset_to(&self, path: &Path, commit: &str) -> impl Future<Output = io::Result<()>> {
            self.resets
                .borrow_mut()
                .push((path.to_owned(), commit.to_owned()));
            ready(if self.fail_reset {
                Err(io::Error::other("injected reset-worktree failure"))
            } else {
                Ok(())
            })
        }

        fn remove_worktree(&self, path: &Path) -> impl Future<Output = io::Result<()>> {
            self.removed.borrow_mut().push(path.to_owned());
            ready(if self.fail_remove {
                Err(io::Error::other("injected remove-worktree failure"))
            } else {
                Ok(())
            })
        }
    }

    /// In-memory [`CommitRunner`] returning canned per-commit results.
    struct FakeCommitRunner {
        outcomes: HashMap<String, FakeResult>,
        complete: HashSet<String>,
        ran: RefCell<Vec<String>>,
    }

    impl FakeCommitRunner {
        fn new() -> Self {
            Self {
                outcomes: HashMap::new(),
                complete: HashSet::new(),
                ran: RefCell::new(Vec::new()),
            }
        }

        fn with(mut self, commit: &str, result: FakeResult) -> Self {
            self.outcomes.insert(commit.to_owned(), result);
            self
        }

        /// Marks `commit` as already fully recorded, so the pre-run check skips it.
        fn complete(mut self, commit: &str) -> Self {
            self.complete.insert(commit.to_owned());
            self
        }
    }

    impl CommitRunner for FakeCommitRunner {
        fn recorded_commits(&self) -> impl Future<Output = Result<HashSet<String>, AppError>> {
            ready(Ok(self.complete.clone()))
        }

        fn run(
            &self,
            _worktree: &Path,
            commit: &str,
        ) -> impl Future<Output = Result<CommitOutcome, AppError>> {
            self.ran.borrow_mut().push(commit.to_owned());
            let result = match self.outcomes.get(commit) {
                Some(FakeResult::Stored(cases)) => Ok(CommitOutcome::Stored { cases: *cases }),
                Some(FakeResult::SkippedExisting) => Ok(CommitOutcome::SkippedExisting),
                Some(FakeResult::SkippedEmpty) => Ok(CommitOutcome::SkippedEmpty),
                Some(FakeResult::BenchFailed(reason)) => Ok(CommitOutcome::BenchFailed {
                    error: InvalidCommandError::new("cargo bench", reason).into(),
                }),
                Some(FakeResult::Infra(_message)) => {
                    Err(
                        build_storage(None, &cbh_config::Config::default(), Path::new("."), None)
                            .unwrap_err()
                            .into(),
                    )
                }
                None => Ok(CommitOutcome::Stored { cases: 1 }),
            };
            ready(result)
        }
    }

    /// `master: c0 - c1 - c2 - c3`, `feature: c1 - f1 - f2`, HEAD at `feature`.
    fn fixture() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .commit("f1", Some("c1"))
            .commit("f2", Some("f1"))
            .branch("master", "c3")
            .branch("feature", "f2")
            .head("feature")
            .mark_default("master");
        git
    }

    fn options(from: &str, to: &str) -> BackfillOptions {
        BackfillOptions {
            from: from.to_owned(),
            to: to.to_owned(),
            ..BackfillOptions::default()
        }
    }

    fn worktree() -> PathBuf {
        PathBuf::from("/tmp/cargo-bench-history-worktree-test")
    }

    /// Drives [`run_commits`] over `commits`, discarding the diagnostics the
    /// separate reporting tests assert on.
    fn drive_commits<G: BackfillGit, C: CommitRunner>(
        options: &BackfillOptions,
        git: &G,
        runner: &C,
        commits: &[String],
    ) -> Result<BackfillReport, AppError> {
        let reporter = RecordingReporter::new();
        block_on(run_commits(
            options,
            git,
            runner,
            &worktree(),
            commits,
            &reporter,
        ))
    }

    #[test]
    fn plan_enumerates_inclusive_first_parent_range_newest_first() {
        let git = FakeBackfillGit::new(fixture());
        let commits = block_on(plan_commits(&options("c1", "f2"), &git)).unwrap();
        assert!(
            commits.iter().eq(["f2", "f1", "c1"].iter()),
            "inclusive of both endpoints, newest first: {commits:?}"
        );
    }

    #[test]
    fn plan_includes_a_single_commit_range() {
        let git = FakeBackfillGit::new(fixture());
        let commits = block_on(plan_commits(&options("f2", "f2"), &git)).unwrap();
        assert!(commits.iter().eq(std::iter::once(&"f2")), "{commits:?}");
    }

    #[test]
    fn plan_rejects_an_unresolvable_endpoint() {
        let git = FakeBackfillGit::new(fixture());
        let error = block_on(plan_commits(&options("absent", "f2"), &git)).unwrap_err();
        assert!(error.find_source::<BackfillError>().is_some());
    }

    #[test]
    fn plan_rejects_a_from_that_is_not_an_ancestor_of_to() {
        // f1 is on the feature side, not in master's first-parent ancestry.
        let git = FakeBackfillGit::new(fixture());
        let error = block_on(plan_commits(&options("f1", "c3"), &git)).unwrap_err();
        assert!(error.find_source::<BackfillError>().is_some());
    }

    #[test]
    fn plan_maps_a_ref_resolution_failure() {
        let mut history = fixture();
        history.fail_resolve();
        let git = FakeBackfillGit::new(history);

        let error = block_on(plan_commits(&options("c1", "f2"), &git)).unwrap_err();

        assert!(error.find_source::<ResolveRefFailedError>().is_some());
        assert!(error.find_source::<FirstParentWalkFailedError>().is_none());
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[test]
    fn plan_maps_a_first_parent_walk_failure() {
        let mut history = fixture();
        history.fail_first_parent();
        let git = FakeBackfillGit::new(history);

        let error = block_on(plan_commits(&options("c1", "f2"), &git)).unwrap_err();

        assert!(error.find_source::<FirstParentWalkFailedError>().is_some());
        assert!(error.find_source::<ResolveRefFailedError>().is_none());
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[test]
    fn plan_backfills_a_to_outside_the_current_branch_history() {
        // HEAD is at feature; c3 (master tip) is not part of feature's history,
        // yet a range built purely from --to's first-parent ancestry still plans.
        let git = FakeBackfillGit::new(fixture());
        let commits = block_on(plan_commits(&options("c0", "c3"), &git)).unwrap();
        assert!(
            commits.iter().eq(["c3", "c2", "c1", "c0"].iter()),
            "the range is derived from --to, independent of the checkout: {commits:?}"
        );
    }

    #[test]
    fn run_commits_records_every_outcome_kind() {
        let git = FakeBackfillGit::new(fixture());
        let runner = FakeCommitRunner::new()
            .with("c0", FakeResult::Stored(5))
            .with("c1", FakeResult::SkippedExisting)
            .with("f1", FakeResult::SkippedEmpty)
            .with("f2", FakeResult::Stored(3));
        let commits = vec![
            "c0".to_owned(),
            "c1".to_owned(),
            "f1".to_owned(),
            "f2".to_owned(),
        ];

        let report = drive_commits(&options("c0", "f2"), &git, &runner, &commits).unwrap();

        assert!(
            report
                .stored
                .iter()
                .eq([("c0".to_owned(), 5), ("f2".to_owned(), 3)].iter()),
            "{:?}",
            report.stored
        );
        assert!(report.skipped_existing.iter().eq(std::iter::once(&"c1")));
        assert!(report.skipped_empty.iter().eq(std::iter::once(&"f1")));
        assert!(report.failures.is_empty());
        assert!(report.stopped_failure.is_none());
        // Every commit was reset into the worktree, in order.
        assert!(
            git.resets
                .borrow()
                .iter()
                .map(|(_, commit)| commit.as_str())
                .eq(["c0", "c1", "f1", "f2"]),
            "{:?}",
            git.resets.borrow()
        );
        assert!(runner.ran.borrow().iter().eq(commits.iter()));
    }

    #[test]
    fn run_commits_stops_on_first_failure_by_default() {
        let git = FakeBackfillGit::new(fixture());
        let runner = FakeCommitRunner::new().with("c1", FakeResult::BenchFailed("boom".to_owned()));
        let commits = vec!["c0".to_owned(), "c1".to_owned(), "f1".to_owned()];

        let report = drive_commits(&options("c0", "f1"), &git, &runner, &commits).unwrap();

        assert!(
            report
                .stored
                .iter()
                .eq(std::iter::once(&("c0".to_owned(), 1)))
        );
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures.first().unwrap().commit, "c1");
        assert!(report.render(3).contains("boom"));
        assert_eq!(report.stopped_failure, Some(0));
        // f1 was never reached.
        assert!(runner.ran.borrow().iter().eq(["c0", "c1"].iter()));
    }

    #[test]
    fn run_commits_continues_past_failures_with_ignore_errors() {
        let git = FakeBackfillGit::new(fixture());
        let runner = FakeCommitRunner::new().with("c1", FakeResult::BenchFailed("boom".to_owned()));
        let commits = vec!["c0".to_owned(), "c1".to_owned(), "f1".to_owned()];
        let mut opts = options("c0", "f1");
        opts.ignore_errors = true;

        let report = drive_commits(&opts, &git, &runner, &commits).unwrap();

        assert!(
            report
                .stored
                .iter()
                .eq([("c0".to_owned(), 1), ("f1".to_owned(), 1)].iter())
        );
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures.first().unwrap().commit, "c1");
        assert!(report.render(3).contains("boom"));
        assert!(report.stopped_failure.is_none());
        assert!(runner.ran.borrow().iter().eq(["c0", "c1", "f1"].iter()));
    }

    #[test]
    fn run_commits_aborts_on_infrastructure_error_even_with_ignore_errors() {
        let git = FakeBackfillGit::new(fixture());
        let runner = FakeCommitRunner::new().with("c1", FakeResult::Infra("disk".to_owned()));
        let commits = vec!["c0".to_owned(), "c1".to_owned(), "f1".to_owned()];
        let mut opts = options("c0", "f1");
        opts.ignore_errors = true;

        let error = drive_commits(&opts, &git, &runner, &commits).unwrap_err();
        assert!(!is_bench_failure(&error));
        assert!(error.find_source::<StorageError>().is_some());
        // The loop stopped at the failing commit; f1 was never reached.
        assert!(runner.ran.borrow().iter().eq(["c0", "c1"].iter()));
    }

    /// The project the storage-scan tests write and read under.
    const PROJECT: &str = "proj";

    /// The target triple the storage-scan tests treat as this host's.
    const TRIPLE: &str = "x86_64-unknown-linux-gnu";

    /// The machine key the storage-scan tests treat as this host's. It is a strict
    /// prefix of `ci-pool` so the sibling-key cases are meaningful.
    const MACHINE: &str = "ci";

    /// The partition a backfill would write to on a host with `machine_key`.
    fn partition(machine_key: &str) -> Partition {
        Partition {
            target_triple: TargetTriple::from(TRIPLE),
            machine_key: MachineKey::from(machine_key),
        }
    }

    /// Stores an empty object under `key`, standing in for a recorded result.
    fn store(storage: &MemoryStorage, key: &str) {
        block_on(storage.put(key, b"{}")).unwrap();
    }

    /// The commits `recorded_commits_in` finds for `machine_key`, sorted.
    fn recorded(storage: &MemoryStorage, machine_key: &str) -> Vec<String> {
        let reporter = RecordingReporter::new();
        let mut commits: Vec<_> = block_on(recorded_commits_in(
            storage,
            PROJECT,
            &partition(machine_key),
            &reporter,
        ))
        .unwrap()
        .into_iter()
        .collect();
        commits.sort();
        commits
    }

    #[test]
    fn run_commits_skips_a_recorded_commit_without_resetting_or_running_it() {
        let git = FakeBackfillGit::new(fixture());
        let runner = FakeCommitRunner::new().complete("c1");
        let commits = vec!["c0".to_owned(), "c1".to_owned(), "f1".to_owned()];

        let report = drive_commits(&options("c0", "f1"), &git, &runner, &commits).unwrap();

        // c1 was recognized as already recorded and reported as skipped-existing.
        assert!(report.skipped_existing.iter().eq(std::iter::once(&"c1")));
        assert!(
            report
                .stored
                .iter()
                .eq([("c0".to_owned(), 1), ("f1".to_owned(), 1)].iter())
        );
        // The expensive work was avoided: c1 was neither reset into the worktree
        // nor run.
        assert!(runner.ran.borrow().iter().eq(["c0", "f1"].iter()));
        assert!(
            git.resets
                .borrow()
                .iter()
                .map(|(_, commit)| commit.as_str())
                .eq(["c0", "f1"]),
            "{:?}",
            git.resets.borrow()
        );
    }

    #[test]
    fn run_commits_reruns_a_recorded_commit_when_overwriting() {
        let git = FakeBackfillGit::new(fixture());
        let runner = FakeCommitRunner::new().complete("c1");
        let commits = vec!["c0".to_owned(), "c1".to_owned(), "f1".to_owned()];
        let mut opts = options("c0", "f1");
        opts.overwrite = true;

        let report = drive_commits(&opts, &git, &runner, &commits).unwrap();

        // With --overwrite the pre-check is bypassed: every commit, including the
        // already-recorded c1, is reset and run.
        assert!(runner.ran.borrow().iter().eq(["c0", "c1", "f1"].iter()));
        assert!(report.skipped_existing.is_empty());
        assert!(
            report.stored.iter().eq([
                ("c0".to_owned(), 1),
                ("c1".to_owned(), 1),
                ("f1".to_owned(), 1)
            ]
            .iter())
        );
    }

    #[test]
    fn run_commits_announces_what_the_skip_pre_check_decided() {
        // A nightly backfill is designed to be killed by a job timeout and never
        // reaches the summary, so the split between skipped and to-be-measured
        // commits has to be stated up front or the run leaves no record of it.
        let git = FakeBackfillGit::new(fixture());
        let runner = FakeCommitRunner::new().complete("c1");
        let commits = vec!["c0".to_owned(), "c1".to_owned(), "f1".to_owned()];
        let reporter = RecordingReporter::new();

        _ = block_on(run_commits(
            &options("c0", "f1"),
            &git,
            &runner,
            &worktree(),
            &commits,
            &reporter,
        ))
        .unwrap();

        let announcements = reporter.announcements();
        assert!(
            announcements.iter().any(|line| line.contains("3 commits")
                && line.contains("1 already recorded")
                && line.contains("2 to measure")),
            "{announcements:?}"
        );
        // Progress is visible per commit under --verbose, so a run cut short still
        // shows how far the skipping got.
        assert!(reporter.contains("skipping c1"), "{:?}", reporter.notes());
    }

    #[test]
    fn scan_outcome_summary_states_the_split_and_the_rule_behind_it() {
        let partial = scan_outcome_summary(10, 4, false);
        assert!(
            partial.contains("backfilling 10 commits, newest first"),
            "{partial}"
        );
        assert!(partial.contains("4 already recorded"), "{partial}");
        assert!(partial.contains("6 to measure"), "{partial}");
        // The flag that changes the decision is named, so a surprising count is
        // actionable from this line alone.
        assert!(partial.contains("--overwrite"), "{partial}");

        // A range with nothing left to do still says so rather than staying silent.
        let complete = scan_outcome_summary(10, 10, false);
        assert!(complete.contains("0 to measure"), "{complete}");

        // With --overwrite the pre-check never ran, so no count is invented for it.
        let overwriting = scan_outcome_summary(10, 0, true);
        assert!(
            overwriting.contains("--overwrite disables the skip pre-check"),
            "{overwriting}"
        );
        assert!(!overwriting.contains("to measure"), "{overwriting}");
    }

    #[test]
    fn recorded_commits_explains_each_listing_and_the_union_rule() {
        // The verbose trail must let a reader reconstruct why a commit counted:
        // which prefixes were listed, what each contributed, and which objects were
        // deliberately not counted.
        let storage = MemoryStorage::new();
        store(
            &storage,
            "v1/proj/objects/criterion/x86_64-unknown-linux-gnu/ci/c0/clean.json",
        );
        let reporter = RecordingReporter::new();

        _ = block_on(recorded_commits_in(
            &storage,
            PROJECT,
            &partition(MACHINE),
            &reporter,
        ))
        .unwrap();

        let notes = reporter.notes();
        assert!(
            notes.iter().any(|note| {
                note.contains("v1/proj/objects/criterion/x86_64-unknown-linux-gnu/ci/")
                    && note.contains("1 of them clean")
            }),
            "{notes:?}"
        );
        assert!(
            reporter.contains("unioned rather than intersected"),
            "{notes:?}"
        );
        assert!(reporter.contains("independent data sets"), "{notes:?}");
    }

    #[test]
    fn recorded_commits_counts_any_engine_of_the_partition() {
        let storage = MemoryStorage::new();
        // No rule says which engines a run produces, so a clean result from any one
        // of them means the partition's gap for that commit is filled.
        store(
            &storage,
            "v1/proj/objects/callgrind/x86_64-unknown-linux-gnu/ci/c0/clean.json",
        );
        store(
            &storage,
            "v1/proj/objects/criterion/x86_64-unknown-linux-gnu/ci/c1/clean.json",
        );

        assert_eq!(recorded(&storage, MACHINE), ["c0", "c1"]);
    }

    #[test]
    fn recorded_commits_ignores_a_sibling_machine_key() {
        let storage = MemoryStorage::new();
        // `ci-pool` is a different machine and thus a different data set, even
        // though its key starts with this host's `ci`.
        store(
            &storage,
            "v1/proj/objects/criterion/x86_64-unknown-linux-gnu/ci-pool/c0/clean.json",
        );

        assert!(recorded(&storage, MACHINE).is_empty());
        // The same object is exactly what the sibling machine itself would skip.
        assert_eq!(recorded(&storage, "ci-pool"), ["c0"]);
    }

    #[test]
    fn recorded_commits_ignores_another_target_triple() {
        let storage = MemoryStorage::new();
        // Numbers from another platform say nothing about this one.
        store(
            &storage,
            "v1/proj/objects/criterion/aarch64-apple-darwin/ci/c0/clean.json",
        );

        assert!(recorded(&storage, MACHINE).is_empty());
    }

    #[test]
    fn recorded_commits_ignores_another_project() {
        let storage = MemoryStorage::new();
        store(
            &storage,
            "v1/other/objects/criterion/x86_64-unknown-linux-gnu/ci/c0/clean.json",
        );

        assert!(recorded(&storage, MACHINE).is_empty());
    }

    #[test]
    fn recorded_commits_ignores_objects_that_are_not_clean_results() {
        let storage = MemoryStorage::new();
        // A dirty snapshot is a working-tree measurement and a blessing sidecar is
        // an annotation; neither fills a backfill gap.
        store(
            &storage,
            "v1/proj/objects/criterion/x86_64-unknown-linux-gnu/ci/c0/dirty-7.json",
        );
        store(
            &storage,
            "v1/proj/objects/criterion/x86_64-unknown-linux-gnu/ci/c1/bless-7.json",
        );

        assert!(recorded(&storage, MACHINE).is_empty());
    }

    #[test]
    fn recorded_commits_is_empty_when_nothing_is_stored() {
        let storage = MemoryStorage::new();

        assert!(recorded(&storage, MACHINE).is_empty());
    }

    #[test]
    fn render_pluralizes_commit_and_case_counts() {
        let one = BackfillReport {
            stored: vec![("abcdef0".to_owned(), 1)],
            ..BackfillReport::default()
        };
        let rendered = one.render(1);
        assert!(
            rendered.contains("Backfill range of 1 commit:"),
            "{rendered}"
        );
        assert!(rendered.contains("(1 case)"), "{rendered}");

        let many = BackfillReport {
            stored: vec![("abcdef0".to_owned(), 3)],
            ..BackfillReport::default()
        };
        let rendered = many.render(2);
        assert!(
            rendered.contains("Backfill range of 2 commits:"),
            "{rendered}"
        );
        assert!(rendered.contains("(3 cases)"), "{rendered}");
    }

    #[test]
    fn render_lists_a_line_for_every_outcome_section() {
        let report = BackfillReport {
            stored: vec![("aaaaaaa".to_owned(), 2)],
            skipped_existing: vec!["bbbbbbb".to_owned()],
            skipped_empty: vec!["ccccccc".to_owned()],
            failures: vec![FailedCommit {
                commit: "ddddddd".to_owned(),
                error: InvalidCommandError::new("cargo bench", "boom").into(),
            }],
            stopped_failure: Some(0),
        };

        let rendered = report.render(5);

        assert!(
            rendered.contains(
                "Backfill range of 5 commits: 1 stored, 1 skipped (existing), \
                 1 skipped (empty), 1 failed."
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("  stored aaaaaaa (2 cases)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("  skipped bbbbbbb (already stored)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("  skipped ccccccc (no benchmark cases)"),
            "{rendered}"
        );
        assert!(
            rendered
                .contains("  failed ddddddd (engine \"cargo bench\" has an invalid command: boom)"),
            "{rendered}"
        );
        assert!(
            rendered
                .contains("  stopped at ddddddd (pass --ignore-errors to continue past failures)"),
            "{rendered}"
        );
    }

    #[test]
    fn execute_completes_and_tears_down_on_success() {
        let git = FakeBackfillGit::new(fixture());
        let runner = FakeCommitRunner::new();
        let outcome = block_on(execute_backfill(
            &options("c0", "f2"),
            &git,
            &runner,
            &worktree(),
            &RecordingReporter::new(),
        ))
        .unwrap();

        let RunOutcome::Completed { message } = outcome else {
            panic!("expected a completed outcome");
        };
        assert!(message.contains("4 stored"), "{message}");
        // The worktree was created once, at the newest commit — the first one the
        // newest-first walk processes — and then removed.
        assert!(
            git.added
                .borrow()
                .iter()
                .eq(std::iter::once(&(worktree(), "f2".to_owned())))
        );
        assert!(git.removed.borrow().iter().eq(std::iter::once(&worktree())));
    }

    #[test]
    fn execute_maps_an_add_worktree_failure() {
        let git = FakeBackfillGit::new(fixture()).with_add_failure();
        let error = block_on(execute_backfill(
            &options("c0", "f2"),
            &git,
            &FakeCommitRunner::new(),
            &worktree(),
            &RecordingReporter::new(),
        ))
        .unwrap_err();

        assert!(error.find_source::<AddWorktreeFailedError>().is_some());
        assert!(error.find_source::<io::Error>().is_some());
        assert!(git.removed.borrow().is_empty());
    }

    #[test]
    fn execute_maps_a_reset_failure_and_still_tears_down() {
        let git = FakeBackfillGit::new(fixture()).with_reset_failure();
        let error = block_on(execute_backfill(
            &options("c0", "f2"),
            &git,
            &FakeCommitRunner::new(),
            &worktree(),
            &RecordingReporter::new(),
        ))
        .unwrap_err();

        assert!(error.find_source::<ResetWorktreeFailedError>().is_some());
        assert!(error.find_source::<io::Error>().is_some());
        assert!(git.removed.borrow().iter().eq(std::iter::once(&worktree())));
    }

    #[test]
    fn execute_maps_a_remove_worktree_failure() {
        let git = FakeBackfillGit::new(fixture()).with_remove_failure();
        let error = block_on(execute_backfill(
            &options("c0", "f2"),
            &git,
            &FakeCommitRunner::new(),
            &worktree(),
            &RecordingReporter::new(),
        ))
        .unwrap_err();

        assert!(error.find_source::<RemoveWorktreeFailedError>().is_some());
        assert!(error.find_source::<io::Error>().is_some());
        assert!(git.removed.borrow().iter().eq(std::iter::once(&worktree())));
    }

    #[test]
    fn execute_returns_error_and_tears_down_when_stopped() {
        let git = FakeBackfillGit::new(fixture());
        let runner = FakeCommitRunner::new().with("c1", FakeResult::BenchFailed("boom".to_owned()));
        let error = block_on(execute_backfill(
            &options("c0", "f2"),
            &git,
            &runner,
            &worktree(),
            &RecordingReporter::new(),
        ))
        .unwrap_err();

        assert!(error.find_source::<BackfillError>().is_some());
        assert!(error.find_source::<InvalidCommandError>().is_some());
        // Teardown still happened despite the failure.
        assert!(git.removed.borrow().iter().eq(std::iter::once(&worktree())));
    }

    #[test]
    fn execute_tears_down_after_an_infrastructure_abort() {
        let git = FakeBackfillGit::new(fixture());
        let runner = FakeCommitRunner::new().with("c0", FakeResult::Infra("disk".to_owned()));
        let error = block_on(execute_backfill(
            &options("c0", "f2"),
            &git,
            &runner,
            &worktree(),
            &RecordingReporter::new(),
        ))
        .unwrap_err();

        assert!(error.find_source::<StorageError>().is_some());
        assert!(git.removed.borrow().iter().eq(std::iter::once(&worktree())));
    }

    #[test]
    fn map_collect_result_classifies_each_run_outcome() {
        let stored = map_collect_result(Ok(CollectSummary {
            stored: 1,
            harvested: 7,
            labels: Vec::new(),
        }))
        .unwrap();
        assert!(matches!(stored, CommitOutcome::Stored { cases: 7 }));

        let empty = map_collect_result(Ok(CollectSummary {
            stored: 0,
            harvested: 0,
            labels: Vec::new(),
        }))
        .unwrap();
        assert!(matches!(empty, CommitOutcome::SkippedEmpty));

        let duplicate = map_collect_result(Err(DuplicateResultError::new(
            "v1/p/objects/callgrind/t/m1/abc/clean.json",
        )
        .into()))
        .unwrap();
        assert!(matches!(duplicate, CommitOutcome::SkippedExisting));

        let failed =
            map_collect_result(Err(EngineFailedError::new("callgrind", 101).into())).unwrap();
        let CommitOutcome::BenchFailed { error } = failed else {
            panic!("expected a bench failure");
        };
        assert!(render_bench_failure(&error).unwrap().contains("101"));
        assert!(error.find_source::<EngineFailedError>().is_some());

        let storage_error = block_on(MemoryStorage::new().get("k")).unwrap_err();
        let infra = map_collect_result(Err(storage_error.into())).unwrap_err();
        assert!(infra.find_source::<StorageError>().is_some());
    }

    #[test]
    fn a_recorded_bench_failure_never_carries_a_backtrace_into_the_summary() {
        // Under `--ignore-errors` the summary is the *success* path, so a per-commit
        // reason is re-rendered inline on its own line. `ohno` appends both
        // `caused by:` and a captured backtrace after the failing error's own
        // message, so a cause whose rendering carries those shapes stands in for a
        // run with `RUST_BACKTRACE` set without the test mutating the environment.
        let parse_failure = io::Error::other(
            "the JSON is malformed\n\nBacktrace:\n   0: cbh_engines::parse_callgrind_summary",
        );
        let outcome = map_collect_result(Err(ParseOutputError::caused_by(
            "target/callgrind/summary.json",
            parse_failure,
        )
        .into()))
        .unwrap();
        let CommitOutcome::BenchFailed { error } = outcome else {
            panic!("expected a bench failure");
        };

        let mut report = BackfillReport::default();
        report.failures.push(FailedCommit {
            commit: "c0ffeec0ffee".to_owned(),
            error,
        });
        let rendered = report.render(1);

        assert!(!rendered.contains("Backtrace:"));
        assert!(!rendered.contains("caused by:"));
        assert!(rendered.contains("failed c0ffeec0ffee (failed to parse benchmark output"));
        // The per-commit note stays on the single line the summary reserves for it.
        assert_eq!(rendered.lines().count(), 2);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "worktree_path reads the wall clock via SystemTime::now"
    )]
    fn worktree_path_is_a_named_scratch_dir_under_temp() {
        // A lib-level assertion on the worktree path (the real `execute_backfill`
        // catches a broken path only by shelling out to `git worktree add`, which
        // hangs on Windows when handed an empty path instead of failing fast).
        let path = worktree_path();

        assert!(
            path.starts_with(std::env::temp_dir()),
            "worktree path should live under the system temp dir: {path:?}"
        );
        let name = path
            .file_name()
            .and_then(|component| component.to_str())
            .unwrap();
        assert!(
            name.starts_with("cargo-bench-history-worktree-"),
            "unexpected worktree name: {name}"
        );
        assert!(
            name.contains(&std::process::id().to_string()),
            "worktree name should embed the process id for uniqueness: {name}"
        );
    }
}
