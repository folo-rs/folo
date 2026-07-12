use crate::harness::*;

/// `examine` pivots one `(benchmark, metric)` series into its raw per-commit data
/// points: one JSON row per recorded observation, oldest first by git topology,
/// pairing each value with the commit it was measured against and that commit's
/// title.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_pivots_a_rising_history_into_points() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let message = workspace
        .drive_json(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["benchmark"], "nm/nm::observe/pull", "{message}");
    assert_eq!(parsed["metric"], "instruction_count", "{message}");

    let sets = parsed["sets"].as_array().unwrap();
    assert_eq!(sets.len(), 1, "one comparable set: {message}");
    let points = sets[0]["points"].as_array().unwrap();
    assert_eq!(points.len(), 6, "one point per commit: {message}");

    // The values follow the seeded rising history in topology order, each paired
    // with its commit's title (the commit message the harness stamps).
    let values: Vec<f64> = points
        .iter()
        .map(|point| point["value"].as_f64().unwrap())
        .collect();
    assert_eq!(
        values,
        vec![100.0, 100.0, 100.0, 130.0, 130.0, 130.0],
        "{message}"
    );
    let titles: Vec<&str> = points
        .iter()
        .map(|point| point["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        vec!["c1", "c2", "c3", "c4", "c5", "c6"],
        "{message}"
    );
    assert!(
        points.iter().all(|point| point["dirty"] == false),
        "every seeded run is clean: {message}"
    );
    // Every point carries the full commit ID it was measured against.
    assert!(
        points
            .iter()
            .all(|point| point["commit"].as_str().unwrap().len() == 40),
        "full commit ids: {message}"
    );
}

/// The default text pivot pairs each value with the short commit id and the start
/// of the commit's title, so a maintainer reading `examine` can correlate a value
/// change with the commit that caused it.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_renders_a_text_pivot_with_titles() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let RunOutcome::Completed { message } = workspace
        .drive(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
        ])
        .await
        .unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(
        message.contains("Data points for nm/nm::observe/pull metric instruction_count"),
        "{message}"
    );
    // The short commit id abbreviates each commit ID to twelve characters.
    let head = workspace.head_commit_id();
    let short_head: String = head.chars().take(12).collect();
    assert!(
        message.contains(&short_head),
        "short head id present: {message}"
    );
    // Values and their titles both appear.
    assert!(message.contains("100"), "{message}");
    assert!(message.contains("130"), "{message}");
    assert!(
        message.contains("c6"),
        "the tip's title is shown: {message}"
    );
    // The series is charted before its data points: the rasciigraph axis marker
    // appears ahead of the first point row. Match c1's title with its leading
    // column gap and trailing newline so the short commit hash (which can contain
    // "c1") cannot be mistaken for the point row.
    let axis = message
        .find('┤')
        .or_else(|| message.find('┼'))
        .expect("a chart leads the points");
    let first_point = message
        .find("  c1\n")
        .expect("the first point row is present");
    assert!(axis < first_point, "a chart leads the points: {message}");
}

/// `examine` renders a per-set Markdown table with a row per observation.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_renders_a_markdown_table() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 130.0);

    let markdown = workspace
        .drive_markdown(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
        ])
        .await;
    assert!(
        markdown.contains("# Data points for nm/nm::observe/pull"),
        "{markdown}"
    );
    let table = markdown
        .find("| Commit | Value | Kind | Title |")
        .expect("the table header is present");
    assert!(markdown.contains("| clean | c1 |"), "{markdown}");
    assert!(markdown.contains("130"), "{markdown}");
    // The two-point series is charted, fenced as a `text` block before the table.
    assert!(
        markdown.contains('┤') || markdown.contains('┼'),
        "a chart is drawn: {markdown}"
    );
    let fence = markdown.find("```text").expect("the chart is fenced");
    assert!(fence < table, "the chart precedes the table: {markdown}");
}

/// `examine` names one metric, isolating it from the other metrics recorded on the
/// same benchmark: only the requested metric's values appear.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_selects_only_the_named_metric() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    // One benchmark carrying two metrics on the same run.
    workspace.seed_metrics(
        "c1",
        vec![
            Metric::new(MetricKind::InstructionCount, 100.0),
            Metric::new(MetricKind::ConditionalBranches, 42.0),
        ],
    );

    let message = workspace
        .drive_json(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "conditional_branches",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["metric"], "conditional_branches", "{message}");
    let points = parsed["sets"][0]["points"].as_array().unwrap();
    assert_eq!(points.len(), 1, "{message}");
    assert_eq!(
        points[0]["value"].as_f64().unwrap(),
        42.0,
        "the conditional_branches value, not the instruction count: {message}"
    );
}

/// Like `analyze`, `examine` repeats per matching discriminant set and honors the
/// facet filters: with two comparable pools present, a `--target-triple` scopes the
/// pivot to just the matching set.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_facet_selection_mirrors_analyze() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind_in("x86_64-unknown-linux-gnu", "synthetic", "c1", 100.0);
    workspace.seed_callgrind_in("x86_64-pc-windows-msvc", "synthetic", "c1", 50.0);

    // Unfiltered, both comparable sets pivot.
    let message = workspace
        .drive_json(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["sets"].as_array().unwrap().len(), 2, "{message}");

    // `--target-triple` narrows to the Linux pool only.
    let message = workspace
        .drive_json(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    let sets = parsed["sets"].as_array().unwrap();
    assert_eq!(sets.len(), 1, "{message}");
    assert_eq!(
        sets[0]["target_triple"], "x86_64-unknown-linux-gnu",
        "{message}"
    );
    assert_eq!(
        sets[0]["points"][0]["value"].as_f64().unwrap(),
        100.0,
        "{message}"
    );
}

/// `examine` pivots a Criterion `wall_time` series just as it does a Callgrind one,
/// proving it reads any engine's reconstructed series and any metric name.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_pivots_a_criterion_wall_time_series() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-02-01", "d1");
    workspace.seed_criterion("d1", "mk-fixed", 20.0);
    workspace.commit_dated("2024-02-02", "d2");
    workspace.seed_criterion("d2", "mk-fixed", 30.0);

    let message = workspace
        .drive_json(&[
            "examine",
            "--benchmark",
            "time/capture/std_instant",
            "--metric",
            "wall_time",
            "--target-triple",
            "x86_64-pc-windows-msvc",
            "--machine-key",
            "mk-fixed",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["metric"], "wall_time", "{message}");
    let sets = parsed["sets"].as_array().unwrap();
    assert_eq!(sets.len(), 1, "{message}");
    assert_eq!(sets[0]["engine"], "criterion", "{message}");
    let values: Vec<f64> = sets[0]["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|point| point["value"].as_f64().unwrap())
        .collect();
    assert_eq!(values, vec![20.0, 30.0], "{message}");
}

/// On a feature branch a dirty snapshot on the target tip pivots by default,
/// ordered after the commit's clean run and flagged; `--no-dirty` drops it — the
/// same admission `analyze`/`list` apply.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_admits_and_flags_dirty_on_a_feature_branch() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-02", "f1");
    workspace.seed_callgrind("f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-03", "f1", 200.0);

    let message = workspace
        .drive_json(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    let points = parsed["sets"][0]["points"].as_array().unwrap();
    // c1 clean, then f1's clean run before its dirty snapshot.
    assert_eq!(points.len(), 3, "{message}");
    assert_eq!(
        points[1]["dirty"], false,
        "clean before dirty on f1: {message}"
    );
    assert_eq!(points[2]["dirty"], true, "{message}");
    assert_eq!(points[2]["value"].as_f64().unwrap(), 200.0, "{message}");

    // The text pivot flags the dirty row.
    let RunOutcome::Completed { message } = workspace
        .drive(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
        ])
        .await
        .unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("(dirty)"), "{message}");

    // `--no-dirty` drops the dirty snapshot, leaving only the two clean runs.
    let message = workspace
        .drive_json(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
            "--no-dirty",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    let points = parsed["sets"][0]["points"].as_array().unwrap();
    assert_eq!(points.len(), 2, "{message}");
    assert!(
        points.iter().all(|point| point["dirty"] == false),
        "{message}"
    );
}

/// A genuinely dirty working tree on the base branch admits the tip's dirty
/// snapshot via the base exception, and the pivot carries the ephemeral-data
/// warning — matching `analyze`'s behavior.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_dirty_tree_on_the_base_warns() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 100.0);
    workspace.seed_dirty_callgrind("2024-01-03", "c2", 130.0);
    workspace.make_dirty("uncommitted.txt");

    let message = workspace
        .drive_json(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    let points = parsed["sets"][0]["points"].as_array().unwrap();
    assert_eq!(
        points.len(),
        3,
        "the dirty tip snapshot is admitted: {message}"
    );
    assert!(
        parsed["warning"].as_str().is_some(),
        "the ephemeral-data warning is present: {message}"
    );

    // `--no-dirty` skips the exception, dropping the dirty tip and its warning.
    let message = workspace
        .drive_json(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
            "--no-dirty",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(
        parsed["sets"][0]["points"].as_array().unwrap().len(),
        2,
        "{message}"
    );
    assert!(parsed["warning"].is_null(), "{message}");
}

/// An unknown `--metric` is rejected up front, before any load, listing the valid
/// names so the maintainer can correct the typo.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_rejects_an_unknown_metric() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);

    let error = workspace
        .drive(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "cycles_per_second",
        ])
        .await
        .unwrap_err();
    let RunError::Analyze { message } = error else {
        panic!("expected an analyze error, got {error:?}");
    };
    assert!(message.contains("unknown metric"), "{message}");
    assert!(
        message.contains("instruction_count"),
        "the error lists valid names: {message}"
    );
}

/// An unmatched benchmark id is not an error: runs enter the analysis but none
/// carry the pair, so the pivot is empty and a hint explains why.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_unmatched_benchmark_reports_a_hint() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let message = workspace
        .drive_json(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/does-not-exist",
            "--metric",
            "instruction_count",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert!(
        parsed["sets"].as_array().unwrap().is_empty(),
        "no set matches the id: {message}"
    );
    let hint = parsed["hint"].as_str().unwrap();
    assert!(hint.contains("entered the analysis"), "{hint}");
    assert!(hint.contains("does-not-exist"), "{hint}");
}

/// Like `analyze`/`list`, `examine` needs a repository to order the points by
/// topology and to label them with commit titles; without one it errors.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_without_a_repository_errors() {
    let workspace = Workspace::new(&storage_only_config());

    let error = workspace
        .drive(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
        ])
        .await
        .unwrap_err();
    let RunError::Analyze { message } = error else {
        panic!("expected an analyze error, got {error:?}");
    };
    assert!(message.contains("requires a git repository"), "{message}");
}

/// The benchmark id names the series to pivot, so it must be present: an empty
/// `--benchmark` is rejected up front, before any load.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_rejects_an_empty_benchmark() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);

    let error = workspace
        .drive(&[
            "examine",
            "--benchmark",
            "",
            "--metric",
            "instruction_count",
        ])
        .await
        .unwrap_err();
    let RunError::Analyze { message } = error else {
        panic!("expected an analyze error, got {error:?}");
    };
    assert!(
        message.contains("--benchmark must not be empty"),
        "{message}"
    );
}

/// `--no-text` suppresses the text report, so with no file output requested there
/// is nothing left to emit — an error rather than a silent no-op.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_requires_an_output() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);

    let error = workspace
        .drive(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
            "--no-text",
        ])
        .await
        .unwrap_err();
    let RunError::Analyze { message } = error else {
        panic!("expected an analyze error, got {error:?}");
    };
    assert!(message.contains("no output selected"), "{message}");
}

/// When runs exist but the selection window excludes them all, `examine` gives the
/// same self-explaining empty-history hint `analyze` does (distinct from the
/// unmatched-series hint): the pivot is empty because no run entered the analysis,
/// not because the pair was unmatched.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_empty_after_since_reports_the_analyze_hint() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    // Every seeded run lands in January 2024; a March cutoff excludes them all.
    let message = workspace
        .drive_json(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
            "--since",
            "2024-03-01",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert!(
        parsed["sets"].as_array().unwrap().is_empty(),
        "the --since cutoff excludes every run: {message}"
    );
    let hint = parsed["hint"].as_str().unwrap();
    assert!(hint.contains("entered the analysis"), "{hint}");
    assert!(hint.contains("--since cutoff"), "{hint}");
}

/// The Markdown report of an empty pivot states plainly that nothing matched
/// rather than emitting a headerless, rowless table.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn examine_markdown_renders_an_empty_pivot() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let markdown = workspace
        .drive_markdown(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/does-not-exist",
            "--metric",
            "instruction_count",
        ])
        .await;
    assert!(
        markdown.contains("No data point matches the selection."),
        "{markdown}"
    );
}
