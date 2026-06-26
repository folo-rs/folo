use crate::harness::*;

/// `list discriminants` enumerates exactly the comparable sets present in
/// storage, reporting each set's engine, target triple, and machine key.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn list_discriminants_lists_present_sets() {
    let workspace = Workspace::repo(&storage_only_config());
    // One commit, but two comparable sets: a Linux and a Windows callgrind pool.
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind_in("x86_64-unknown-linux-gnu", "synthetic", "c1", 100.0);
    workspace.seed_callgrind_in("x86_64-pc-windows-msvc", "synthetic", "c1", 100.0);

    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "discriminants", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    let sets = parsed.as_array().unwrap();
    assert_eq!(sets.len(), 2, "exactly the two present sets: {message}");
    let triples: Vec<&str> = sets
        .iter()
        .map(|set| set["target_triple"].as_str().unwrap())
        .collect();
    assert!(triples.contains(&"x86_64-unknown-linux-gnu"), "{message}");
    assert!(triples.contains(&"x86_64-pc-windows-msvc"), "{message}");
    assert!(
        sets.iter().all(|set| set["engine"] == "callgrind"),
        "{message}"
    );
}

/// `list` previews exactly the data set the matching `analyze` would consume:
/// per discriminant set, the run, series, and per-commit counts of the selected
/// runs, ordered oldest-first by git topology.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn list_previews_the_analyzed_data_set() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "runs", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 6, "{message}");
    assert_eq!(parsed["totals"]["series"], 1, "{message}");
    assert_eq!(parsed["totals"]["discriminant_sets"], 1, "{message}");

    let sets = parsed["sets"].as_array().unwrap();
    assert_eq!(sets.len(), 1, "{message}");
    assert_eq!(sets[0]["engine"], "callgrind", "{message}");
    assert_eq!(sets[0]["runs"], 6, "{message}");

    let commits = sets[0]["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 6, "one run per commit: {message}");
    // The seeded commits, oldest first by topology, each one clean run.
    assert!(
        commits
            .iter()
            .all(|commit| commit["runs"] == 1 && commit["clean"] == 1 && commit["dirty"] == 0),
        "{message}"
    );
}

/// `list` mirrors `analyze`'s facet selection: with two comparable sets present, a
/// `--target-triple` filter previews only the matching set, exactly as `analyze`
/// would scope it.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn list_facet_selection_mirrors_analyze() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind_in("x86_64-unknown-linux-gnu", "synthetic", "c1", 100.0);
    workspace.seed_callgrind_in("x86_64-pc-windows-msvc", "synthetic", "c1", 50.0);

    let RunOutcome::Completed { message } = workspace
        .drive(&[
            "list",
            "runs",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--format",
            "json",
        ])
        .await
        .unwrap()
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["discriminant_sets"], 1, "{message}");
    assert_eq!(
        parsed["sets"][0]["target_triple"], "x86_64-unknown-linux-gnu",
        "{message}"
    );
}

/// `list` admits and excludes dirty snapshots exactly as `analyze` does: on a
/// feature branch a dirty run on the target side is previewed by default and
/// dropped under `--no-dirty`.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn list_admits_and_excludes_dirty_like_analyze() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-02", "f1");
    workspace.seed_callgrind("f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-03", "f1", 200.0);

    // By default the dirty snapshot on the target side is included: f1 hosts both a
    // clean and a dirty run, for three runs across two commits.
    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "runs", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 3, "{message}");
    let commits = parsed["sets"][0]["commits"].as_array().unwrap();
    let f1 = commits.last().unwrap();
    assert_eq!(f1["runs"], 2, "{message}");
    assert_eq!(f1["clean"], 1, "{message}");
    assert_eq!(f1["dirty"], 1, "{message}");

    // `--no-dirty` drops the dirty snapshot, leaving only the two clean runs.
    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "runs", "--no-dirty", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 2, "{message}");
}

/// Like `analyze`, `list` requires a repository to resolve the timeline from git
/// topology; without one it errors rather than reporting an empty data set.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn list_without_a_repository_errors() {
    let workspace = Workspace::new(&storage_only_config());

    let error = workspace.drive(&["list", "runs"]).await.unwrap_err();
    let RunError::Analyze { message } = error else {
        panic!("expected an analyze error, got {error:?}");
    };
    assert!(message.contains("requires a git repository"), "{message}");
}

/// `--all` is a `list blessings`-only switch; passing it to another subject is a
/// clear up-front error rather than a silently ignored flag.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn list_runs_rejects_the_blessings_only_all_switch() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);

    let error = workspace
        .drive(&["list", "runs", "--all"])
        .await
        .unwrap_err();
    let RunError::Analyze { message } = error else {
        panic!("expected an analyze error, got {error:?}");
    };
    assert!(message.contains("--all"), "{message}");
    assert!(message.contains("list blessings"), "{message}");
}
