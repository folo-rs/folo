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
    let branch_before = workspace.current_branch();
    let head_before = workspace.head();

    let RunOutcome::Completed { message } = workspace.drive(&["backfill", &c1, &c2]).await.unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("2 stored"), "{message}");

    // One clean object per commit, keyed by that commit's full ID. `backfill`
    // auto-detects the target triple, so derive it from a stored object to keep
    // the key assertions correct on every platform CI runs on.
    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 2, "{objects:?}");
    let triple = objects[0].1.context.toolchain.target_triple.clone();
    for commit_id in [&c1, &c2] {
        let expected =
            format!("v1/testproj/objects/callgrind/{triple}/synthetic/{commit_id}/clean.json");
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
    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["runs"], 2,
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
    for commit_id in [&c1, &m, &c3] {
        let expected =
            format!("v1/testproj/objects/callgrind/{triple}/synthetic/{commit_id}/clean.json");
        assert!(
            objects.iter().any(|(key, _)| key == &expected),
            "missing {expected} in {objects:?}"
        );
    }
    // The merged-in side-branch commits are off the first-parent line: nothing is
    // stored for them.
    for commit_id in [&sf1, &sf2] {
        assert!(
            !objects
                .iter()
                .any(|(key, _)| key.contains(commit_id.as_str())),
            "side-branch commit {commit_id} must not be backfilled: {objects:?}"
        );
    }
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

/// `backfill --help` is an early exit whose usage text documents the range
/// positionals.
#[test]
fn backfill_help_documents_range_positionals() {
    let early_exit = Cli::from_args(&["cargo-bench-history"], &["backfill", "--help"]).unwrap_err();
    assert!(early_exit.output.contains("FROM"), "{}", early_exit.output);
    assert!(early_exit.output.contains("TO"), "{}", early_exit.output);
}
