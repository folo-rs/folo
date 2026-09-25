use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::future::{Future, ready};

use cbh_diag::RecordingReporter;
use cbh_git::{FakeGitHistory, GitHistory};
use cbh_storage::{MemoryStorage, StorageError, TestStorageError};
use futures::executor::block_on;

use super::*;
use crate::EngineFailedError;
use crate::model::{MachineKey, TargetTriple};

/// A canned per-commit result the fake [`CommitRunner`] returns.
#[derive(Clone)]
pub(crate) enum FakeResult {
    Stored(usize),
    SkippedExisting,
    SkippedEmpty,
    BenchFailed,
    Infra,
}

/// In-memory [`BackfillGit`] over a [`FakeGitHistory`], recording worktree ops.
pub(crate) struct FakeBackfillGit {
    history: FakeGitHistory,
    pub(crate) added: RefCell<Vec<(PathBuf, String)>>,
    pub(crate) resets: RefCell<Vec<(PathBuf, String)>>,
    pub(crate) removed: RefCell<Vec<PathBuf>>,
    fail_add: bool,
    fail_reset: bool,
    pub(crate) fail_remove: bool,
}

impl FakeBackfillGit {
    pub(crate) fn new(history: FakeGitHistory) -> Self {
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

    pub(crate) fn with_add_failure(mut self) -> Self {
        self.fail_add = true;
        self
    }

    pub(crate) fn with_reset_failure(mut self) -> Self {
        self.fail_reset = true;
        self
    }

    pub(crate) fn with_remove_failure(mut self) -> Self {
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
pub(crate) struct FakeCommitRunner {
    outcomes: HashMap<String, FakeResult>,
    complete: HashSet<String>,
    pub(crate) ran: RefCell<Vec<String>>,
}

impl FakeCommitRunner {
    pub(crate) fn new() -> Self {
        Self {
            outcomes: HashMap::new(),
            complete: HashSet::new(),
            ran: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn with(mut self, commit: &str, result: FakeResult) -> Self {
        self.outcomes.insert(commit.to_owned(), result);
        self
    }

    /// Marks `commit` as already fully recorded, so the pre-run check skips it.
    pub(crate) fn complete(mut self, commit: &str) -> Self {
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
            Some(FakeResult::BenchFailed) => Ok(CommitOutcome::BenchFailed(
                EngineFailedError::new("cargo bench", 1).into(),
            )),
            Some(FakeResult::Infra) => {
                let error = StorageError::from(TestStorageError::new());
                Err(error.into())
            }
            None => Ok(CommitOutcome::Stored { cases: 1 }),
        };
        ready(result)
    }
}

/// `master: c0 - c1 - c2 - c3`, `feature: c1 - f1 - f2`, HEAD at `feature`.
pub(crate) fn fixture() -> FakeGitHistory {
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

pub(crate) fn options(from: &str, to: &str) -> BackfillOptions {
    BackfillOptions {
        from: from.to_owned(),
        to: to.to_owned(),
        ..BackfillOptions::default()
    }
}

pub(crate) fn worktree() -> PathBuf {
    PathBuf::from("/tmp/cargo-bench-history-worktree-test")
}

/// Drives [`run_commits`] over `commits`, discarding the diagnostics the
/// separate reporting tests assert on.
pub(crate) fn drive_commits<G: BackfillGit, C: CommitRunner>(
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

/// The project the storage-scan tests write and read under.
pub(crate) const PROJECT: &str = "proj";

/// The target triple the storage-scan tests treat as this host's.
const TRIPLE: &str = "x86_64-unknown-linux-gnu";

/// The machine key the storage-scan tests treat as this host's. It is a strict
/// prefix of `ci-pool` so the sibling-key cases are meaningful.
pub(crate) const MACHINE: &str = "ci";

/// The partition a backfill would write to on a host with `machine_key`.
pub(crate) fn partition(machine_key: &str) -> Partition {
    Partition {
        target_triple: TargetTriple::from(TRIPLE),
        machine_key: MachineKey::from(machine_key),
    }
}

/// Stores an empty object under `key`, standing in for a recorded result.
pub(crate) fn store(storage: &MemoryStorage, key: &str) {
    block_on(storage.put(key, b"{}")).unwrap();
}

/// The commits `recorded_commits_in` finds for `machine_key`, sorted.
pub(crate) fn recorded(storage: &MemoryStorage, machine_key: &str) -> Vec<String> {
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
