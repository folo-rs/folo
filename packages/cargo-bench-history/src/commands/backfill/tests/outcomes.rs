use super::*;

#[test]
fn missing_historical_directories_are_per_commit_failures() {
    for directory in [
        Ok(false),
        Err(io::ErrorKind::NotFound.into()),
        Err(io::ErrorKind::NotADirectory.into()),
    ] {
        let error =
            check_project_directory(Path::new("historical-project"), directory).unwrap_err();
        let outcome = map_collect_result(Err(error)).unwrap();
        let CommitOutcome::BenchFailed(error) = outcome else {
            panic!("expected a per-commit failure");
        };
        assert!(
            error
                .find_source::<MissingProjectDirectoryError>()
                .is_some()
        );
    }
}

#[test]
fn available_historical_directory_passes_without_changing_io_failures() {
    check_project_directory(Path::new("project"), Ok(true)).unwrap();
    let error = check_project_directory(
        Path::new("project"),
        Err(io::ErrorKind::PermissionDenied.into()),
    )
    .unwrap_err();
    assert!(BenchFailure::find(&error).is_none());
    assert_eq!(
        error.find_source::<io::Error>().unwrap().kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[test]
fn scan_outcome_summary_states_the_split_and_the_rule_behind_it() {
    let partial = scan_outcome_summary(10, 4, false, None);
    assert!(
        partial.contains("backfilling 10 commits, newest first"),
        "{partial}"
    );
    assert!(partial.contains("4 already recorded"), "{partial}");
    assert!(partial.contains("6 pending"), "{partial}");
    // The flag that changes the decision is named, so a surprising count is
    // actionable from this line alone.
    assert!(partial.contains("--overwrite"), "{partial}");

    // A range with nothing left to do still says so rather than staying silent.
    let complete = scan_outcome_summary(10, 10, false, None);
    assert!(complete.contains("0 pending"), "{complete}");

    // With --overwrite the pre-check never ran, so no count is invented for it.
    let overwriting = scan_outcome_summary(10, 0, true, None);
    assert!(
        overwriting.contains("--overwrite disables the skip pre-check"),
        "{overwriting}"
    );
    assert!(!overwriting.contains("to measure"), "{overwriting}");
    assert!(!overwriting.contains("already recorded"));
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
            error: EngineFailedError::new("cargo bench", 1).into(),
        }],
        stopped_failure: Some(0),
        deferred: 1,
    };

    let rendered = report.render(5);

    assert!(
        rendered.contains(
            "Backfill range of 5 commits: 1 stored, 1 skipped (existing), \
             1 skipped (empty), 1 failed, 1 deferred. Stopped on benchmark failure."
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
        rendered.contains("  failed ddddddd (engine \"cargo bench\" failed with exit code 1)"),
        "{rendered}"
    );
    assert!(
        rendered.contains("  stopped at ddddddd (pass --ignore-errors to continue past failures)"),
        "{rendered}"
    );
}

#[test]
fn stopping_error_comes_from_the_explicit_failure_entry() {
    let mut report = BackfillReport {
        failures: vec![
            FailedCommit {
                commit: "c0".to_owned(),
                error: EngineFailedError::new("criterion", 1).into(),
            },
            FailedCommit {
                commit: "c1".to_owned(),
                error: ParseOutputError::caused_by(
                    "target/callgrind/summary.json",
                    io::Error::other("invalid summary"),
                )
                .into(),
            },
        ],
        stopped_failure: Some(1),
        ..BackfillReport::default()
    };

    let error = report.take_stopping_error().unwrap();
    let failure = error.find_source::<ParseOutputError>().unwrap();

    assert_eq!(failure.path(), Path::new("target/callgrind/summary.json"));
    assert!(error.find_source::<io::Error>().is_some());
    assert!(error.find_source::<EngineFailedError>().is_none());
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures.first().unwrap().commit, "c0");
    assert!(report.stopped_failure.is_none());
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

    let failed = map_collect_result(Err(EngineFailedError::new("callgrind", 101).into())).unwrap();
    let CommitOutcome::BenchFailed(error) = failed else {
        panic!("expected a bench failure");
    };
    let failure = error.find_source::<EngineFailedError>().unwrap();
    assert_eq!(failure.engine(), "callgrind");
    assert_eq!(failure.code(), 101);

    let storage_error = block_on(MemoryStorage::new().get("k")).unwrap_err();
    let infra = map_collect_result(Err(storage_error.into())).unwrap_err();
    assert!(infra.find_source::<StorageError>().is_some());
}

#[test]
fn a_recorded_bench_failure_never_carries_a_backtrace_into_the_summary() {
    // Under `--ignore-errors` the summary is the *success* path, so a per-commit
    // reason is rendered from the typed failure's fields. The full source chain
    // stays attached to the retained error rather than entering that line.
    let parse_failure = io::Error::other(
        "the JSON is malformed\n\nBacktrace:\n   0: cbh_engines::parse_callgrind_summary",
    );
    let outcome = map_collect_result(Err(ParseOutputError::caused_by(
        "target/callgrind/summary.json",
        parse_failure,
    )
    .into()))
    .unwrap();
    let CommitOutcome::BenchFailed(error) = outcome else {
        panic!("expected a bench failure");
    };
    let failure = error.find_source::<ParseOutputError>().unwrap();
    assert_eq!(failure.path(), Path::new("target/callgrind/summary.json"));
    assert!(error.find_source::<io::Error>().is_some());

    let mut report = BackfillReport::default();
    report.failures.push(FailedCommit {
        commit: "c0ffeec0ffee".to_owned(),
        error,
    });
    let rendered = report.render(1);

    assert!(!rendered.contains("Backtrace:"));
    assert!(!rendered.contains("caused by:"));
    assert!(rendered.contains("target/callgrind/summary.json"));
    // The per-commit note stays on the single line the summary reserves for it.
    assert_eq!(rendered.lines().count(), 2);
}
