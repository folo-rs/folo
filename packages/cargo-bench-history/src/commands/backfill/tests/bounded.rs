use super::*;

#[test]
fn bounded_backfill_skips_before_limiting_and_reports_the_whole_range() {
    let git = FakeBackfillGit::new(fixture());
    let runner = FakeCommitRunner::new()
        .complete("f2")
        .complete("c0")
        .complete("c3");
    let mut opts = options("c0", "f2");
    opts.max_commits = NonZeroUsize::new(1);
    let reporter = RecordingReporter::new();

    let RunOutcome::Completed { message } = block_on(execute_backfill(
        &opts,
        &git,
        &runner,
        &worktree(),
        &reporter,
    ))
    .unwrap() else {
        panic!("expected a completed outcome");
    };

    assert_eq!(*runner.ran.borrow(), ["f1"]);
    assert_eq!(*git.resets.borrow(), [(worktree(), "f1".to_owned())]);
    assert_eq!(*git.removed.borrow(), [worktree()]);
    assert!(message.contains(
        "4 commits: 1 stored, 2 skipped (existing), 0 skipped (empty), 0 failed, 1 deferred. Commit limit reached."
    ));
    assert!(reporter.announced("2 already recorded"));
    assert!(reporter.announced("2 pending"));
    assert!(reporter.announced("--max-commits 1 allows at most 1 replay attempt"));
}

#[test]
fn every_runner_outcome_consumes_a_commit_attempt() {
    for outcome in [
        FakeResult::Stored(3),
        FakeResult::SkippedExisting,
        FakeResult::SkippedEmpty,
        FakeResult::BenchFailed,
    ] {
        let git = FakeBackfillGit::new(fixture());
        let runner = FakeCommitRunner::new().with("c2", outcome.clone());
        let commits = ["c2", "c1", "c0"].map(str::to_owned);
        let mut opts = options("c0", "c2");
        opts.max_commits = NonZeroUsize::new(1);
        opts.ignore_errors = true;

        let report = drive_commits(&opts, &git, &runner, &commits).unwrap();

        assert_eq!(*runner.ran.borrow(), ["c2"]);
        assert_eq!(*git.resets.borrow(), [(worktree(), "c2".to_owned())]);
        assert_eq!(report.deferred, 2);
        assert!(report.stopped_failure.is_none());
        assert!(report.render(3).contains("Commit limit reached."));
        assert_eq!(
            report.stored.len(),
            usize::from(matches!(outcome, FakeResult::Stored(_)))
        );
        assert_eq!(
            report.skipped_existing.len(),
            usize::from(matches!(outcome, FakeResult::SkippedExisting))
        );
        assert_eq!(
            report.skipped_empty.len(),
            usize::from(matches!(outcome, FakeResult::SkippedEmpty))
        );
        assert_eq!(
            report.failures.len(),
            usize::from(matches!(outcome, FakeResult::BenchFailed))
        );
    }
}

#[test]
fn bounded_backfill_attempts_n_commits_before_the_next_reset() {
    let git = FakeBackfillGit::new(fixture());
    let runner = FakeCommitRunner::new().complete("c2");
    let commits = ["c3", "c2", "c1", "c0"].map(str::to_owned);
    let mut opts = options("c0", "c3");
    opts.max_commits = NonZeroUsize::new(2);

    let report = drive_commits(&opts, &git, &runner, &commits).unwrap();

    assert_eq!(*runner.ran.borrow(), ["c3", "c1"]);
    assert_eq!(
        *git.resets.borrow(),
        [(worktree(), "c3".to_owned()), (worktree(), "c1".to_owned()),]
    );
    assert_eq!(report.deferred, 1);
    assert_eq!(report.stored.len(), 2);
    assert_eq!(report.skipped_existing, ["c2"]);
}

#[test]
fn bounded_backfill_continues_after_ignored_failure_with_remaining_budget() {
    let git = FakeBackfillGit::new(fixture());
    let runner = FakeCommitRunner::new().with("c2", FakeResult::BenchFailed);
    let commits = ["c2", "c1", "c0"].map(str::to_owned);
    let mut opts = options("c0", "c2");
    opts.max_commits = NonZeroUsize::new(2);
    opts.ignore_errors = true;

    let report = drive_commits(&opts, &git, &runner, &commits).unwrap();

    assert_eq!(*runner.ran.borrow(), ["c2", "c1"]);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.stored, [("c1".to_owned(), 1)]);
    assert_eq!(report.deferred, 1);
    assert!(report.stopped_failure.is_none());
    assert!(report.render(3).contains("Commit limit reached."));
}

#[test]
fn bounded_backfill_exhausts_the_range_when_all_pending_commits_fit() {
    for limit in [1, 2] {
        let git = FakeBackfillGit::new(fixture());
        let runner = FakeCommitRunner::new().complete("c1").complete("c0");
        let commits = ["c2", "c1", "c0"].map(str::to_owned);
        let mut opts = options("c0", "c2");
        opts.max_commits = NonZeroUsize::new(limit);

        let report = drive_commits(&opts, &git, &runner, &commits).unwrap();

        assert_eq!(*runner.ran.borrow(), ["c2"]);
        assert_eq!(report.skipped_existing, ["c1", "c0"]);
        assert_eq!(report.deferred, 0);
        assert!(report.render(3).contains("Range exhausted."));
        assert!(!report.render(3).contains("limit reached"));
    }
}

#[test]
fn bounded_backfill_with_every_commit_recorded_succeeds_without_replay() {
    let git = FakeBackfillGit::new(fixture()).with_reset_failure();
    let runner = FakeCommitRunner::new().complete("c1").complete("c0");
    let mut opts = options("c0", "c1");
    opts.max_commits = NonZeroUsize::new(1);
    let reporter = RecordingReporter::new();

    let RunOutcome::Completed { message } = block_on(execute_backfill(
        &opts,
        &git,
        &runner,
        &worktree(),
        &reporter,
    ))
    .unwrap() else {
        panic!("expected a completed outcome");
    };

    assert!(runner.ran.borrow().is_empty());
    assert!(git.resets.borrow().is_empty());
    assert_eq!(*git.removed.borrow(), [worktree()]);
    assert!(message.contains("0 stored, 2 skipped (existing)"));
    assert!(message.contains("0 deferred. Range exhausted."));
    assert!(reporter.announced("0 pending"));
    assert!(reporter.announced("at most 0 replay attempts"));
}

#[test]
fn bounded_overwrite_starts_at_the_newest_commit_on_every_invocation() {
    let git = FakeBackfillGit::new(fixture());
    let runner = FakeCommitRunner::new().complete("c2");
    let commits = ["c2", "c1", "c0"].map(str::to_owned);
    let mut opts = options("c0", "c2");
    opts.max_commits = NonZeroUsize::new(1);
    opts.overwrite = true;

    for _ in 0..2 {
        let report = drive_commits(&opts, &git, &runner, &commits).unwrap();
        assert_eq!(report.stored, [("c2".to_owned(), 1)]);
        assert!(report.skipped_existing.is_empty());
        assert_eq!(report.deferred, 2);
    }
    assert_eq!(*runner.ran.borrow(), ["c2", "c2"]);
    let summary = scan_outcome_summary(3, 0, true, opts.max_commits);
    assert!(summary.contains("eligible for re-measurement"));
    assert!(summary.contains("at most 1 replay attempt"));
    assert!(!summary.contains("already recorded"));
}

#[test]
fn a_benchmark_failure_takes_precedence_over_the_commit_limit() {
    let git = FakeBackfillGit::new(fixture());
    let runner = FakeCommitRunner::new()
        .with("c2", FakeResult::BenchFailed)
        .complete("c0");
    let commits = ["c2", "c1", "c0"].map(str::to_owned);
    let mut opts = options("c0", "c2");
    opts.max_commits = NonZeroUsize::new(1);

    let report = drive_commits(&opts, &git, &runner, &commits).unwrap();

    assert_eq!(report.stopped_failure, Some(0));
    assert_eq!(report.deferred, 1);
    assert_eq!(report.skipped_existing, ["c0"]);
    assert!(report.render(3).contains("Stopped on benchmark failure."));
    assert!(!report.render(3).contains("Commit limit reached."));
    assert_eq!(*runner.ran.borrow(), ["c2"]);
}

#[test]
fn bounded_backfill_preserves_failure_and_cleanup_errors() {
    for (outcome, fail_remove) in [
        (FakeResult::BenchFailed, false),
        (FakeResult::Infra, false),
        (FakeResult::Stored(1), true),
    ] {
        let mut git = FakeBackfillGit::new(fixture());
        git.fail_remove = fail_remove;
        let runner = FakeCommitRunner::new().with("c1", outcome.clone());
        let mut opts = options("c0", "c1");
        opts.max_commits = NonZeroUsize::new(1);
        // Ignoring per-commit failures never suppresses infrastructure errors.
        opts.ignore_errors = matches!(outcome, FakeResult::Infra);
        let reporter = RecordingReporter::new();

        let error = block_on(execute_backfill(
            &opts,
            &git,
            &runner,
            &worktree(),
            &reporter,
        ))
        .unwrap_err();

        match outcome {
            FakeResult::BenchFailed => {
                assert!(error.find_source::<EngineFailedError>().is_some());
            }
            FakeResult::Infra => assert!(error.find_source::<StorageError>().is_some()),
            _ => assert!(error.find_source::<RemoveWorktreeFailedError>().is_some()),
        }
        assert_eq!(*runner.ran.borrow(), ["c1"]);
        assert_eq!(*git.resets.borrow(), [(worktree(), "c1".to_owned())]);
        assert_eq!(*git.removed.borrow(), [worktree()]);
        assert!(reporter.announced("--max-commits 1"));
    }
}
