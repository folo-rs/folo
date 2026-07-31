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
    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 1,
        "the rising history should flag once"
    );

    // Accept the current level at the base-branch tip.
    let RunOutcome::Completed { message } =
        workspace.drive(&["bless", "nm/nm::observe"]).await.unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("Blessed"), "{message}");

    // The same data is no longer flagged: the blessing re-baselined the series.
    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 0,
        "a blessed regression must not be re-flagged: {report}"
    );
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
    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 0,
        "the dirty-tree blessing must still apply"
    );
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

    let message = workspace.drive_json(&["list", "blessings"]).await;
    let short_head = head.get(..12).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["scope"], "head", "{message}");
    assert_eq!(parsed["commit"], short_head, "{message}");
    let blessings = parsed["blessings"].as_array().unwrap();
    assert_eq!(blessings.len(), 1, "{message}");
    assert_eq!(blessings[0]["commit"], short_head, "{message}");
    // The blessed commit's committer date is read from git topology (HEAD is the
    // seeded tip c10, dated 2024-01-10), not from a denormalized copy on the sidecar.
    assert_eq!(
        blessings[0]["commit_time"], "2024-01-10T00:00:00Z",
        "{message}"
    );
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
    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 0,
        "the blessing should suppress the regression"
    );

    // Revoking it brings the finding back.
    let RunOutcome::Completed { message } = workspace.drive(&["unbless"]).await.unwrap() else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("Removed"), "{message}");

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 1,
        "the regression returns once the blessing is revoked: {report}"
    );
}

/// Blessing off the base branch is allowed but warns: the sidecar is still written,
/// with an explanatory note that it only takes effect once the commit joins the base
/// branch's first-parent history (for example after a fast-forward).
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn bless_on_a_feature_branch_warns_but_succeeds() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-02", "f1");
    workspace.seed_callgrind("f1", 130.0);

    let RunOutcome::Completed { message } =
        workspace.drive(&["bless", "nm/nm::observe"]).await.unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("not on the base branch"), "{message}");
    assert!(message.contains("first-parent history"), "{message}");
    assert!(message.contains("Blessed"), "{message}");

    // The blessing sidecar is still recorded at the off-base commit (the clean run
    // present at f1 anchors it), so `list blessings` at HEAD finds it.
    let listing = workspace.drive_json(&["list", "blessings"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&listing).unwrap();
    let blessings = parsed["blessings"].as_array().unwrap();
    assert_eq!(
        blessings.len(),
        1,
        "the off-base blessing is still recorded: {listing}"
    );
}

/// The off-base check follows *first-parent* history, not mere ancestry. A commit
/// merged in as a second parent is reachable from the base branch, yet it is not on
/// the base branch's official (first-parent) line, so `analyze` would ignore a
/// blessing there. Blessing such a commit must therefore warn, even though a plain
/// ancestry check would consider it "on the base branch."
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn bless_on_a_merged_in_side_commit_warns_off_first_parent() {
    let workspace = Workspace::repo(&storage_only_config());
    // master:  root - c1 - M - c3   (M merges the side branch in)
    //                  \   /
    //  side:            sf1
    workspace.commit("c1");
    workspace.checkout_new_branch("side");
    workspace.commit("sf1");
    workspace.checkout("master");
    workspace.merge("side", "M");
    workspace.commit("c3");

    // A clean run anchors the blessing at the second-parent commit sf1.
    workspace.seed_callgrind("sf1", 100.0);

    // Bless with HEAD on sf1: reachable from master (via M's second parent) but off
    // master's first-parent line.
    workspace.checkout("side");
    let RunOutcome::Completed { message } =
        workspace.drive(&["bless", "nm/nm::observe"]).await.unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("not on the base branch"), "{message}");
    assert!(message.contains("first-parent history"), "{message}");
    assert!(message.contains("Blessed"), "{message}");

    // The sidecar is still written at sf1.
    let listing = workspace.drive_json(&["list", "blessings"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&listing).unwrap();
    assert_eq!(
        parsed["blessings"].as_array().unwrap().len(),
        1,
        "the off-first-parent blessing is still recorded: {listing}"
    );
}
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn bless_before_capture_applies_once_the_data_lands() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    // Build the full commit history first, capturing no data yet. The chain is
    // `MIN_SERIES_POINTS` long so the later-landing step is a real regression the
    // blessing must suppress (not one too short to flag regardless).
    let dates = sequential_dates("2024-01-01", MIN_SERIES_POINTS);
    for (index, date) in dates.iter().enumerate() {
        workspace.commit_dated(date, &format!("c{}", index + 1));
    }

    // Pre-emptively accept the tip before any run exists there. With no data to
    // anchor to, the target sets are synthesized from the resolved facets (every
    // engine on this host).
    let RunOutcome::Completed { message } = workspace.drive(&["bless", "--all"]).await.unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("no stored result"), "{message}");
    assert!(message.contains("Blessed"), "{message}");

    // Now capture the rising Callgrind history over those same commits.
    for (index, _) in dates.iter().enumerate() {
        let value = if index < MIN_REGIME { 100.0 } else { 130.0 };
        workspace.seed_callgrind(&format!("c{}", index + 1), value);
    }

    // The pre-emptive blessing occupies the exact Callgrind set the run landed in,
    // so the regression is suppressed with no re-bless.
    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 0,
        "a pre-emptive blessing must apply once the data lands: {report}"
    );
}

/// A spike that rose and then recovered is suppressed by default (its current
/// level matches the baseline), but `--include-inactive` surfaces it as an
/// inactive finding so a reviewer can audit history that already self-corrected.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_include_inactive_surfaces_a_recovered_spike() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    // A `MIN_REGIME`-point baseline, a `MIN_REGIME`-point spike, then a
    // `MIN_REGIME`-point return to the baseline level. Each regime must be at least
    // `MIN_REGIME` points long for the change-point detector to trust both the rise
    // and the recovery, so the spike registers as an inactive (self-corrected)
    // finding.
    for (day, value) in std::iter::repeat_n(10.0, MIN_REGIME)
        .chain(std::iter::repeat_n(20.0, MIN_REGIME))
        .chain(std::iter::repeat_n(10.0, MIN_REGIME))
        .enumerate()
    {
        let day = day + 1;
        let label = format!("c{day}");
        workspace.commit_dated(&format!("2024-01-{day:02}"), &label);
        workspace.seed_callgrind(&label, value);
    }

    // By default, a fully recovered spike is not reported.
    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["mode"], "history", "{report}");
    assert!(
        parsed["findings"].as_array().is_some_and(Vec::is_empty),
        "a recovered spike is suppressed by default: {report}"
    );

    // Opting in surfaces it as an inactive finding.
    let report = workspace
        .drive_json(&["analyze", "--include-inactive"])
        .await;
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
