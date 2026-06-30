use crate::harness::*;

/// `prune --dirty` removes the dirty runs a matching `list`/`analyze` would include
/// on the target side of a feature branch, leaving the clean runs untouched. The
/// end state is verified through `list`, proving the production delete path reaches
/// the configured storage.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_dirty_removes_dirty_runs_on_a_feature_branch() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-02", "f1");
    workspace.seed_callgrind("f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-03", "f1", 200.0);

    // Before: the target-side dirty snapshot is part of the data set.
    let message = workspace.drive_json(&["list", "runs"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 3, "{message}");

    // `prune --dirty` removes exactly the one dirty run.
    let message = workspace.drive_json(&["prune", "--dirty"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["dry_run"], false, "{message}");
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");

    // After: only the two clean runs remain; the dirty snapshot is gone.
    let message = workspace.drive_json(&["list", "runs"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 2, "{message}");
    let commits = parsed["sets"][0]["commits"].as_array().unwrap();
    assert!(
        commits.iter().all(|commit| commit["dirty"] == 0),
        "no dirty run remains: {message}"
    );
}

/// The key divergence from `analyze`/`list`: on the base branch with a *clean*
/// working tree, `list` hides the base-tip dirty snapshot, yet `prune --dirty` still
/// removes it (the base-tip exception is unconditional for the dirty scope).
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_dirty_removes_the_base_branch_tip_dirty_with_a_clean_tree() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-02", "c1", 200.0);

    // With a clean working tree, `list` excludes the base-tip dirty snapshot.
    let message = workspace.drive_json(&["list", "runs"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "list hides the base-tip dirty run with a clean tree: {message}"
    );

    // `prune --dirty` removes it anyway (with the base-branch guard confirmed).
    let message = workspace
        .drive_json(&["prune", "--dirty", "--prune-base"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");

    // A second pass finds nothing left to remove.
    let message = workspace
        .drive_json(&["prune", "--dirty", "--prune-base", "--dry-run"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(
        parsed["totals"]["runs"], 0,
        "the base-tip dirty run was already removed: {message}"
    );
}

/// `--dry-run` previews the removal without deleting: it reports the same runs a
/// real `prune` would, but a follow-up real `prune` still finds and removes them.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_dry_run_reports_without_deleting() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-02", "f1");
    workspace.seed_dirty_callgrind("2024-01-02", "f1", 200.0);

    let message = workspace
        .drive_json(&["prune", "--dirty", "--dry-run"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["dry_run"], true, "{message}");
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");

    // The dry run deleted nothing: a real prune still removes the same run.
    let message = workspace.drive_json(&["prune", "--dirty"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["dry_run"], false, "{message}");
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");
}

/// An engine facet scopes the prune: a callgrind-scoped `prune --dirty` leaves a
/// dirty criterion snapshot on the same commit untouched.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_dirty_scopes_by_engine() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-02", "f1");
    workspace.seed_dirty_callgrind("2024-01-02", "f1", 200.0);
    workspace.seed_dirty_criterion("2024-01-02", "f1", "m1", 20.0);

    // Prune only the callgrind set.
    let message = workspace
        .drive_json(&["prune", "--dirty", "--engine", "callgrind"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "only the callgrind dirty run: {message}"
    );
    assert_eq!(parsed["sets"][0]["engine"], "callgrind", "{message}");

    // The criterion dirty snapshot survives: a criterion-scoped prune still finds it
    // once the windows triple and its non-host machine key are named explicitly.
    let message = workspace
        .drive_json(&[
            "prune",
            "--dirty",
            "--engine",
            "criterion",
            "--target-triple",
            "x86_64-pc-windows-msvc",
            "--machine-key",
            "m1",
            "--dry-run",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "criterion dirty survived: {message}"
    );
    assert_eq!(parsed["sets"][0]["engine"], "criterion", "{message}");
}

/// A target-triple facet scopes the prune just like it scopes `analyze`/`list`:
/// the same target commit hosts a dirty Linux (callgrind) run and a dirty Windows
/// (criterion) run, and `--target-triple <linux>` removes only the former.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_dirty_scopes_by_target_triple_facet() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-02", "f1");
    workspace.seed_dirty_callgrind("2024-01-02", "f1", 200.0);
    workspace.seed_dirty_criterion("2024-01-02", "f1", "m1", 20.0);

    // The Linux triple removes only the callgrind (linux) dirty run.
    let message = workspace
        .drive_json(&[
            "prune",
            "--dirty",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");
    assert_eq!(
        parsed["sets"][0]["target_triple"], "x86_64-unknown-linux-gnu",
        "{message}"
    );
    assert_eq!(parsed["sets"][0]["engine"], "callgrind", "{message}");

    // The Windows (criterion) dirty run survives: a Windows-triple pass still finds
    // it once the machine facet is widened to its non-host machine key.
    let message = workspace
        .drive_json(&[
            "prune",
            "--dirty",
            "--target-triple",
            "x86_64-pc-windows-msvc",
            "--machine-key",
            "m1",
            "--dry-run",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "windows dirty survived: {message}"
    );
    assert_eq!(
        parsed["sets"][0]["target_triple"], "x86_64-pc-windows-msvc",
        "{message}"
    );
}

/// `prune` spans every selected discriminant set in one pass: a dirty callgrind and
/// a dirty criterion run on the same target commit form two sets, and an unfiltered
/// `prune --dirty` removes both — exercising the multi-set plan and its plural
/// rendering.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_dirty_removes_runs_across_multiple_discriminant_sets() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-02", "f1");
    workspace.seed_dirty_callgrind("2024-01-02", "f1", 200.0);
    workspace.seed_dirty_criterion("2024-01-02", "f1", "m1", 20.0);

    // Text format exercises the plural "discriminant sets" summary branch. The
    // facets widen to every triple/machine so the synthetic callgrind set and the
    // windows-keyed criterion set are both in scope regardless of host.
    let RunOutcome::Completed { message } = workspace
        .drive(&[
            "prune",
            "--dirty",
            "--target-triple",
            "all",
            "--machine-key",
            "all",
        ])
        .await
        .unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(
        message.contains("Removed 2 runs across 2 discriminant sets"),
        "{message}"
    );

    // Both sets are now empty: a second pass finds nothing to remove.
    let message = workspace
        .drive_json(&[
            "prune",
            "--dirty",
            "--target-triple",
            "all",
            "--machine-key",
            "all",
            "--dry-run",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 0, "{message}");
}

/// `--since` removes only the dirty runs on or after the cutoff — one of the two
/// scopes that read object bodies in production (to recover each run's effective
/// time).
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_dirty_since_only_removes_runs_on_or_after_the_cutoff() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-02", "f1");
    workspace.seed_dirty_callgrind("2024-01-02", "f1", 100.0);
    workspace.commit_dated("2024-01-05", "f2");
    workspace.seed_dirty_callgrind("2024-01-05", "f2", 200.0);

    // Only the 2024-01-05 run is on or after the cutoff.
    let message = workspace
        .drive_json(&["prune", "--dirty", "--since", "2024-01-04"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");

    // The on-or-after run is gone; the earlier run survives the cutoff.
    let message = workspace
        .drive_json(&["prune", "--dirty", "--since", "2024-01-04", "--dry-run"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(
        parsed["totals"]["runs"], 0,
        "the late run was removed: {message}"
    );

    let message = workspace
        .drive_json(&["prune", "--dirty", "--dry-run"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "the early run survived the cutoff: {message}"
    );
}

/// `--until` removes only the runs on or before the cutoff: the mirror of `--since`,
/// also reading object bodies to recover each run's commit time.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_dirty_until_only_removes_runs_on_or_before_the_cutoff() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-02", "f1");
    workspace.seed_dirty_callgrind("2024-01-02", "f1", 100.0);
    workspace.commit_dated("2024-01-05", "f2");
    workspace.seed_dirty_callgrind("2024-01-05", "f2", 200.0);

    // Only the 2024-01-02 run is on or before the cutoff.
    let message = workspace
        .drive_json(&["prune", "--dirty", "--until", "2024-01-03"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");

    // The later run survives the cutoff; only the on-or-before run was removed.
    let message = workspace
        .drive_json(&["prune", "--dirty", "--dry-run"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "the later run survived the cutoff: {message}"
    );
}

/// The `--all` scope deletes clean *and* dirty runs (and their blessing sidecars)
/// for the narrowed selection. A `<commit>` argument selecting one feature commit removes its
/// clean and dirty runs while leaving the base-branch run intact.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_all_removes_clean_and_dirty_for_a_commit() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-02", "f1");
    workspace.seed_callgrind("f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-03", "f1", 200.0);
    let f1 = workspace.commit("f1");

    // Narrowed to f1, the `--all` scope removes its clean and dirty runs.
    let message = workspace.drive_json(&["prune", &f1, "--all"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 2, "clean + dirty f1: {message}");

    // Only the base-branch clean run remains.
    let message = workspace
        .drive_json(&["list", "runs", "--context", "feature"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");
}

/// `prune` requires a scope: invoking it without `--clean`, `--dirty`, or `--all`
/// is rejected at parse time, protecting against an accidental wipe with no intent.
#[test]
fn prune_requires_a_scope() {
    let error = Cli::from_args(&["cargo-bench-history"], &["prune"]).unwrap_err();
    assert!(
        error
            .output
            .contains("the following required arguments were not provided")
            || error.output.contains("required"),
        "the error should name the missing required scope: {}",
        error.output
    );
}

/// `--all` deletes the whole selected data set, but Design B preserves base-branch
/// history: pruning the feature context removes only the feature-unique commits,
/// leaving the base commit at the merge-base intact.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_all_removes_only_the_feature_side() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-02", "f1");
    workspace.seed_callgrind("f1", 100.0);

    let message = workspace
        .drive_json(&["prune", "--all", "--context", "feature"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "only the feature-unique run is deleted: {message}"
    );

    // The base commit survives; only the base-branch baseline remains.
    let message = workspace
        .drive_json(&["list", "runs", "--context", "feature"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "the base-branch run is preserved: {message}"
    );
}

/// `--clean` deletes clean runs (and their blessings) while leaving dirty snapshots
/// in place — the inverse of `--dirty`.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_clean_scope_removes_clean_and_keeps_dirty() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-02", "f1");
    workspace.seed_callgrind("f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-03", "f1", 200.0);
    let f1 = workspace.commit("f1");

    // `--clean` narrowed to f1 removes only its clean run.
    let message = workspace.drive_json(&["prune", &f1, "--clean"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "only the clean f1 run: {message}"
    );

    // The dirty f1 snapshot survives: a `--dirty` pass still finds it.
    let message = workspace
        .drive_json(&["prune", "--dirty", "--dry-run"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 1, "dirty f1 survived: {message}");
}

/// A blessing sidecar follows its clean run: deleting a commit's clean run with the
/// default scope also removes the blessing recorded there.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_removes_a_blessing_with_its_clean_run() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();
    let head = workspace.head();

    workspace.drive(&["bless", "nm/nm::observe"]).await.unwrap();

    // The blessing is recorded at HEAD.
    let message = workspace.drive_json(&["list", "blessings"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(
        parsed["blessings"].as_array().unwrap().len(),
        1,
        "{message}"
    );

    // Pruning HEAD's clean run also deletes the blessing that rode on it. The
    // checkout is the base branch tip, so the base-branch guard must be confirmed.
    let message = workspace
        .drive_json(&["prune", &head, "--all", "--prune-base"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 1, "the clean HEAD run: {message}");
    assert_eq!(parsed["totals"]["blessings"], 1, "{message}");

    // The blessing is gone.
    let message = workspace.drive_json(&["list", "blessings"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert!(
        parsed["blessings"].as_array().unwrap().is_empty(),
        "the blessing was removed with its clean run: {message}"
    );
}

/// Like `analyze`/`list`, `prune` requires a repository to resolve the topology;
/// without one it errors rather than removing nothing silently.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_without_a_repository_errors() {
    let workspace = Workspace::new(&storage_only_config());

    let error = workspace.drive(&["prune", "--dirty"]).await.unwrap_err();
    let RunError::Analyze { message } = error else {
        panic!("expected an analyze error, got {error:?}");
    };
    assert!(message.contains("requires a git repository"), "{message}");
}
