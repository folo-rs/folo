use crate::harness::*;

/// Blessing a benchmark at HEAD re-baselines its history so a previously flagged
/// regression is no longer reported: `analyze` flags the rising series, a `bless`
/// at the base-branch tip accepts that level, and a second `analyze` over the very
/// same data is silent.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_does_not_reflag_a_blessed_regression() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    // Before blessing, the sustained upward step is a regression.
    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the rising history should flag once");

    // Accept the current level at the base-branch tip.
    let RunOutcome::Completed { message } =
        workspace.drive(&["bless", "nm/nm::observe"]).await.unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("Blessed"), "{message}");

    // The same data is no longer flagged: the blessing re-baselined the series.
    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "a blessed regression must not be re-flagged: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["notable"], false, "{report}");
    assert!(
        parsed["findings"].as_array().is_some_and(Vec::is_empty),
        "{report}"
    );
}

/// A dirty working tree on the base branch does not block blessing: the committed
/// `clean.json` at HEAD is what gets accepted, so `bless` succeeds but prints a
/// warning, and the blessing still re-baselines the series.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn bless_on_a_dirty_base_branch_warns_but_succeeds() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();
    workspace.make_dirty("uncommitted.txt");

    let RunOutcome::Completed { message } =
        workspace.drive(&["bless", "nm/nm::observe"]).await.unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(
        message.contains("Warning: uncommitted changes present"),
        "{message}"
    );
    assert!(message.contains("Blessed"), "{message}");

    // The blessing still took effect: the same data is no longer flagged.
    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 0, "the dirty-tree blessing must still apply");
}

/// `list blessings` reports the sidecar a `bless` just recorded at the current
/// commit, naming the prefix filter it carries.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn list_blessings_reports_the_blessing_recorded_at_head() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();
    let head = workspace.head();

    workspace.drive(&["bless", "nm/nm::observe"]).await.unwrap();

    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "blessings", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected a completed outcome");
    };
    let short_head = head.get(..12).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["scope"], "head", "{message}");
    assert_eq!(parsed["commit"], short_head, "{message}");
    let blessings = parsed["blessings"].as_array().unwrap();
    assert_eq!(blessings.len(), 1, "{message}");
    assert_eq!(blessings[0]["commit"], short_head, "{message}");
    assert!(
        blessings[0]["prefixes"]
            .as_array()
            .is_some_and(|prefixes| prefixes.iter().any(|prefix| prefix == "nm/nm::observe")),
        "{message}"
    );
}

/// `unbless` removes a blessing, so the regression it had re-baselined is reported
/// again — proving the round trip is reversible.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn unbless_restores_a_previously_blessed_regression() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    workspace.drive(&["bless", "nm/nm::observe"]).await.unwrap();

    // The blessing currently suppresses the regression.
    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "the blessing should suppress the regression"
    );

    // Revoking it brings the finding back.
    let RunOutcome::Completed { message } = workspace.drive(&["unbless"]).await.unwrap() else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("Removed"), "{message}");

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 1,
        "the regression returns once the blessing is revoked: {report}"
    );
}

/// Blessing is base-branch-only: attempting it from a feature branch is a hard
/// error with no escape hatch, and records nothing.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn bless_on_a_feature_branch_is_rejected() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.seed_callgrind("2024-01-02", "f1", 130.0);

    let error = workspace
        .drive(&["bless", "nm/nm::observe"])
        .await
        .unwrap_err();
    let RunError::Bless { message } = error else {
        panic!("expected a bless error, got {error:?}");
    };
    assert!(message.contains("not on the base branch"), "{message}");
    // Only the two seeded clean runs exist; no blessing sidecar was written.
    let objects = workspace.stored_objects();
    assert!(
        objects.iter().all(|(key, _)| key.ends_with("clean.json")),
        "no blessing sidecar should be written: {objects:?}"
    );
}

/// A spike that rose and then recovered is suppressed by default (its current
/// level matches the baseline), but `--include-inactive` surfaces it as an
/// inactive finding so a reviewer can audit history that already self-corrected.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_include_inactive_surfaces_a_recovered_spike() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    // A flat baseline, a sustained spike, then a return to the baseline level.
    workspace.seed_callgrind("2024-01-01", "c1", 10.0);
    workspace.seed_callgrind("2024-01-02", "c2", 10.0);
    workspace.seed_callgrind("2024-01-03", "c3", 10.0);
    workspace.seed_callgrind("2024-01-04", "c4", 10.0);
    workspace.seed_callgrind("2024-01-05", "c5", 20.0);
    workspace.seed_callgrind("2024-01-06", "c6", 20.0);
    workspace.seed_callgrind("2024-01-07", "c7", 10.0);
    workspace.seed_callgrind("2024-01-08", "c8", 10.0);
    workspace.seed_callgrind("2024-01-09", "c9", 10.0);
    workspace.seed_callgrind("2024-01-10", "c10", 10.0);

    // By default, a fully recovered spike is not reported.
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["mode"], "history", "{report}");
    assert!(
        parsed["findings"].as_array().is_some_and(Vec::is_empty),
        "a recovered spike is suppressed by default: {report}"
    );

    // Opting in surfaces it as an inactive finding.
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json", "--include-inactive"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    let finding = &parsed["findings"][0];
    assert_eq!(finding["direction"], "regression", "{report}");
    assert_eq!(
        finding["active"], false,
        "a recovered spike is an inactive finding: {report}"
    );
    assert!(
        finding["flipped_at"].is_string(),
        "an inactive finding names the commit it recovered at: {report}"
    );
}
