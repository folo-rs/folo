use super::*;

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
    let runner = FakeCommitRunner::new().with("c1", FakeResult::BenchFailed);
    let commits = vec!["c0".to_owned(), "c1".to_owned(), "f1".to_owned()];

    let report = drive_commits(&options("c0", "f1"), &git, &runner, &commits).unwrap();

    assert!(
        report
            .stored
            .iter()
            .eq(std::iter::once(&("c0".to_owned(), 1)))
    );
    assert_eq!(report.failures.len(), 1);
    let recorded = report.failures.first().unwrap();
    assert_eq!(recorded.commit, "c1");
    let failure = recorded.error.find_source::<EngineFailedError>().unwrap();
    assert_eq!(failure.engine(), "cargo bench");
    assert_eq!(failure.code(), 1);
    assert_eq!(report.stopped_failure, Some(0));
    // f1 was never reached.
    assert!(runner.ran.borrow().iter().eq(["c0", "c1"].iter()));
}

#[test]
fn run_commits_continues_past_failures_with_ignore_errors() {
    let git = FakeBackfillGit::new(fixture());
    let runner = FakeCommitRunner::new().with("c1", FakeResult::BenchFailed);
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
    let recorded = report.failures.first().unwrap();
    assert_eq!(recorded.commit, "c1");
    assert!(recorded.error.find_source::<EngineFailedError>().is_some());
    assert!(report.stopped_failure.is_none());
    assert!(runner.ran.borrow().iter().eq(["c0", "c1", "f1"].iter()));
}

#[test]
fn run_commits_aborts_on_infrastructure_error_even_with_ignore_errors() {
    let git = FakeBackfillGit::new(fixture());
    let runner = FakeCommitRunner::new().with("c1", FakeResult::Infra);
    let commits = vec!["c0".to_owned(), "c1".to_owned(), "f1".to_owned()];
    let mut opts = options("c0", "f1");
    opts.ignore_errors = true;

    let error = drive_commits(&opts, &git, &runner, &commits).unwrap_err();
    assert!(BenchFailure::find(&error).is_none());
    assert!(error.find_source::<StorageError>().is_some());
    // The loop stopped at the failing commit; f1 was never reached.
    assert!(runner.ran.borrow().iter().eq(["c0", "c1"].iter()));
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
    // The scan explains why expensive work will or will not run.
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
            && line.contains("2 pending")),
        "{announcements:?}"
    );
    // Progress is visible per commit under --verbose, so a run cut short still
    // shows how far the skipping got.
    assert!(reporter.contains("skipping c1"), "{:?}", reporter.notes());
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
    let runner = FakeCommitRunner::new().with("c1", FakeResult::BenchFailed);
    let error = block_on(execute_backfill(
        &options("c0", "f2"),
        &git,
        &runner,
        &worktree(),
        &RecordingReporter::new(),
    ))
    .unwrap_err();

    assert!(error.find_source::<BackfillError>().is_some());
    assert!(error.find_source::<EngineFailedError>().is_some());
    // Teardown still happened despite the failure.
    assert!(git.removed.borrow().iter().eq(std::iter::once(&worktree())));
}

#[test]
fn execute_tears_down_after_an_infrastructure_abort() {
    let git = FakeBackfillGit::new(fixture());
    let runner = FakeCommitRunner::new().with("c0", FakeResult::Infra);
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
