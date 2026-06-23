use crate::harness::*;

/// A backfill stores one clean result per commit in the range, leaves the primary
/// checkout and branch untouched, and the backfilled points then surface through
/// `analyze` in git-topology order.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn backfill_stores_one_clean_object_per_commit_and_restores_checkout() {
    let workspace =
        Workspace::clean_repo(&storage_only_config()).with_bench(&["--summary", "grp=single"]);
    let c1 = workspace.commit("c1");
    let c2 = workspace.commit("c2");
    let c3 = workspace.commit("c3");
    let branch_before = workspace.current_branch();
    let head_before = workspace.head();

    let RunOutcome::Completed { message } = workspace.drive(&["backfill", &c1, &c3]).await.unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("3 stored"), "{message}");

    // One clean object per commit, keyed by that commit's full SHA. `backfill`
    // auto-detects the target triple, so derive it from a stored object to keep
    // the key assertions correct on every platform CI runs on.
    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 3, "{objects:?}");
    let triple = objects[0].1.context.toolchain.target_triple.clone();
    assert!(triple.ends_with("-unknown-linux-gnu"), "{triple}");
    for sha in [&c1, &c2, &c3] {
        let expected = format!("v2/testproj/callgrind/{triple}/synthetic/{sha}/clean.json");
        assert!(
            objects.iter().any(|(key, _)| key == &expected),
            "missing {expected} in {objects:?}"
        );
    }

    // The backfill never touches the primary checkout: HEAD and the branch are
    // exactly as they were before it ran.
    assert_eq!(workspace.current_branch(), branch_before);
    assert_eq!(workspace.head(), head_before);

    // The backfilled points are now visible to `analyze` along the master line.
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["runs"], 3,
        "analyze should see every backfilled commit: {report}"
    );
}

/// Backfill walks the first-parent line across a merge commit: a range spanning a
/// pull-request merge stores one clean object on each first-parent commit
/// (including the merge commit itself) and never on the merged-in side branch's
/// own commits.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn backfill_spans_a_merge_commit_along_first_parent() {
    let workspace =
        Workspace::clean_repo(&storage_only_config()).with_bench(&["--summary", "grp=single"]);
    // master:  root - c1 - M - c3   (M merges the side branch into master)
    //                  \   /
    //  side:            sf1 - sf2
    let c1 = workspace.commit("c1");
    workspace.checkout_new_branch("side");
    let sf1 = workspace.commit("sf1");
    let sf2 = workspace.commit("sf2");
    workspace.checkout("master");
    let m = workspace.merge("side", "M");
    let c3 = workspace.commit("c3");

    let RunOutcome::Completed { message } = workspace.drive(&["backfill", &c1, &c3]).await.unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("3 stored"), "{message}");

    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 3, "{objects:?}");
    let triple = objects[0].1.context.toolchain.target_triple.clone();
    for sha in [&c1, &m, &c3] {
        let expected = format!("v2/testproj/callgrind/{triple}/synthetic/{sha}/clean.json");
        assert!(
            objects.iter().any(|(key, _)| key == &expected),
            "missing {expected} in {objects:?}"
        );
    }
    // The merged-in side-branch commits are off the first-parent line: nothing is
    // stored for them.
    for sha in [&sf1, &sf2] {
        assert!(
            !objects.iter().any(|(key, _)| key.contains(sha.as_str())),
            "side-branch commit {sha} must not be backfilled: {objects:?}"
        );
    }
}

/// Re-running an identical backfill skips every commit whose result already
/// exists (write-once collision), making backfill resumable without duplicating.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn backfill_skips_already_stored_commits_on_rerun() {
    let workspace =
        Workspace::clean_repo(&storage_only_config()).with_bench(&["--summary", "grp=single"]);
    let c1 = workspace.commit("c1");
    let c2 = workspace.commit("c2");

    workspace.drive(&["backfill", &c1, &c2]).await.unwrap();
    assert_eq!(workspace.stored_objects().len(), 2);

    let RunOutcome::Completed { message } = workspace.drive(&["backfill", &c1, &c2]).await.unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(
        message.contains("0 stored, 2 skipped (existing)"),
        "{message}"
    );
    // No new objects were written.
    assert_eq!(workspace.stored_objects().len(), 2);
}

/// The pre-run existence check skips an already-recorded commit *before* it is
/// benchmarked: a re-run whose engine would fail if it were invoked still
/// succeeds, because the recorded commits never reach the (failing) engine.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn backfill_skips_recorded_commits_without_invoking_the_engine() {
    let workspace =
        Workspace::clean_repo(&storage_only_config()).with_bench(&["--summary", "grp=single"]);
    let c1 = workspace.commit("c1");
    let c2 = workspace.commit("c2");

    workspace.drive(&["backfill", &c1, &c2]).await.unwrap();
    assert_eq!(workspace.stored_objects().len(), 2);

    // Re-run with an engine that exits non-zero whenever it actually runs — the
    // `.git` marker is present in every worktree, so `--fail-if-exists .git` fails
    // unconditionally. The pre-run check recognizes both commits as already
    // recorded and skips them before the engine is invoked, so the command still
    // succeeds; without the pre-check the failing engine would run and abort.
    let RunOutcome::Completed { message } = workspace
        .drive_with_bench(
            &["--summary", "grp=single", "--fail-if-exists", ".git"],
            &["backfill", &c1, &c2],
        )
        .await
        .unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(
        message.contains("0 stored, 2 skipped (existing)"),
        "{message}"
    );
    // Nothing changed: the skip path neither re-ran nor rewrote anything.
    assert_eq!(workspace.stored_objects().len(), 2);
}

/// `--overwrite` replaces every already-stored commit in the range rather than
/// skipping it, leaving exactly one clean object per commit.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn backfill_overwrite_replaces_already_stored_commits() {
    let workspace =
        Workspace::clean_repo(&storage_only_config()).with_bench(&["--summary", "grp=single"]);
    let c1 = workspace.commit("c1");
    let c2 = workspace.commit("c2");

    workspace.drive(&["backfill", &c1, &c2]).await.unwrap();

    let RunOutcome::Completed { message } = workspace
        .drive(&["backfill", &c1, &c2, "--overwrite"])
        .await
        .unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("2 stored"), "{message}");
    // Each commit still has exactly one object; nothing was duplicated.
    assert_eq!(workspace.stored_objects().len(), 2);
}

/// A commit that fails to benchmark stops the backfill by default: earlier commits
/// are stored, but the loop halts at the failure and reports a non-zero exit.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn backfill_stops_on_a_failing_commit_by_default() {
    let workspace = Workspace::clean_repo(&storage_only_config()).with_bench(&[
        "--summary",
        "grp=single",
        "--fail-if-exists",
        "BROKEN",
    ]);
    let c1 = workspace.commit("c1");
    workspace.commit_with_file("c2 introduces a broken build", "BROKEN", "boom\n");
    let c3 = workspace.commit_removing_file("c3 fixes the build", "BROKEN");

    let error = workspace.drive(&["backfill", &c1, &c3]).await.unwrap_err();
    let RunError::Backfill { message } = error else {
        panic!("expected a backfill error, got {error:?}");
    };
    assert!(message.contains("stopped at"), "{message}");

    // Only the first (healthy) commit was stored before the stop; c3 never ran.
    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 1, "{objects:?}");
    assert!(objects[0].0.contains(&c1), "{:?}", objects[0].0);
}

/// `--ignore-errors` continues past a failing commit, storing the healthy commits
/// on either side and reporting the failure in the summary.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn backfill_ignore_errors_continues_past_a_failing_commit() {
    let workspace = Workspace::clean_repo(&storage_only_config()).with_bench(&[
        "--summary",
        "grp=single",
        "--fail-if-exists",
        "BROKEN",
    ]);
    let c1 = workspace.commit("c1");
    workspace.commit_with_file("c2 introduces a broken build", "BROKEN", "boom\n");
    let c3 = workspace.commit_removing_file("c3 fixes the build", "BROKEN");

    let outcome = workspace
        .drive(&["backfill", &c1, &c3, "--ignore-errors"])
        .await
        .unwrap();
    // Even though a commit failed to benchmark, `--ignore-errors` makes the
    // overall command succeed (exit code zero); the failure is reported, not fatal.
    assert!(
        outcome.is_success(),
        "--ignore-errors must yield a successful exit despite the failure: {outcome:?}"
    );
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("2 stored"), "{message}");
    assert!(message.contains("1 failed"), "{message}");
    // The two healthy commits are stored; the broken one contributed nothing.
    assert_eq!(workspace.stored_objects().len(), 2);
}

/// A dirty primary working tree does not block backfill: it runs in an isolated
/// worktree and benches the requested commit by SHA, so the primary checkout's
/// state is irrelevant to what gets measured and stored.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn backfill_ignores_a_dirty_primary_working_tree() {
    let workspace =
        Workspace::clean_repo(&storage_only_config()).with_bench(&["--summary", "grp=single"]);
    let c1 = workspace.commit("c1");
    workspace.make_dirty("uncommitted.txt");

    let outcome = workspace.drive(&["backfill", &c1, &c1]).await.unwrap();
    assert!(
        outcome.is_success(),
        "a dirty primary tree must not affect backfill: {outcome:?}"
    );
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("1 stored"), "{message}");
    assert!(!workspace.stored_objects().is_empty());
}

/// A range whose `--from` is not a first-parent ancestor of `--to` (here, a
/// reversed range) is rejected before any worktree work, storing nothing.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn backfill_rejects_a_range_outside_the_first_parent_history() {
    let workspace =
        Workspace::clean_repo(&storage_only_config()).with_bench(&["--summary", "grp=single"]);
    let c1 = workspace.commit("c1");
    let c2 = workspace.commit("c2");

    let error = workspace.drive(&["backfill", &c2, &c1]).await.unwrap_err();
    let RunError::Backfill { message } = error else {
        panic!("expected a backfill error, got {error:?}");
    };
    assert!(message.contains("not a first-parent ancestor"), "{message}");
    assert!(workspace.stored_objects().is_empty());
}

/// `backfill --help` is an early exit whose usage text documents the range
/// positionals.
#[test]
fn backfill_help_documents_range_positionals() {
    let early_exit = Cli::from_args(&["cargo-bench-history"], &["backfill", "--help"]).unwrap_err();
    assert!(early_exit.output.contains("FROM"), "{}", early_exit.output);
    assert!(early_exit.output.contains("TO"), "{}", early_exit.output);
}
