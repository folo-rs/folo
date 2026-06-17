//! The `backfill` command: replay `run` across a range of historical commits.
//!
//! Backfilling bootstraps a history for a repository that adopted the tool late,
//! and supports ad-hoc "what did this look like N commits ago" investigations. It
//! checks out each commit of a range in a dedicated git **worktree** (never the
//! primary checkout) and runs the configured engines there exactly as the `run`
//! command does, recording each commit's committer date as the effective time
//! (see DESIGN §8.5).
//!
//! Like `run`, the orchestration is generic over small ports so the loop logic is
//! exercised with in-memory fakes (Miri-safe): a [`BackfillGit`] port for the git
//! topology and worktree lifecycle, and a [`CommitRunner`] port that runs and
//! stores one commit. The production [`execute`] wires the real adapters; the real
//! [`CommitRunner`] reuses the `run` pipeline ([`run_engines`]) against a
//! worktree-rooted probe, engine runner, and output source.
//!
//! A commit whose two engines disagree on collision (one stores, the other is a
//! duplicate) is reported as skipped-existing; `--overwrite` avoids that by
//! replacing in place.

use std::env::consts;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tick::Clock;

use crate::bench_output::FsBenchOutputSource;
use crate::config::{Config, load_config};
use crate::git_history::{GitHistory, SystemGitHistory};
use crate::probe::SystemProbe;
use crate::process::{TokioBenchRunner, capture};
use crate::storage::{Storage, build_storage};
use crate::wiring::{default_config_path, resolve_project_id};
use crate::{BackfillOptions, RunError, RunOptions, RunOutcome};

use super::run::{RunDeps, RunSummary, run_engines};

/// Read access to a repository's commit topology plus the worktree lifecycle a
/// backfill needs to check out each commit in isolation.
trait BackfillGit {
    /// Whether the primary working tree has no uncommitted changes.
    fn is_clean(&self) -> impl Future<Output = io::Result<bool>>;

    /// Resolves a ref (branch, tag, `HEAD`, or SHA) to its full commit SHA, or
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
    /// Runs the engines in `worktree` and reports the outcome. Recoverable
    /// build/bench failures are reported as [`CommitOutcome::BenchFailed`];
    /// infrastructure failures (storage, git, I/O, configuration) propagate as
    /// `Err` so the backfill aborts regardless of `--ignore-errors`.
    fn run(
        &self,
        worktree: &Path,
        commit: &str,
    ) -> impl Future<Output = Result<CommitOutcome, RunError>>;
}

/// What happened when a single commit was processed.
#[derive(Clone, Debug, Eq, PartialEq)]
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
    BenchFailed {
        /// Human-readable failure description.
        reason: String,
    },
}

/// The real `backfill`: load configuration, wire the production adapters, and
/// orchestrate the range.
pub(crate) async fn execute(options: &BackfillOptions) -> Result<RunOutcome, RunError> {
    let config_path = options
        .config_path
        .clone()
        .unwrap_or_else(default_config_path);
    let config = load_config(&config_path).await?;

    let workspace_dir = std::env::current_dir().map_err(RunError::Io)?;
    let project_id = resolve_project_id(&config, &workspace_dir);
    let storage = build_storage(&config)?;

    let git = SystemBackfillGit::new(&workspace_dir);
    let runner = SystemCommitRunner {
        config: &config,
        project_id: &project_id,
        storage: &storage,
        tool_version: env!("CARGO_PKG_VERSION"),
        options,
    };
    let worktree = worktree_path();

    execute_backfill(options, &git, &runner, &worktree).await
}

/// Plans and runs the backfill against injected ports.
///
/// Validation precedes any worktree work, so a precondition failure leaves the
/// repository untouched. The worktree is always torn down — on success and on
/// failure — and a stop after a per-commit failure surfaces as
/// [`RunError::Backfill`] (a non-zero exit) carrying the partial summary.
async fn execute_backfill<G, C>(
    options: &BackfillOptions,
    git: &G,
    runner: &C,
    worktree: &Path,
) -> Result<RunOutcome, RunError>
where
    G: BackfillGit,
    C: CommitRunner,
{
    let commits = plan_commits(options, git).await?;
    let first = commits
        .first()
        .expect("the planned range always contains at least the --from commit");

    git.add_worktree(worktree, first)
        .await
        .map_err(RunError::Io)?;
    let result = run_commits(options, git, runner, worktree, &commits).await;
    let teardown = git.remove_worktree(worktree).await;

    let report = result?;
    teardown.map_err(RunError::Io)?;

    let message = report.render(commits.len());
    if report.stopped.is_some() {
        Err(RunError::Backfill { message })
    } else {
        Ok(RunOutcome::Completed { message })
    }
}

/// Validates the request and resolves the oldest-first, inclusive commit range.
///
/// Refuses a dirty working tree, requires both endpoints to resolve, requires
/// `--from` to be a first-parent ancestor of `--to`, and requires `--to` to be
/// part of `HEAD`'s history (so `analyze` will surface the backfilled points).
async fn plan_commits<G: BackfillGit>(
    options: &BackfillOptions,
    git: &G,
) -> Result<Vec<String>, RunError> {
    if !git.is_clean().await.map_err(RunError::Io)? {
        return Err(RunError::Backfill {
            message: "the working tree has uncommitted changes; commit or stash them first"
                .to_owned(),
        });
    }

    let from = resolve_required(git, &options.from, "--from").await?;
    let to = resolve_required(git, &options.to, "--to").await?;

    let mut ancestry = git.first_parent(&to).await.map_err(RunError::Io)?;
    let start = ancestry
        .iter()
        .position(|commit| commit == &from)
        .ok_or_else(|| RunError::Backfill {
            message: format!(
                "--from ({}) is not a first-parent ancestor of --to ({})",
                options.from, options.to
            ),
        })?;

    if let Some(head) = git.resolve("HEAD").await.map_err(RunError::Io)? {
        let head_ancestry = git.first_parent(&head).await.map_err(RunError::Io)?;
        if !head_ancestry.iter().any(|commit| commit == &to) {
            return Err(RunError::Backfill {
                message: format!(
                    "--to ({}) is not part of the current branch's history; \
                     check out a branch that contains it before backfilling",
                    options.to
                ),
            });
        }
    }

    Ok(ancestry.split_off(start))
}

/// Resolves `reference` to a commit SHA, mapping an absent ref to a clear error.
async fn resolve_required<G: BackfillGit>(
    git: &G,
    reference: &str,
    flag: &str,
) -> Result<String, RunError> {
    git.resolve(reference)
        .await
        .map_err(RunError::Io)?
        .ok_or_else(|| RunError::Backfill {
            message: format!("cannot resolve {flag} ({reference}) to a commit"),
        })
}

/// Runs each commit of the range in the worktree, aggregating a [`BackfillReport`].
///
/// A per-commit build/bench failure stops the loop unless `--ignore-errors` is
/// set; an infrastructure error always aborts (propagated as `Err`).
async fn run_commits<G, C>(
    options: &BackfillOptions,
    git: &G,
    runner: &C,
    worktree: &Path,
    commits: &[String],
) -> Result<BackfillReport, RunError>
where
    G: BackfillGit,
    C: CommitRunner,
{
    let mut report = BackfillReport::default();
    for commit in commits {
        git.reset_to(worktree, commit).await.map_err(RunError::Io)?;
        match runner.run(worktree, commit).await? {
            CommitOutcome::Stored { cases } => report.stored.push((commit.clone(), cases)),
            CommitOutcome::SkippedExisting => report.skipped_existing.push(commit.clone()),
            CommitOutcome::SkippedEmpty => report.skipped_empty.push(commit.clone()),
            CommitOutcome::BenchFailed { reason } => {
                report.failed.push((commit.clone(), reason));
                if !options.ignore_errors {
                    report.stopped = Some(commit.clone());
                    break;
                }
            }
        }
    }
    Ok(report)
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
    /// Commits that failed to build or benchmark, with the reason.
    failed: Vec<(String, String)>,
    /// The commit the run stopped at after a failure (without `--ignore-errors`).
    stopped: Option<String>,
}

impl BackfillReport {
    /// Renders the multi-line summary for a range of `total` commits.
    fn render(&self, total: usize) -> String {
        let mut lines = vec![format!(
            "Backfill range of {total} commit(s): {} stored, {} skipped (existing), \
             {} skipped (empty), {} failed.",
            self.stored.len(),
            self.skipped_existing.len(),
            self.skipped_empty.len(),
            self.failed.len(),
        )];
        for (commit, cases) in &self.stored {
            lines.push(format!("  stored {} ({cases} case(s))", short(commit)));
        }
        for commit in &self.skipped_existing {
            lines.push(format!("  skipped {} (already stored)", short(commit)));
        }
        for commit in &self.skipped_empty {
            lines.push(format!("  skipped {} (no benchmark cases)", short(commit)));
        }
        for (commit, reason) in &self.failed {
            lines.push(format!("  failed {} ({reason})", short(commit)));
        }
        if let Some(commit) = &self.stopped {
            lines.push(format!(
                "  stopped at {} (pass --ignore-errors to continue past failures)",
                short(commit)
            ));
        }
        lines.join("\n")
    }
}

/// Abbreviates a commit SHA for display, falling back to the full value.
fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// A unique scratch path for the backfill worktree, under the system temp dir.
fn worktree_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    std::env::temp_dir().join(format!(
        "cargo-bench-history-worktree-{}-{nanos}",
        std::process::id()
    ))
}

/// Maps a per-commit `run` result to a [`CommitOutcome`].
///
/// A stored set (or several) is success; a duplicate is a resumable skip; an
/// empty harvest is a non-fatal skip; a build/bench failure is recoverable;
/// everything else (storage, configuration, I/O) is infrastructure and aborts.
fn map_run_result(result: Result<RunSummary, RunError>) -> Result<CommitOutcome, RunError> {
    match result {
        Ok(summary) if summary.stored > 0 => Ok(CommitOutcome::Stored {
            cases: summary.harvested,
        }),
        Ok(_) => Ok(CommitOutcome::SkippedEmpty),
        Err(RunError::Duplicate { .. }) => Ok(CommitOutcome::SkippedExisting),
        Err(
            error @ (RunError::Engine { .. } | RunError::Command { .. } | RunError::Parse { .. }),
        ) => Ok(CommitOutcome::BenchFailed {
            reason: error.to_string(),
        }),
        Err(other) => Err(other),
    }
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
    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; environment IO with no pure logic to assert.
    async fn is_clean(&self) -> io::Result<bool> {
        let repo = self.repo.to_string_lossy().into_owned();
        let output = capture("git", &["-C", repo.as_str(), "status", "--porcelain"]).await?;
        if !output.status.success() {
            return Err(io::Error::other(
                "git status failed; is this a git repository?",
            ));
        }
        Ok(output.stdout.trim().is_empty())
    }

    async fn resolve(&self, reference: &str) -> io::Result<Option<String>> {
        self.history.resolve(reference).await
    }

    async fn first_parent(&self, reference: &str) -> io::Result<Vec<String>> {
        self.history.first_parent(reference).await
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

/// The real [`CommitRunner`], wiring the `run` pipeline against a worktree.
struct SystemCommitRunner<'a, S> {
    /// The loaded configuration (from the invoking checkout, stable per range).
    config: &'a Config,
    /// Resolved project identity for the storage partition.
    project_id: &'a str,
    /// The configured storage backend.
    storage: &'a S,
    /// Version of this tool, recorded with each run.
    tool_version: &'a str,
    /// The backfill options whose engine/triple/machine/overwrite are reused.
    options: &'a BackfillOptions,
}

impl<S: Storage> CommitRunner for SystemCommitRunner<'_, S> {
    #[cfg_attr(test, mutants::skip)] // Wires real adapters; the result mapping is tested via `map_run_result`.
    async fn run(&self, worktree: &Path, _commit: &str) -> Result<CommitOutcome, RunError> {
        let probe = SystemProbe::in_dir(worktree);
        let runner = TokioBenchRunner::in_dir(worktree);
        let target_root = worktree.join("target");
        let output = FsBenchOutputSource::new(target_root.clone());
        let clock = Clock::new_tokio();
        let env = |name: &str| std::env::var(name).ok();

        // A backfilled run is always clean (the worktree is a pristine checkout)
        // and dates from its commit, so no `--timestamp` override is used.
        let run_options = RunOptions {
            config_path: None,
            engine: self.options.engine.clone(),
            timestamp: None,
            target_triple: self.options.target_triple.clone(),
            machine_key: self.options.machine_key.clone(),
            no_store: false,
            overwrite: self.options.overwrite,
            passthrough: self.options.passthrough.clone(),
        };
        let deps = RunDeps {
            runner: &runner,
            probe: &probe,
            output: &output,
            storage: self.storage,
            clock: &clock,
            env: &env,
            config: self.config,
            project_id: self.project_id,
            tool_version: self.tool_version,
            target_root: &target_root,
            host_os: consts::OS,
        };

        map_run_result(run_engines(&run_options, &deps).await)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::future::{Future, ready};

    use futures::executor::block_on;

    use crate::StorageError;
    use crate::git_history::FakeGitHistory;

    use super::*;

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
        clean: bool,
        added: RefCell<Vec<(PathBuf, String)>>,
        resets: RefCell<Vec<(PathBuf, String)>>,
        removed: RefCell<Vec<PathBuf>>,
    }

    impl FakeBackfillGit {
        fn new(history: FakeGitHistory, clean: bool) -> Self {
            Self {
                history,
                clean,
                added: RefCell::new(Vec::new()),
                resets: RefCell::new(Vec::new()),
                removed: RefCell::new(Vec::new()),
            }
        }
    }

    impl BackfillGit for FakeBackfillGit {
        fn is_clean(&self) -> impl Future<Output = io::Result<bool>> {
            ready(Ok(self.clean))
        }

        fn resolve(&self, reference: &str) -> impl Future<Output = io::Result<Option<String>>> {
            self.history.resolve(reference)
        }

        fn first_parent(&self, reference: &str) -> impl Future<Output = io::Result<Vec<String>>> {
            self.history.first_parent(reference)
        }

        fn add_worktree(&self, path: &Path, commit: &str) -> impl Future<Output = io::Result<()>> {
            self.added
                .borrow_mut()
                .push((path.to_owned(), commit.to_owned()));
            ready(Ok(()))
        }

        fn reset_to(&self, path: &Path, commit: &str) -> impl Future<Output = io::Result<()>> {
            self.resets
                .borrow_mut()
                .push((path.to_owned(), commit.to_owned()));
            ready(Ok(()))
        }

        fn remove_worktree(&self, path: &Path) -> impl Future<Output = io::Result<()>> {
            self.removed.borrow_mut().push(path.to_owned());
            ready(Ok(()))
        }
    }

    /// In-memory [`CommitRunner`] returning canned per-commit results.
    struct FakeCommitRunner {
        outcomes: HashMap<String, FakeResult>,
        ran: RefCell<Vec<String>>,
    }

    impl FakeCommitRunner {
        fn new() -> Self {
            Self {
                outcomes: HashMap::new(),
                ran: RefCell::new(Vec::new()),
            }
        }

        fn with(mut self, commit: &str, result: FakeResult) -> Self {
            self.outcomes.insert(commit.to_owned(), result);
            self
        }
    }

    impl CommitRunner for FakeCommitRunner {
        fn run(
            &self,
            _worktree: &Path,
            commit: &str,
        ) -> impl Future<Output = Result<CommitOutcome, RunError>> {
            self.ran.borrow_mut().push(commit.to_owned());
            let result = match self.outcomes.get(commit) {
                Some(FakeResult::Stored(cases)) => Ok(CommitOutcome::Stored { cases: *cases }),
                Some(FakeResult::SkippedExisting) => Ok(CommitOutcome::SkippedExisting),
                Some(FakeResult::SkippedEmpty) => Ok(CommitOutcome::SkippedEmpty),
                Some(FakeResult::BenchFailed(reason)) => Ok(CommitOutcome::BenchFailed {
                    reason: reason.clone(),
                }),
                Some(FakeResult::Infra(message)) => {
                    Err(RunError::Io(io::Error::other(message.clone())))
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

    #[test]
    fn plan_enumerates_inclusive_first_parent_range_oldest_first() {
        let git = FakeBackfillGit::new(fixture(), true);
        let commits = block_on(plan_commits(&options("c1", "f2"), &git)).expect("range plans");
        assert!(
            commits.iter().eq(["c1", "f1", "f2"].iter()),
            "inclusive of both endpoints, oldest first: {commits:?}"
        );
    }

    #[test]
    fn plan_includes_a_single_commit_range() {
        let git = FakeBackfillGit::new(fixture(), true);
        let commits = block_on(plan_commits(&options("f2", "f2"), &git)).expect("range plans");
        assert!(commits.iter().eq(std::iter::once(&"f2")), "{commits:?}");
    }

    #[test]
    fn plan_rejects_a_dirty_working_tree() {
        let git = FakeBackfillGit::new(fixture(), false);
        let error = block_on(plan_commits(&options("c1", "f2"), &git)).expect_err("must refuse");
        let RunError::Backfill { message } = error else {
            panic!("expected a backfill error, got {error:?}");
        };
        assert!(message.contains("uncommitted changes"), "{message}");
    }

    #[test]
    fn plan_rejects_an_unresolvable_endpoint() {
        let git = FakeBackfillGit::new(fixture(), true);
        let error = block_on(plan_commits(&options("absent", "f2"), &git)).expect_err("refuse");
        let RunError::Backfill { message } = error else {
            panic!("expected a backfill error, got {error:?}");
        };
        assert!(message.contains("cannot resolve --from"), "{message}");
    }

    #[test]
    fn plan_rejects_a_from_that_is_not_an_ancestor_of_to() {
        // f1 is on the feature side, not in master's first-parent ancestry.
        let git = FakeBackfillGit::new(fixture(), true);
        let error = block_on(plan_commits(&options("f1", "c3"), &git)).expect_err("refuse");
        let RunError::Backfill { message } = error else {
            panic!("expected a backfill error, got {error:?}");
        };
        assert!(message.contains("not a first-parent ancestor"), "{message}");
    }

    #[test]
    fn plan_rejects_a_to_outside_the_current_branch_history() {
        // HEAD is at feature; c3 (master tip) is not part of feature's history.
        let git = FakeBackfillGit::new(fixture(), true);
        let error = block_on(plan_commits(&options("c0", "c3"), &git)).expect_err("refuse");
        let RunError::Backfill { message } = error else {
            panic!("expected a backfill error, got {error:?}");
        };
        assert!(
            message.contains("not part of the current branch"),
            "{message}"
        );
    }

    #[test]
    fn run_commits_records_every_outcome_kind() {
        let git = FakeBackfillGit::new(fixture(), true);
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

        let report = block_on(run_commits(
            &options("c0", "f2"),
            &git,
            &runner,
            &worktree(),
            &commits,
        ))
        .expect("loop completes");

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
        assert!(report.failed.is_empty());
        assert!(report.stopped.is_none());
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
        let git = FakeBackfillGit::new(fixture(), true);
        let runner = FakeCommitRunner::new().with("c1", FakeResult::BenchFailed("boom".to_owned()));
        let commits = vec!["c0".to_owned(), "c1".to_owned(), "f1".to_owned()];

        let report = block_on(run_commits(
            &options("c0", "f1"),
            &git,
            &runner,
            &worktree(),
            &commits,
        ))
        .expect("loop returns a partial report");

        assert!(
            report
                .stored
                .iter()
                .eq(std::iter::once(&("c0".to_owned(), 1)))
        );
        assert!(
            report
                .failed
                .iter()
                .eq(std::iter::once(&("c1".to_owned(), "boom".to_owned())))
        );
        assert_eq!(report.stopped.as_deref(), Some("c1"));
        // f1 was never reached.
        assert!(runner.ran.borrow().iter().eq(["c0", "c1"].iter()));
    }

    #[test]
    fn run_commits_continues_past_failures_with_ignore_errors() {
        let git = FakeBackfillGit::new(fixture(), true);
        let runner = FakeCommitRunner::new().with("c1", FakeResult::BenchFailed("boom".to_owned()));
        let commits = vec!["c0".to_owned(), "c1".to_owned(), "f1".to_owned()];
        let mut opts = options("c0", "f1");
        opts.ignore_errors = true;

        let report =
            block_on(run_commits(&opts, &git, &runner, &worktree(), &commits)).expect("completes");

        assert!(
            report
                .stored
                .iter()
                .eq([("c0".to_owned(), 1), ("f1".to_owned(), 1)].iter())
        );
        assert!(
            report
                .failed
                .iter()
                .eq(std::iter::once(&("c1".to_owned(), "boom".to_owned())))
        );
        assert!(report.stopped.is_none());
        assert!(runner.ran.borrow().iter().eq(["c0", "c1", "f1"].iter()));
    }

    #[test]
    fn run_commits_aborts_on_infrastructure_error_even_with_ignore_errors() {
        let git = FakeBackfillGit::new(fixture(), true);
        let runner = FakeCommitRunner::new().with("c1", FakeResult::Infra("disk".to_owned()));
        let commits = vec!["c0".to_owned(), "c1".to_owned(), "f1".to_owned()];
        let mut opts = options("c0", "f1");
        opts.ignore_errors = true;

        let error = block_on(run_commits(&opts, &git, &runner, &worktree(), &commits))
            .expect_err("infra error aborts");
        assert!(matches!(error, RunError::Io(_)), "{error:?}");
        // The loop stopped at the failing commit; f1 was never reached.
        assert!(runner.ran.borrow().iter().eq(["c0", "c1"].iter()));
    }

    #[test]
    fn execute_completes_and_tears_down_on_success() {
        let git = FakeBackfillGit::new(fixture(), true);
        let runner = FakeCommitRunner::new();
        let outcome = block_on(execute_backfill(
            &options("c0", "f2"),
            &git,
            &runner,
            &worktree(),
        ))
        .expect("backfill completes");

        let RunOutcome::Completed { message } = outcome else {
            panic!("expected a completed outcome");
        };
        assert!(message.contains("4 stored"), "{message}");
        // The worktree was created once at the first commit and then removed.
        assert!(
            git.added
                .borrow()
                .iter()
                .eq(std::iter::once(&(worktree(), "c0".to_owned())))
        );
        assert!(git.removed.borrow().iter().eq(std::iter::once(&worktree())));
    }

    #[test]
    fn execute_returns_error_and_tears_down_when_stopped() {
        let git = FakeBackfillGit::new(fixture(), true);
        let runner = FakeCommitRunner::new().with("c1", FakeResult::BenchFailed("boom".to_owned()));
        let error = block_on(execute_backfill(
            &options("c0", "f2"),
            &git,
            &runner,
            &worktree(),
        ))
        .expect_err("a stop is an error exit");

        let RunError::Backfill { message } = error else {
            panic!("expected a backfill error, got {error:?}");
        };
        assert!(message.contains("stopped at c1"), "{message}");
        // Teardown still happened despite the failure.
        assert!(git.removed.borrow().iter().eq(std::iter::once(&worktree())));
    }

    #[test]
    fn execute_tears_down_after_an_infrastructure_abort() {
        let git = FakeBackfillGit::new(fixture(), true);
        let runner = FakeCommitRunner::new().with("c0", FakeResult::Infra("disk".to_owned()));
        let error = block_on(execute_backfill(
            &options("c0", "f2"),
            &git,
            &runner,
            &worktree(),
        ))
        .expect_err("infra error aborts");

        assert!(matches!(error, RunError::Io(_)), "{error:?}");
        assert!(git.removed.borrow().iter().eq(std::iter::once(&worktree())));
    }

    #[test]
    fn execute_refuses_dirty_tree_before_touching_the_worktree() {
        let git = FakeBackfillGit::new(fixture(), false);
        let runner = FakeCommitRunner::new();
        let error = block_on(execute_backfill(
            &options("c0", "f2"),
            &git,
            &runner,
            &worktree(),
        ))
        .expect_err("dirty tree refused");

        assert!(matches!(error, RunError::Backfill { .. }), "{error:?}");
        assert!(git.added.borrow().is_empty(), "no worktree created");
        assert!(git.removed.borrow().is_empty(), "nothing to remove");
        assert!(runner.ran.borrow().is_empty(), "no commit run");
    }

    #[test]
    fn map_run_result_classifies_each_run_outcome() {
        let stored = map_run_result(Ok(RunSummary {
            stored: 1,
            harvested: 7,
            labels: Vec::new(),
        }))
        .expect("stored is success");
        assert_eq!(stored, CommitOutcome::Stored { cases: 7 });

        let empty = map_run_result(Ok(RunSummary {
            stored: 0,
            harvested: 0,
            labels: Vec::new(),
        }))
        .expect("empty harvest is a skip");
        assert_eq!(empty, CommitOutcome::SkippedEmpty);

        let duplicate = map_run_result(Err(RunError::Duplicate {
            key: "v2/p/callgrind/t/synthetic/abc/clean.json".to_owned(),
        }))
        .expect("duplicate is a skip");
        assert_eq!(duplicate, CommitOutcome::SkippedExisting);

        let failed = map_run_result(Err(RunError::Engine {
            engine: "callgrind".to_owned(),
            code: Some(101),
        }))
        .expect("engine failure is recoverable");
        let CommitOutcome::BenchFailed { reason } = failed else {
            panic!("expected a bench failure");
        };
        assert!(reason.contains("101"), "{reason}");

        let infra = map_run_result(Err(RunError::Storage(StorageError::NotFound {
            key: "k".to_owned(),
        })));
        assert!(matches!(infra, Err(RunError::Storage(_))), "{infra:?}");
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
            .expect("worktree path should have a UTF-8 file name");
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
