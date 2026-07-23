use crate::harness::*;

/// An empty history analyzes cleanly and reports nothing.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_empty_history_reports_no_changes() {
    let workspace = Workspace::repo(&storage_only_config());

    let outcome = workspace.drive(&["analyze"]).await.unwrap();
    let RunOutcome::Analyzed {
        report,
        regressions,
        ..
    } = outcome
    else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
    assert_eq!(regressions, 0);
    assert!(report.contains("No notable changes detected."), "{report}");
}

/// A rising history is flagged as a regression end to end.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_detects_regression_in_seeded_history() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let outcome = workspace.drive(&["analyze"]).await.unwrap();
    let RunOutcome::Analyzed {
        report,
        regressions,
        ..
    } = outcome
    else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
    assert_eq!(regressions, 1);
    assert!(report.contains("regression"), "{report}");
    assert!(report.contains("nm::observe/pull"), "{report}");
    assert!(report.contains("instruction_count"), "{report}");
}

/// History written by an older tool can carry metric kinds since dropped (the
/// build-layout-volatile Callgrind events). `analyze` must skip those kinds and keep
/// folding the retained ones rather than aborting the whole run on an unknown-variant
/// parse error — the exact failure a stored `conditional_branch_misses` metric caused
/// on the real history. The `instruction_count` regression must still surface.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_tolerates_legacy_metric_kinds_in_stored_history() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind_with_legacy_metric("c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind_with_legacy_metric("c2", 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_callgrind_with_legacy_metric("c3", 100.0);
    workspace.commit_dated("2024-01-04", "c4");
    workspace.seed_callgrind_with_legacy_metric("c4", 130.0);
    workspace.commit_dated("2024-01-05", "c5");
    workspace.seed_callgrind_with_legacy_metric("c5", 130.0);
    workspace.commit_dated("2024-01-06", "c6");
    workspace.seed_callgrind_with_legacy_metric("c6", 130.0);

    let outcome = workspace.drive(&["analyze"]).await.unwrap();
    let RunOutcome::Analyzed {
        report,
        regressions,
        ..
    } = outcome
    else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
    assert_eq!(regressions, 1);
    assert!(report.contains("nm::observe/pull"), "{report}");
    assert!(report.contains("instruction_count"), "{report}");
}

/// A rising Criterion `wall_time` history is flagged as a regression, proving the
/// analyzer reconstructs series across a machine-keyed partition too.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_detects_criterion_wall_time_regression() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_criterion_history("mk-fixed");

    let outcome = workspace
        .drive(&[
            "analyze",
            "--target-triple",
            "x86_64-pc-windows-msvc",
            "--machine-key",
            "mk-fixed",
        ])
        .await
        .unwrap();
    let RunOutcome::Analyzed {
        report,
        regressions,
        ..
    } = outcome
    else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
    assert_eq!(regressions, 1);
    assert!(report.contains("time/capture/std_instant"), "{report}");
    assert!(report.contains("wall_time"), "{report}");
}

/// A flagged regression never fails the process: findings are advisory, so the
/// exit code reflects only whether the tool ran, not what it found.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_with_regression_is_always_successful() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let outcome = workspace.drive(&["analyze"]).await.unwrap();
    assert!(
        outcome.is_success(),
        "findings must never fail the build: {outcome:?}"
    );
}

/// `--json <path>` writes a structured, parseable report.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_json_output_is_structured() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["project"], "testproj");
    // The report names the analyzed tip commit (a full-length hex commit ID) so a
    // reader can identify exactly which commit it describes; the seeded checkout is
    // clean. Accept either length Git may use (40 for SHA-1, 64 for SHA-256).
    let tip = parsed["tip_commit"].as_str().unwrap();
    assert!(
        (tip.len() == 40 || tip.len() == 64)
            && tip.chars().all(|character| character.is_ascii_hexdigit()),
        "tip_commit should be a full-length hex commit ID: {tip:?}"
    );
    assert_eq!(parsed["tip_dirty"], false);
    assert_eq!(parsed["regressions"], 1);
    assert_eq!(parsed["findings"][0]["direction"], "regression");
}

/// `--markdown <path>` writes per-finding blocks mirroring the text report.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_markdown_output_renders_blocks() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let report = workspace.drive_markdown(&["analyze"]).await;
    assert!(
        report.contains("# Benchmark history analysis: testproj"),
        "{report}"
    );
    // The header names the analyzed tip commit so the reader knows which commit the
    // findings describe.
    assert!(report.contains("- Commit: "), "{report}");
    // Findings render as heading + bold-headline blocks; there is no table.
    assert!(!report.contains("| Change | Direction |"), "{report}");
    // Each finding leads with its benchmark id as a `###` chapter title (nested under
    // the set `##`), then a bold percentage headline carrying the confidence.
    assert!(report.contains("\n### `"), "{report}");
    assert!(report.contains("% confidence)"), "{report}");
    assert!(report.contains("via change point"), "{report}");
}

/// Markdown and JSON files can be rendered from one analysis pass while text is suppressed.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_writes_markdown_and_json_from_one_suppressed_text_pass() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let outcome = workspace
        .drive(&[
            "analyze",
            "--no-text",
            "--markdown",
            "report.md",
            "--json",
            "report.json",
        ])
        .await
        .unwrap();
    let RunOutcome::Analyzed { report, .. } = outcome else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
    assert!(report.is_empty(), "{report}");

    let markdown = workspace.read("report.md");
    let json = workspace.read("report.json");
    assert!(markdown.is_some(), "the Markdown report should be written");
    assert!(json.is_some(), "the JSON report should be written");

    let markdown = markdown.unwrap();
    assert!(
        markdown.contains("# Benchmark history analysis: testproj"),
        "{markdown}"
    );
    let json = json.unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["project"], "testproj");
}

/// `--markdown-summary <path>` renders a flat, ungrouped condensed report that omits
/// the per-set headings the full Markdown report carries. With few findings there is
/// no truncation note.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_markdown_summary_renders_a_flat_report() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let summary = workspace.drive_markdown_summary(&["analyze"]).await;

    assert!(
        summary.contains("# Benchmark history analysis: testproj"),
        "{summary}"
    );
    // Each finding leads with its benchmark id as a `##` chapter title, then a bold
    // percentage headline carrying the confidence — the same block the full report
    // uses, one heading level up.
    assert!(summary.contains("\n## `"), "{summary}");
    assert!(summary.contains("% confidence)"), "{summary}");
    // A single seeded regression is well within the top-20 cap, so no truncation note
    // is added.
    assert!(
        !summary.contains("Showing the top"),
        "an untruncated summary must not claim it is partial: {summary}"
    );
    // The per-set `## engine/triple/machine` grouping is deliberately dropped from the
    // summary, unlike the full Markdown report; only the flat finding headings remain.
    assert!(
        !summary.contains("## callgrind"),
        "the summary must be flat, without per-set headings: {summary}"
    );
    // Each flat finding instead carries the facet flags of its partition as a footer,
    // so a reader who loses the per-set grouping still knows how to query the exact set.
    assert!(
        summary.contains(
            "_Filter:_ `--engine callgrind --target-triple x86_64-unknown-linux-gnu --machine-key harness-auto-machine`"
        ),
        "{summary}"
    );
}

/// When an analysis produces more findings than the summary cap, `--markdown-summary`
/// keeps only the top 10 by magnitude and states the total, while a full `--markdown`
/// report from the same run still lists every finding.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_markdown_summary_caps_findings_and_names_the_total() {
    let workspace = Workspace::repo(&storage_only_config());
    // Seed more distinct rising benchmarks than the top-10 cap so truncation engages.
    workspace.seed_many_rising_callgrind_history(25);

    let outcome = workspace
        .drive(&[
            "analyze",
            "--no-text",
            "--markdown",
            "full.md",
            "--markdown-summary",
            "summary.md",
        ])
        .await
        .unwrap();
    let RunOutcome::Analyzed { regressions, .. } = outcome else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
    assert_eq!(regressions, 25, "every seeded benchmark should regress");

    let summary = workspace
        .read("summary.md")
        .expect("the Markdown summary should be written");
    let full = workspace
        .read("full.md")
        .expect("the full Markdown report should be written");

    // The header carries the *true* total (all 25), not the retained subset.
    assert!(summary.contains("- Regressions: 25"), "{summary}");
    // The truncation note names how many of the total are shown.
    assert!(
        summary.contains("Showing the top 10 of 25 findings by magnitude."),
        "{summary}"
    );
    // Exactly the cap of finding headlines survives in the summary. Each finding
    // headline carries the confidence, which appears nowhere else, so it counts blocks.
    let summary_blocks = summary.matches("% confidence)").count();
    assert_eq!(
        summary_blocks, 10,
        "summary should keep only the cap: {summary}"
    );
    // The full report from the same run still lists every finding.
    let full_blocks = full.matches("% confidence)").count();
    assert_eq!(
        full_blocks, 25,
        "full report should keep every finding: {full}"
    );
}

/// `--no-text` suppresses stdout text while still writing a JSON report.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_no_text_suppresses_stdout_alongside_json_file() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let outcome = workspace
        .drive(&["analyze", "--no-text", "--json", "report.json"])
        .await
        .unwrap();
    let RunOutcome::Analyzed { report, .. } = outcome else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
    assert!(report.is_empty(), "{report}");

    let json = workspace
        .read("report.json")
        .expect("the JSON report should be written");
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["project"], "testproj");
}

/// Text remains on by default when a JSON file is also requested.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_default_text_coexists_with_json_file() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let outcome = workspace
        .drive(&["analyze", "--json", "report.json"])
        .await
        .unwrap();
    let RunOutcome::Analyzed { report, .. } = outcome else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
    assert!(!report.is_empty(), "text should be rendered by default");
    assert!(report.contains("regression"), "{report}");

    let json = workspace
        .read("report.json")
        .expect("the JSON report should be written");
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["project"], "testproj");
}

/// `--since` excludes points before the cutoff, so an early spike is dropped and
/// the remaining flat history shows no change.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_since_filters_old_points() {
    let workspace = Workspace::repo(&storage_only_config());
    // A flat recent history (no change), preceded long ago by an unrelated point.
    workspace.commit_dated("2020-01-01", "c0");
    workspace.seed_callgrind("c0", 999.0);
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_callgrind("c3", 100.0);

    let report = workspace
        .drive_json(&["analyze", "--since", "2024-01-01"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["runs"], 3,
        "the pre-cutoff point is excluded: {report}"
    );
    assert_eq!(parsed["regressions"], 0, "{report}");
}

/// `--engine` restricts analysis to one engine's partition.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_engine_filters_partition() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    // A criterion-partition object that the callgrind filter must skip. Its commit
    // segment is never read because the engine facet excludes it from listing.
    workspace.seed(
        "v1/testproj/objects/criterion/x86_64-pc-windows-msvc/m1/abc123/clean.json",
        &ir_result_set(1, "c1", 100.0),
    );

    let report = workspace
        .drive_json(&["analyze", "--engine", "callgrind"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["runs"], 1,
        "only the callgrind object is loaded: {report}"
    );
}

/// Selecting no output for `analyze` is reported as an analysis error.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_rejects_no_output_selected() {
    let workspace = Workspace::repo(&storage_only_config());

    let error = workspace
        .drive(&["analyze", "--no-text"])
        .await
        .unwrap_err();
    let RunError::Analyze { message } = error else {
        panic!("expected an analyze error, got {error:?}");
    };
    assert!(message.contains("no output selected"), "{message}");
}

/// A benchmark-id prefix narrows analysis to the matching benchmarks: a
/// regression in another benchmark is invisible when the filter selects a flat one.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_prefix_filter_excludes_other_benchmarks() {
    let workspace = Workspace::repo(&storage_only_config());
    // `alpha` stays flat while `beta` climbs into a regression.
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_two_benchmarks("c1", 100.0, 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_two_benchmarks("c2", 100.0, 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_two_benchmarks("c3", 100.0, 100.0);
    workspace.commit_dated("2024-01-04", "c4");
    workspace.seed_two_benchmarks("c4", 100.0, 130.0);
    workspace.commit_dated("2024-01-05", "c5");
    workspace.seed_two_benchmarks("c5", 100.0, 130.0);
    workspace.commit_dated("2024-01-06", "c6");
    workspace.seed_two_benchmarks("c6", 100.0, 130.0);

    // Unfiltered, the `beta` climb flags as a regression.
    let RunOutcome::Analyzed { regressions, .. } = workspace.drive(&["analyze"]).await.unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the beta climb should flag unfiltered");

    // Restricting to the flat `alpha` benchmark family hides it.
    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace.drive(&["analyze", "alpha/"]).await.unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 0, "the alpha series is flat: {report}");
}

/// Two benchmarks regressing by different magnitudes produce two findings that the
/// report ranks largest-relative-move first, end to end through real storage and
/// rendering.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_ranks_findings_by_relative_move_across_benchmarks() {
    let workspace = Workspace::repo(&storage_only_config());
    // `alpha` doubles (+100%); `beta` steps up +5%.
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_two_benchmarks("c1", 100.0, 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_two_benchmarks("c2", 100.0, 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_two_benchmarks("c3", 100.0, 100.0);
    workspace.commit_dated("2024-01-04", "c4");
    workspace.seed_two_benchmarks("c4", 200.0, 105.0);
    workspace.commit_dated("2024-01-05", "c5");
    workspace.seed_two_benchmarks("c5", 200.0, 105.0);
    workspace.commit_dated("2024-01-06", "c6");
    workspace.seed_two_benchmarks("c6", 200.0, 105.0);

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 2,
        "both benchmarks regress: {report}"
    );

    let findings = parsed["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 2, "{report}");
    // The larger relative move (alpha, +100%) ranks ahead of the smaller (beta, +5%).
    assert_eq!(findings[0]["segments"][0], "alpha", "{report}");
    assert_eq!(findings[0]["segments"][1], "alpha::bench", "{report}");
    assert_eq!(findings[1]["segments"][0], "beta", "{report}");

    // The Markdown rendering preserves the same ranked order in its finding blocks.
    let markdown = workspace.drive_markdown(&["analyze"]).await;
    let alpha_row = markdown.find("alpha::bench").unwrap();
    let beta_row = markdown.find("beta::bench").unwrap();
    assert!(
        alpha_row < beta_row,
        "the larger move must precede the smaller:\n{markdown}"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_reports_improvement_without_regression() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_callgrind("c3", 100.0);
    workspace.commit_dated("2024-01-04", "c4");
    workspace.seed_callgrind("c4", 70.0);
    workspace.commit_dated("2024-01-05", "c5");
    workspace.seed_callgrind("c5", 70.0);
    workspace.commit_dated("2024-01-06", "c6");
    workspace.seed_callgrind("c6", 70.0);

    let report = workspace
        .drive_json(&["analyze", "--include-improvements"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 0,
        "a drop in instructions is not a regression: {report}"
    );
    assert_eq!(parsed["findings"][0]["direction"], "improvement");
}

/// Run-to-run jitter on a Criterion `wall_time` series whose underlying mean is
/// flat produces no finding at all: the rank-sum gate finds no real separation
/// between the regimes and the trend test finds no monotonic drift, so substantial
/// per-point noise never manufactures a spurious regression or improvement. This is
/// the core signal-to-noise guarantee for the noisy engine.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_criterion_jitter_is_not_flagged() {
    let workspace = Workspace::repo(&storage_only_config());
    // Eight points oscillating roughly +/-15% around 20ns with no sustained shift.
    for (date, label, value) in [
        ("2024-02-01", "d1", 20.0),
        ("2024-02-02", "d2", 23.0),
        ("2024-02-03", "d3", 18.0),
        ("2024-02-04", "d4", 21.0),
        ("2024-02-05", "d5", 19.0),
        ("2024-02-06", "d6", 22.0),
        ("2024-02-07", "d7", 18.0),
        ("2024-02-08", "d8", 20.0),
    ] {
        workspace.commit_dated(date, label);
        workspace.seed_criterion(label, "mk", value);
    }

    let report = workspace
        .drive_json(&[
            "analyze",
            "--target-triple",
            "x86_64-pc-windows-msvc",
            "--machine-key",
            "mk",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 0,
        "jitter must not read as a regression: {report}"
    );
    assert!(
        parsed["findings"].as_array().unwrap().is_empty(),
        "noisy jitter around a flat mean produces no findings at all: {report}"
    );
}

/// A slow, monotonic Criterion `wall_time` drift is flagged as a `drift` finding
/// (not a change-point): the Mann-Kendall trend is significant, the Theil-Sen line
/// fits the gentle ramp better than any single step, and the total movement clears
/// the practical-magnitude floor. This exercises the second detector and the
/// model-fit arbitration that routes ramps to drift and steps to change-point.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_criterion_slow_drift_is_flagged_as_drift() {
    let workspace = Workspace::repo(&storage_only_config());
    // A gentle one-nanosecond-per-commit ramp from 20ns to 27ns.
    for (date, label, value) in [
        ("2024-02-01", "d1", 20.0),
        ("2024-02-02", "d2", 21.0),
        ("2024-02-03", "d3", 22.0),
        ("2024-02-04", "d4", 23.0),
        ("2024-02-05", "d5", 24.0),
        ("2024-02-06", "d6", 25.0),
        ("2024-02-07", "d7", 26.0),
        ("2024-02-08", "d8", 27.0),
    ] {
        workspace.commit_dated(date, label);
        workspace.seed_criterion(label, "mk", value);
    }

    let report = workspace
        .drive_json(&[
            "analyze",
            "--target-triple",
            "x86_64-pc-windows-msvc",
            "--machine-key",
            "mk",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 1,
        "the upward drift is a regression: {report}"
    );
    assert_eq!(parsed["findings"][0]["method"], "drift", "{report}");
    assert_eq!(parsed["findings"][0]["direction"], "regression", "{report}");
}

/// A Callgrind series whose instruction count steps up by a single count - a 0.1%
/// move - is below the practical-magnitude floor every engine now shares, so it is
/// suppressed. No engine is exact (even a CPU simulator's counts jitter run to run),
/// and a sub-percent change is not worth a human's attention regardless of how
/// certainly it was measured.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_callgrind_sub_floor_step_is_suppressed() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 1000.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 1000.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_callgrind("c3", 1000.0);
    workspace.commit_dated("2024-01-04", "c4");
    workspace.seed_callgrind("c4", 1001.0);
    workspace.commit_dated("2024-01-05", "c5");
    workspace.seed_callgrind("c5", 1001.0);
    workspace.commit_dated("2024-01-06", "c6");
    workspace.seed_callgrind("c6", 1001.0);

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 0,
        "a 0.1% step is below the practical-magnitude floor and must not flag: {report}"
    );
    let findings = parsed["findings"].as_array().expect("findings is an array");
    assert!(
        findings.is_empty(),
        "the sub-floor move is fully suppressed, so no finding of any direction reaches \
         the report: {report}"
    );
}

/// A Callgrind series that steps up by 5% clears the practical-magnitude floor and is
/// flagged as a `change_point`. Callgrind is low-noise but not exact, so it passes
/// through the same rank test, practical floor and residual gate as any other engine
/// rather than being trusted as deterministic.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_callgrind_step_above_the_practical_floor_is_flagged() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 1000.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 1000.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_callgrind("c3", 1000.0);
    workspace.commit_dated("2024-01-04", "c4");
    workspace.seed_callgrind("c4", 1050.0);
    workspace.commit_dated("2024-01-05", "c5");
    workspace.seed_callgrind("c5", 1050.0);
    workspace.commit_dated("2024-01-06", "c6");
    workspace.seed_callgrind("c6", 1050.0);

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 1,
        "a 5% Callgrind step clears the practical floor and must flag: {report}"
    );
    assert_eq!(parsed["findings"][0]["method"], "change_point", "{report}");
    assert_eq!(parsed["findings"][0]["direction"], "regression", "{report}");
}

/// An `alloc_tracker` series whose allocated-bytes figure steps up by 5% is flagged as
/// a `change_point`. Allocation figures are not deterministic - warmup and
/// buffer-resize allocations jitter the per-iteration count - so they clear the same
/// gates as any other engine; the constant allocation count produces no finding.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_alloc_tracker_step_is_flagged_as_change_point() {
    let workspace = Workspace::repo(&storage_only_config());
    // A flat allocated-bytes baseline that steps up by 5% at the fourth commit; the
    // allocation count stays constant throughout.
    workspace.commit_dated("2024-03-01", "a1");
    workspace.seed_alloc_tracker("a1", "allocate_vec", 200.0, 2.0);
    workspace.commit_dated("2024-03-02", "a2");
    workspace.seed_alloc_tracker("a2", "allocate_vec", 200.0, 2.0);
    workspace.commit_dated("2024-03-03", "a3");
    workspace.seed_alloc_tracker("a3", "allocate_vec", 200.0, 2.0);
    workspace.commit_dated("2024-03-04", "a4");
    workspace.seed_alloc_tracker("a4", "allocate_vec", 210.0, 2.0);
    workspace.commit_dated("2024-03-05", "a5");
    workspace.seed_alloc_tracker("a5", "allocate_vec", 210.0, 2.0);
    workspace.commit_dated("2024-03-06", "a6");
    workspace.seed_alloc_tracker("a6", "allocate_vec", 210.0, 2.0);

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 1,
        "a 5% allocated-bytes step clears the practical floor and must flag: {report}"
    );
    assert_eq!(parsed["findings"][0]["method"], "change_point", "{report}");
    assert_eq!(parsed["findings"][0]["direction"], "regression", "{report}");
    assert_eq!(parsed["findings"][0]["kind"], "allocated_bytes", "{report}");
    assert!(report.contains("allocate_vec"), "{report}");
}

/// A gapped `alloc_tracker` history - where some commits measure a different
/// operation - reconstructs each series across only the commits that measured it,
/// treating an unmeasured commit as a missing observation rather than a drop to
/// zero. The `allocate_vec` series spanning the gaps still surfaces its real step.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_alloc_tracker_ignores_gaps_in_a_sparse_history() {
    let workspace = Workspace::repo(&storage_only_config());
    // `allocate_vec` is measured only at every other commit; the intervening
    // commits measure `free_box` instead, leaving `allocate_vec` unobserved there.
    // Its step spans the gaps: three baseline observations, then three raised ones.
    // The history ends on an `allocate_vec` commit so that series is present at the
    // tip and survives the ghost filter (`free_box` is the ghost here, and being
    // flat it produces no finding regardless).
    let mut day = 1;
    for bytes in [200.0, 200.0, 200.0, 260.0, 260.0, 260.0] {
        workspace.commit_dated(&format!("2024-04-{day:02}"), &format!("g{day}"));
        workspace.seed_alloc_tracker(&format!("g{day}"), "free_box", 8.0, 1.0);
        day += 1;
        workspace.commit_dated(&format!("2024-04-{day:02}"), &format!("g{day}"));
        workspace.seed_alloc_tracker(&format!("g{day}"), "allocate_vec", bytes, 2.0);
        day += 1;
    }

    // Only `allocate_vec`'s allocated-bytes step is a finding. A gap read as a drop
    // to zero would manufacture wild swings; instead the series is contiguous.
    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 1,
        "the gapped series' real step flags: {report}"
    );
    assert_eq!(parsed["findings"][0]["method"], "change_point", "{report}");
    assert_eq!(parsed["findings"][0]["direction"], "regression", "{report}");
    assert!(report.contains("allocate_vec"), "{report}");
    assert!(
        !report.contains("free_box"),
        "the flat free_box series produces no finding: {report}"
    );
}

/// A slow, monotonic `all_the_time` processor-time drift is flagged as a `drift`
/// finding. Processor time is a noisy, hardware-dependent measurement (like
/// Criterion wall time), so a gentle ramp clears the practical-magnitude floor only
/// via the trend detector. These synthetic points are seeded mean-only (no
/// confidence interval), so the detector falls back gracefully to the Mann-Kendall
/// trend and practical floor. In practice `all_the_time` does record a confidence
/// interval, which would only ever act as an additional veto here, never change the
/// outcome.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_all_the_time_slow_drift_is_flagged_as_drift() {
    let workspace = Workspace::repo(&storage_only_config());
    // A gentle one-nanosecond-per-commit ramp from 20ns to 27ns.
    for (date, label, value) in [
        ("2024-05-01", "t1", 20.0),
        ("2024-05-02", "t2", 21.0),
        ("2024-05-03", "t3", 22.0),
        ("2024-05-04", "t4", 23.0),
        ("2024-05-05", "t5", 24.0),
        ("2024-05-06", "t6", 25.0),
        ("2024-05-07", "t7", 26.0),
        ("2024-05-08", "t8", 27.0),
    ] {
        workspace.commit_dated(date, label);
        workspace.seed_all_the_time(label, "mk", "read_cell", value);
    }

    let report = workspace
        .drive_json(&[
            "analyze",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--machine-key",
            "mk",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 1,
        "the upward drift is a regression: {report}"
    );
    assert_eq!(parsed["findings"][0]["method"], "drift", "{report}");
    assert_eq!(parsed["findings"][0]["direction"], "regression", "{report}");
    assert_eq!(parsed["findings"][0]["kind"], "processor_time", "{report}");
}

/// Per-point noise in an `all_the_time` processor-time series never manufactures a
/// spurious finding: oscillation around a flat mean clears neither the trend test
/// nor the change-point gate, so the noisy engine produces nothing.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_all_the_time_jitter_is_not_flagged() {
    let workspace = Workspace::repo(&storage_only_config());
    // Eight points oscillating roughly +/-15% around 20ns with no sustained shift.
    for (date, label, value) in [
        ("2024-06-01", "j1", 20.0),
        ("2024-06-02", "j2", 23.0),
        ("2024-06-03", "j3", 18.0),
        ("2024-06-04", "j4", 21.0),
        ("2024-06-05", "j5", 19.0),
        ("2024-06-06", "j6", 22.0),
        ("2024-06-07", "j7", 18.0),
        ("2024-06-08", "j8", 20.0),
    ] {
        workspace.commit_dated(date, label);
        workspace.seed_all_the_time(label, "mk", "read_cell", value);
    }

    let report = workspace
        .drive_json(&[
            "analyze",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--machine-key",
            "mk",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 0,
        "jitter must not read as a regression: {report}"
    );
    assert!(
        parsed["findings"].as_array().unwrap().is_empty(),
        "noisy jitter around a flat mean produces no findings at all: {report}"
    );
}

/// A sustained processor-time step whose per-regime confidence intervals are
/// tight and non-overlapping clears the noise detector's CI gate and is reported
/// as a regression. This proves the confidence interval the `all_the_time` engine
/// now records flows through the adapter into the CI-non-overlap gate for
/// processor time.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_all_the_time_step_with_disjoint_intervals_is_a_regression() {
    let workspace = Workspace::repo(&storage_only_config());
    // Five points near 100ns then five near 130ns, each with a tight +/-2ns
    // confidence interval so the two regimes' intervals do not overlap.
    for (date, label, value) in [
        ("2024-07-01", "s1", 98.0),
        ("2024-07-02", "s2", 100.0),
        ("2024-07-03", "s3", 102.0),
        ("2024-07-04", "s4", 99.0),
        ("2024-07-05", "s5", 101.0),
        ("2024-07-06", "s6", 128.0),
        ("2024-07-07", "s7", 130.0),
        ("2024-07-08", "s8", 132.0),
        ("2024-07-09", "s9", 129.0),
        ("2024-07-10", "s10", 131.0),
    ] {
        workspace.commit_dated(date, label);
        workspace.seed_all_the_time_with_interval(label, "mk", "read_cell", value, 2.0);
    }

    let report = workspace
        .drive_json(&[
            "analyze",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--machine-key",
            "mk",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 1,
        "a sustained step with disjoint intervals is a regression: {report}"
    );
    assert_eq!(parsed["findings"][0]["method"], "change_point", "{report}");
    assert_eq!(parsed["findings"][0]["direction"], "regression", "{report}");
    assert_eq!(parsed["findings"][0]["kind"], "processor_time", "{report}");
}

/// The same processor-time step values, but with confidence intervals so wide
/// that the two regimes overlap, is suppressed: the CI-non-overlap gate rejects
/// it even though the point medians separate cleanly. This is the companion to
/// [`analyze_all_the_time_step_with_disjoint_intervals_is_a_regression`] — only
/// the recorded dispersion differs, proving the gate is active on processor time.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_all_the_time_step_with_overlapping_intervals_is_suppressed() {
    let workspace = Workspace::repo(&storage_only_config());
    for (date, label, value) in [
        ("2024-08-01", "o1", 98.0),
        ("2024-08-02", "o2", 100.0),
        ("2024-08-03", "o3", 102.0),
        ("2024-08-04", "o4", 99.0),
        ("2024-08-05", "o5", 101.0),
        ("2024-08-06", "o6", 128.0),
        ("2024-08-07", "o7", 130.0),
        ("2024-08-08", "o8", 132.0),
        ("2024-08-09", "o9", 129.0),
        ("2024-08-10", "o10", 131.0),
    ] {
        // A +/-60ns interval makes the ~100ns and ~130ns regimes overlap.
        workspace.commit_dated(date, label);
        workspace.seed_all_the_time_with_interval(label, "mk", "read_cell", value, 60.0);
    }

    let report = workspace
        .drive_json(&[
            "analyze",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--machine-key",
            "mk",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 0,
        "an overlapping-interval step is suppressed by the CI gate: {report}"
    );
    assert!(
        parsed["findings"].as_array().unwrap().is_empty(),
        "wide overlapping intervals suppress the step entirely: {report}"
    );
}

/// Without a repository to resolve topology from, `analyze` is an error rather
/// than an empty success — a series' timeline comes from git, not storage.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_without_a_repository_errors() {
    let workspace = Workspace::new(&storage_only_config());

    let error = workspace.drive(&["analyze"]).await.unwrap_err();
    let RunError::Analyze { message } = error else {
        panic!("expected an analyze error, got {error:?}");
    };
    assert!(message.contains("requires a git repository"), "{message}");
}

/// The official view (analyzing the default branch against itself) admits only
/// clean runs: a dirty snapshot sitting on the tip commit is excluded, so a dirty
/// spike does not flag and the run count covers only the clean points.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_official_view_excludes_dirty_runs() {
    // A clean working tree, so the base-branch dirty-tree exception does not apply.
    let workspace = Workspace::clean_repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_callgrind("c3", 100.0);
    workspace.commit_dated("2024-01-04", "c4");
    workspace.seed_callgrind("c4", 100.0);
    // A dirty snapshot on the tip commit that would spike the series if admitted.
    workspace.seed_dirty_callgrind("2024-01-05", "c4", 200.0);

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 0,
        "the dirty spike must be excluded: {report}"
    );
    assert_eq!(
        parsed["runs"], 4,
        "only the four clean runs are loaded, not the dirty snapshot: {report}"
    );
}

/// When every stored run on the default branch is a dirty (uncommitted-tree)
/// snapshot — the trap a user hits when the configuration file is never committed,
/// so each `collect` records a `dirty-*.json` on the base-side tip — `analyze` finds
/// zero runs but renders a diagnostic hint that explains the exclusion, so the
/// empty result is not mistaken for "no data". This exercises the discoverability
/// fix end to end through the production dispatch.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_hints_when_every_run_is_a_dirty_snapshot_on_the_base() {
    // A clean working tree, so the base-branch dirty-tree exception does not apply
    // and every dirty-on-base snapshot stays excluded (yielding the hint).
    let workspace = Workspace::clean_repo(&storage_only_config());
    // Two dirty snapshots on master commits and not a single clean run anywhere.
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_dirty_callgrind("2024-01-01", "c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_dirty_callgrind("2024-01-02", "c2", 130.0);

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["runs"], 0,
        "every dirty-on-base snapshot is excluded: {report}"
    );
    let hint = parsed["hint"].as_str().unwrap();
    assert!(
        hint.contains("Found 2 stored runs"),
        "the hint should count the stored runs: {hint}"
    );
    assert!(
        hint.contains("dirty"),
        "the hint should explain the dirty-on-base exclusion: {hint}"
    );
}

/// The mirror of the exclusion case: when the working tree is *currently* dirty on
/// the base branch (the user is evaluating the tool, or accidentally working on top
/// of the default branch), the dirty snapshot on the tip commit IS admitted, and
/// the report ends with a warning that such data may be excluded from future
/// analysis. `--no-dirty` overrides it. Exercised end to end through the production
/// dispatch with a real dirty git tree.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_dirty_tree_on_the_base_admits_the_tip_with_a_warning() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_callgrind("c3", 100.0);
    // Three dirty regression snapshots on the tip commit (c3) complete a sustained
    // step over the clean baseline (three raised points clear the change-point gate).
    workspace.seed_dirty_callgrind("2024-01-04", "c3", 130.0);
    workspace.seed_dirty_callgrind("2024-01-05", "c3", 130.0);
    workspace.seed_dirty_callgrind("2024-01-06", "c3", 130.0);
    // Make the working tree genuinely dirty (an uncommitted, non-ignored file),
    // matching the "evaluating the tool" / "working on the base branch" scenario.
    workspace.make_dirty("uncommitted.txt");

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 1,
        "the dirty tip regression is admitted: {report}"
    );
    assert_eq!(
        parsed["runs"], 6,
        "the dirty tip snapshots join the three clean runs: {report}"
    );
    let warning = parsed["warning"].as_str().unwrap();
    assert!(
        warning.contains("Switch to a new branch"),
        "the warning should advise switching to a branch: {warning}"
    );

    // `--no-dirty` skips the probe and the exception, so the dirty tip is dropped.
    let report = workspace.drive_json(&["analyze", "--no-dirty"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["runs"], 3,
        "--no-dirty drops the dirty tip snapshot: {report}"
    );
    assert!(
        parsed["warning"].is_null(),
        "no warning fires under --no-dirty: {report}"
    );
}

/// The reported corner case, end to end: a *currently-dirty* working tree on the
/// base branch but with ONLY clean runs recorded (no dirty snapshot on the tip).
/// Mode auto-detection keys off the recorded data set, not the on-disk tree, so this
/// stays `history` mode — and the long-range change-point detector still flags the
/// sustained clean step. (The old behaviour keyed off `git status` and wrongly fell
/// into branch mode here.)
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_dirty_tree_without_recorded_dirty_runs_stays_history_mode() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    // A clean base branch with a sustained upward step (a regression). No dirty
    // snapshots are recorded anywhere.
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_callgrind("c3", 100.0);
    workspace.commit_dated("2024-01-04", "c4");
    workspace.seed_callgrind("c4", 130.0);
    workspace.commit_dated("2024-01-05", "c5");
    workspace.seed_callgrind("c5", 130.0);
    workspace.commit_dated("2024-01-06", "c6");
    workspace.seed_callgrind("c6", 130.0);
    // Dirty the working tree, even though no dirty run was ever stored.
    workspace.make_dirty("uncommitted.txt");

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["mode"], "history",
        "a dirty tree with only clean runs is still the official history view: {report}"
    );
    assert_eq!(
        parsed["regressions"], 1,
        "history mode flags the sustained clean step: {report}"
    );
    assert_eq!(
        parsed["runs"], 6,
        "all six clean runs participate; nothing dirty exists: {report}"
    );
    assert!(
        parsed["warning"].is_null(),
        "no dirty runs are admitted, so nothing is ephemeral: {report}"
    );
}

/// A feature view admits dirty snapshots on the branch's own commits (after the
/// merge-base): a dirty regression on the feature tip flags by default, and
/// `--no-dirty` suppresses it.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_feature_branch_admits_dirty_snapshots() {
    let workspace = Workspace::repo(&storage_only_config());
    // A flat clean baseline on master.
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_callgrind("c3", 100.0);
    // Branch off master and add a clean point plus three dirty regression snapshots
    // on it (three raised points satisfy the change-point persistence requirement).
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-04", "f1");
    workspace.seed_callgrind("f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-05", "f1", 200.0);
    workspace.seed_dirty_callgrind("2024-01-06", "f1", 200.0);
    workspace.seed_dirty_callgrind("2024-01-07", "f1", 200.0);

    // By default (feature is HEAD; base is the detected master) the dirty snapshot
    // on the target side is admitted, so the spike flags.
    let RunOutcome::Analyzed { regressions, .. } = workspace.drive(&["analyze"]).await.unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the dirty feature snapshot should flag");

    // `--no-dirty` drops it, leaving only the flat clean history.
    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace.drive(&["analyze", "--no-dirty"]).await.unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "with --no-dirty only the flat clean series remains: {report}"
    );
}

/// The series timeline follows git topology, not the runs' commit timestamps:
/// seeding the regressing commit last in topology but earliest in time still flags
/// it, proving commit time does not reorder the history.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_orders_by_topology_not_commit_time() {
    let workspace = Workspace::repo(&storage_only_config());
    // Topology c1 -> .. -> c6 with a sustained step at c4, but commit times
    // strictly descend, so a commit-time ordering would reverse the step into
    // a non-regression (a drop). A flagged regression proves topology order won.
    workspace.commit_dated("2024-04-06", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.commit_dated("2024-04-05", "c2");
    workspace.seed_callgrind("c2", 100.0);
    workspace.commit_dated("2024-04-04", "c3");
    workspace.seed_callgrind("c3", 100.0);
    workspace.commit_dated("2024-04-03", "c4");
    workspace.seed_callgrind("c4", 130.0);
    workspace.commit_dated("2024-04-02", "c5");
    workspace.seed_callgrind("c5", 130.0);
    workspace.commit_dated("2024-04-01", "c6");
    workspace.seed_callgrind("c6", 130.0);

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 1,
        "topology order must place the 130 step last and flag it: {report}"
    );
    assert_eq!(parsed["findings"][0]["baseline"], 100.0, "{report}");
    assert_eq!(parsed["findings"][0]["latest"], 130.0, "{report}");
}

/// Branch mode judges a feature branch by its *tip commit*, not its journey:
/// a branch that first improved and then regressed is reported as a regression
/// against the base, since only the tip commit lands in the base on merge. The
/// intermediate improvement is not attributed as a within-branch flip.
/// The JSON also advertises the resolved mode and the downstream `notable` signal.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_branch_mode_reports_the_tip_commit_state() {
    let workspace = Workspace::repo(&storage_only_config());
    // A flat clean baseline on master.
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_callgrind("c3", 100.0);
    // The feature branch first improves (80) then regresses hard (130): the latest
    // state is what matters, so this must be reported as a regression vs the base.
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-04", "f1");
    workspace.seed_callgrind("f1", 80.0);
    workspace.commit_dated("2024-01-05", "f2");
    workspace.seed_callgrind("f2", 80.0);
    workspace.commit_dated("2024-01-06", "f3");
    workspace.seed_callgrind("f3", 80.0);
    workspace.commit_dated("2024-01-07", "f4");
    workspace.seed_callgrind("f4", 130.0);
    workspace.commit_dated("2024-01-08", "f5");
    workspace.seed_callgrind("f5", 130.0);
    workspace.commit_dated("2024-01-09", "f6");
    workspace.seed_callgrind("f6", 130.0);

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 1,
        "the latest (worse) regime must be reported, not the intermediate improvement: {report}"
    );
    assert_eq!(parsed["mode"], "branch", "{report}");
    assert_eq!(parsed["notable"], true, "{report}");
    let finding = &parsed["findings"][0];
    assert_eq!(finding["direction"], "regression", "{report}");
    assert_eq!(finding["baseline"], 100.0, "{report}");
    assert_eq!(finding["latest"], 130.0, "{report}");
    assert!(
        finding["flipped_at"].is_null(),
        "branch mode judges the tip commit alone, with no within-branch flip: {report}"
    );
    assert!(
        finding.get("series").is_none(),
        "the JSON finding mirrors the text data and omits the charting series: {report}"
    );
}

/// A feature branch that holds flat at the base level produces no finding in
/// branch mode: with no change in the latest state there is nothing to report,
/// and `notable` stays false so downstream automation knows to stay quiet.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_branch_mode_stays_silent_on_a_flat_branch() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_callgrind("c3", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-04", "f1");
    workspace.seed_callgrind("f1", 100.0);
    workspace.commit_dated("2024-01-05", "f2");
    workspace.seed_callgrind("f2", 100.0);
    workspace.commit_dated("2024-01-06", "f3");
    workspace.seed_callgrind("f3", 100.0);

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 0,
        "a flat branch must not flag: {report}"
    );
    assert_eq!(parsed["mode"], "branch", "{report}");
    assert_eq!(parsed["notable"], false, "{report}");
    assert!(
        parsed["findings"].as_array().is_some_and(Vec::is_empty),
        "{report}"
    );
}

/// History mode reports only regressions by default: steady improvement on the
/// base branch over time is expected and not noteworthy, so a sustained drop is
/// suppressed unless `--include-improvements` is given, which then surfaces it.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_history_mode_suppresses_improvements_by_default() {
    let workspace = Workspace::repo(&storage_only_config());
    // A clean base branch with a sustained downward step (an improvement).
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_callgrind("c3", 100.0);
    workspace.commit_dated("2024-01-04", "c4");
    workspace.seed_callgrind("c4", 70.0);
    workspace.commit_dated("2024-01-05", "c5");
    workspace.seed_callgrind("c5", 70.0);
    workspace.commit_dated("2024-01-06", "c6");
    workspace.seed_callgrind("c6", 70.0);

    // By default history mode stays silent about the improvement.
    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["mode"], "history", "{report}");
    assert_eq!(parsed["notable"], false, "{report}");
    assert_eq!(parsed["improvements"], 0, "{report}");
    assert!(
        parsed["findings"].as_array().is_some_and(Vec::is_empty),
        "improvements are suppressed by default in history mode: {report}"
    );

    // Opting in surfaces the very same improvement.
    let report = workspace
        .drive_json(&["analyze", "--include-improvements"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["improvements"], 1, "{report}");
    assert_eq!(
        parsed["findings"][0]["direction"], "improvement",
        "{report}"
    );
}

/// A `--target-triple` filter restricts analysis to the matching set: the same
/// commits host a regressing Linux series and a flat Windows series, and each
/// triple selection sees only its own.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_target_triple_facet_isolates_linux_from_windows() {
    let workspace = Workspace::repo(&storage_only_config());
    // Each commit carries both a Linux point (rising into a regression) and a
    // Windows point (flat).
    for (date, label, linux, windows) in [
        ("2024-01-01", "c1", 100.0, 50.0),
        ("2024-01-02", "c2", 100.0, 50.0),
        ("2024-01-03", "c3", 100.0, 50.0),
        ("2024-01-04", "c4", 130.0, 50.0),
        ("2024-01-05", "c5", 130.0, 50.0),
        ("2024-01-06", "c6", 130.0, 50.0),
    ] {
        workspace.commit_dated(date, label);
        workspace.seed_callgrind_in(
            "x86_64-unknown-linux-gnu",
            HARNESS_AUTO_MACHINE_KEY,
            label,
            linux,
        );
        workspace.seed_callgrind_in(
            "x86_64-pc-windows-msvc",
            HARNESS_AUTO_MACHINE_KEY,
            label,
            windows,
        );
    }

    // The Linux triple sees the regression.
    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&["analyze", "--target-triple", "x86_64-unknown-linux-gnu"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the Linux series regresses");

    // The Windows triple sees only the flat series.
    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--target-triple", "x86_64-pc-windows-msvc"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 0, "the Windows series is flat: {report}");
}

/// From a feature checkout, `--context master` analyzes the default branch's
/// official line (clean only), independent of the currently checked-out branch.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_branch_selects_official_line_from_a_feature_checkout() {
    let workspace = Workspace::repo(&storage_only_config());
    // Master carries a clean sustained regression.
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_callgrind("c3", 100.0);
    workspace.commit_dated("2024-01-04", "c4");
    workspace.seed_callgrind("c4", 130.0);
    workspace.commit_dated("2024-01-05", "c5");
    workspace.seed_callgrind("c5", 130.0);
    workspace.commit_dated("2024-01-06", "c6");
    workspace.seed_callgrind("c6", 130.0);
    // A feature branch with an unrelated dirty improvement that master must ignore.
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-07", "f1");
    workspace.seed_dirty_callgrind("2024-01-07", "f1", 10.0);

    let report = workspace
        .drive_json(&["analyze", "--context", "master"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["regressions"], 1,
        "the master regression is selected, the feature snapshot ignored: {report}"
    );
    assert_eq!(parsed["findings"][0]["latest"], 130.0, "{report}");
}

/// The official line of a branch that contains merge commits follows first-parent
/// topology: a run stored on a merge commit is on the line, but runs on the
/// merged-in side branch's own commits are not. This is the common real-world
/// shape (a pull request merged into `master`), which the flat-prefix model never
/// exercised.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_official_line_follows_first_parent_across_a_merge() {
    let workspace = Workspace::repo(&storage_only_config());
    // master:  root - c1 - M - c3 - c4 - c5 - c6   (M merges the side branch in)
    //                  \   /
    //  side:            sf1 - sf2
    workspace.commit("c1");
    workspace.checkout_new_branch("side");
    workspace.commit("sf1");
    workspace.commit("sf2");
    workspace.checkout("master");
    workspace.merge("side", "M");
    workspace.commit("c3");
    workspace.commit("c4");
    workspace.commit("c5");
    workspace.commit("c6");

    // The first-parent line carries a clean sustained regression on its tail.
    workspace.seed_callgrind("c1", 100.0);
    workspace.seed_callgrind("M", 100.0);
    workspace.seed_callgrind("c3", 100.0);
    workspace.seed_callgrind("c4", 130.0);
    workspace.seed_callgrind("c5", 130.0);
    workspace.seed_callgrind("c6", 130.0);
    // Side-branch points sit on the second-parent side and must never leak into the
    // official line; they carry wild values that would distort the series if read.
    workspace.seed_callgrind("sf1", 999.0);
    workspace.seed_callgrind("sf2", 999.0);

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["runs"], 6,
        "only the first-parent line c1 -> M -> c3 -> c4 -> c5 -> c6 is analyzed, not \
         the side branch: {report}"
    );
    assert_eq!(
        parsed["regressions"], 1,
        "the regression on the tip commit flags once the merge commit is on the line: {report}"
    );
    assert_eq!(parsed["findings"][0]["latest"], 130.0, "{report}");
}

/// When a feature branch merges the default branch *in* (so the merge-base sits
/// off the feature's first-parent chain), the selection treats the whole feature
/// line as target-side and admits dirty snapshots on every selected commit —
/// including ones that a plain branch-point split would have left base-side and
/// clean-only. The merged-in default-branch commits stay off the line.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_feature_that_merged_master_admits_dirty_off_chain_merge_base() {
    let workspace = Workspace::repo(&storage_only_config());
    // master:  root - c1 - c2 - c3
    //                  \
    // feature:          f1 - M - f2   (M merges master's tip c3 into feature)
    //                        /
    // merge-base(feature, master) = c3, which is NOT on feature's first-parent
    // chain [root, c1, f1, M, f2].
    workspace.commit("c1");
    workspace.checkout_new_branch("feature");
    workspace.commit("f1");
    workspace.checkout("master");
    workspace.commit("c2");
    workspace.commit("c3");
    workspace.checkout("feature");
    workspace.merge("master", "M");
    workspace.commit("f2");

    // Clean points along the feature's first-parent line.
    workspace.seed_callgrind("c1", 100.0);
    workspace.seed_callgrind("f1", 100.0);
    workspace.seed_callgrind("f2", 100.0);
    // A dirty snapshot on c1. Without the merge, c1 would be base-side (clean
    // only) and this would be excluded; the off-chain merge-base makes the whole
    // line target-side, so it is admitted.
    workspace.seed_dirty_callgrind("2024-01-03", "c1", 200.0);
    // Merged-in master commits c2/c3 are off the first-parent line and excluded.
    workspace.seed_callgrind("c2", 999.0);
    workspace.seed_callgrind("c3", 999.0);

    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    // Loaded: c1 clean, c1 dirty, f1 clean, f2 clean = 4. The dirty c1 being
    // counted proves the off-chain merge-base admitted it; c2/c3 being absent
    // proves first-parent selection excluded the merged-in commits (otherwise the
    // count would be 6).
    assert_eq!(
        parsed["runs"], 4,
        "off-chain merge-base admits the dirty base-side snapshot and excludes the \
         merged-in commits: {report}"
    );
}

/// Two machine-key partitions on the same engine/triple stay isolated: a rising
/// (regressing) series on one machine and a flat series on the other never merge
/// into one series, so exactly one set regresses. This guards the series grouping
/// key keeping `machine` — dropping it would cross-compare the two machines.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_criterion_machine_keys_stay_isolated() {
    let workspace = Workspace::repo(&storage_only_config());
    // Same commits, same Windows/x86_64 triple, two distinct machine keys. `mk-flat`
    // stays flat across the full d1..d8 history (the same commits the rising machine
    // uses), so it is present at the tip and never regresses.
    workspace.seed_rising_criterion_history("mk-rising");
    for label in ["d1", "d2", "d3", "d4", "d5", "d6", "d7", "d8"] {
        workspace.seed_criterion(label, "mk-flat", 20.0);
    }

    let report = workspace
        .drive_json(&[
            "analyze",
            "--target-triple",
            "x86_64-pc-windows-msvc",
            "--machine-key",
            "all",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    let sets = parsed["sets"].as_array().unwrap();
    assert_eq!(
        sets.len(),
        2,
        "the two machine keys form two comparable sets: {report}"
    );
    assert_eq!(
        parsed["regressions"], 1,
        "only the rising machine regresses; the machines do not cross-compare: {report}"
    );
}

/// `--target-triple` selects a single comparable set by its whole partition value:
/// the same commits host a regressing `x86_64-unknown-linux-gnu` series and a flat
/// `aarch64-unknown-linux-gnu` series, and each `--target-triple` selection sees
/// only its own.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_target_triple_facet_selects_one_set() {
    let workspace = Workspace::repo(&storage_only_config());
    for (date, label, x64, arm) in [
        ("2024-01-01", "c1", 100.0, 50.0),
        ("2024-01-02", "c2", 100.0, 50.0),
        ("2024-01-03", "c3", 100.0, 50.0),
        ("2024-01-04", "c4", 130.0, 50.0),
        ("2024-01-05", "c5", 130.0, 50.0),
        ("2024-01-06", "c6", 130.0, 50.0),
    ] {
        workspace.commit_dated(date, label);
        workspace.seed_callgrind_in(
            "x86_64-unknown-linux-gnu",
            HARNESS_AUTO_MACHINE_KEY,
            label,
            x64,
        );
        workspace.seed_callgrind_in(
            "aarch64-unknown-linux-gnu",
            HARNESS_AUTO_MACHINE_KEY,
            label,
            arm,
        );
    }

    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&["analyze", "--target-triple", "x86_64-unknown-linux-gnu"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the x86_64 triple regresses");

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--target-triple", "aarch64-unknown-linux-gnu"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 0, "the aarch64 triple is flat: {report}");
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_machine_key_facet_selects_one_set() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_criterion_history("mk-rising");
    workspace.commit_dated("2024-02-01", "d1");
    workspace.seed_criterion("d1", "mk-flat", 20.0);
    workspace.commit_dated("2024-02-02", "d2");
    workspace.seed_criterion("d2", "mk-flat", 20.0);
    workspace.commit_dated("2024-02-03", "d3");
    workspace.seed_criterion("d3", "mk-flat", 20.0);
    workspace.commit_dated("2024-02-04", "d4");
    workspace.seed_criterion("d4", "mk-flat", 20.0);

    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&[
            "analyze",
            "--target-triple",
            "x86_64-pc-windows-msvc",
            "--machine-key",
            "mk-rising",
        ])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the rising machine regresses");

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&[
            "analyze",
            "--target-triple",
            "x86_64-pc-windows-msvc",
            "--machine-key",
            "mk-flat",
        ])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "the flat machine does not regress: {report}"
    );
}

/// The dirty/feature-branch admission path works for Criterion too (not only
/// Callgrind): a dirty Criterion regression on a feature commit flags by default
/// and is suppressed by `--no-dirty`.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_criterion_feature_branch_admits_dirty_snapshots() {
    let workspace = Workspace::repo(&storage_only_config());
    // A flat clean Criterion baseline on master.
    workspace.commit_dated("2024-02-01", "c1");
    workspace.seed_criterion("c1", "mk", 20.0);
    workspace.commit_dated("2024-02-02", "c2");
    workspace.seed_criterion("c2", "mk", 20.0);
    workspace.commit_dated("2024-02-03", "c3");
    workspace.seed_criterion("c3", "mk", 20.0);
    // Branch off master, add a clean point, then several dirty snapshots that step
    // up — enough points on each side for the rank-sum gate to clear the noise.
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-02-04", "f1");
    workspace.seed_criterion("f1", "mk", 20.0);
    workspace.seed_dirty_criterion("2024-02-05", "f1", "mk", 40.0);
    workspace.seed_dirty_criterion("2024-02-06", "f1", "mk", 40.0);
    workspace.seed_dirty_criterion("2024-02-07", "f1", "mk", 40.0);
    workspace.seed_dirty_criterion("2024-02-08", "f1", "mk", 40.0);

    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&[
            "analyze",
            "--target-triple",
            "x86_64-pc-windows-msvc",
            "--machine-key",
            "mk",
        ])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 1,
        "the dirty Criterion feature snapshot should flag"
    );

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&[
            "analyze",
            "--target-triple",
            "x86_64-pc-windows-msvc",
            "--machine-key",
            "mk",
            "--no-dirty",
        ])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "with --no-dirty only the flat clean Criterion series remains: {report}"
    );
}

/// `analyze` ignores "ghost" benchmarks — ones no longer present at the context
/// commit — so a regression on a since-removed benchmark is not re-flagged.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_excludes_ghost_benchmarks() {
    let workspace = Workspace::repo(&storage_only_config());
    // `bench_000` runs flat through the tip; `bench_001` climbs into a regression but
    // is dropped from the suite at the tip commit, so it is a ghost there.
    for (date, label, values) in [
        ("2024-01-01", "c1", vec![100.0, 100.0]),
        ("2024-01-02", "c2", vec![100.0, 100.0]),
        ("2024-01-03", "c3", vec![100.0, 100.0]),
        ("2024-01-04", "c4", vec![100.0, 130.0]),
        ("2024-01-05", "c5", vec![100.0, 130.0]),
        ("2024-01-06", "c6", vec![100.0, 130.0]),
    ] {
        workspace.commit_dated(date, label);
        workspace.seed_many_callgrind(label, &values);
    }
    // The tip carries only the surviving benchmark; `bench_001` is absent here.
    workspace.commit_dated("2024-01-07", "c7");
    workspace.seed_many_callgrind("c7", &[100.0]);

    // The ghost's regression is filtered out, and the JSON reports the count.
    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["regressions"], 0, "{report}");
    assert_eq!(parsed["ghosts_excluded"], 1, "{report}");
    assert!(
        !report.contains("many::bench_001"),
        "the ghost benchmark must not appear: {report}"
    );
}

/// The rendered chart's rows: every axis row carries a `┤` or `┼` tick and the
/// chart is always drawn uncolored, so filtering on those glyphs isolates the plot
/// from the surrounding (possibly colored) report prose.
fn chart_lines(report: &str) -> Vec<&str> {
    report
        .lines()
        .filter(|line| line.contains('┤') || line.contains('┼'))
        .collect()
}

/// The chart's rendered width in characters — the widest of its rows. Two charts
/// spanning the same number of topology columns share this width even when one
/// carries a trailing gap, so a width match proves the fill reached the tip column.
fn chart_width(report: &str) -> usize {
    chart_lines(report)
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
}

/// A feature branch whose comparison base lags the merge-base — the newest base
/// data sits one commit behind the branch point — both warns in prose and still
/// draws the branch chart. That trailing lag is the visual form of the warning: the
/// chart spans the base history and the tip with the gap columns in between.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_branch_comparison_base_lag_warns_and_charts_the_gap() {
    let workspace = Workspace::repo(&storage_only_config());
    // Base data reaches only c3; the c4 merge-base carries none, so the branch's
    // comparison base ends one commit behind the branch point.
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_callgrind("c1", 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_callgrind("c2", 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_callgrind("c3", 100.0);
    // c4 is the merge-base and is intentionally left unseeded.
    workspace.commit_dated("2024-01-04", "c4");
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-05", "f1");
    workspace.seed_callgrind("f1", 130.0);
    workspace.commit_dated("2024-01-06", "f2");
    workspace.seed_callgrind("f2", 130.0);
    workspace.commit_dated("2024-01-07", "f3");
    workspace.seed_callgrind("f3", 130.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace.drive(&["analyze"]).await.unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 1,
        "the branch tip regresses against the base: {report}"
    );
    assert!(
        report.contains(
            "Warning: comparison base is 1 commit behind base \
             (no base data at more recent commits)"
        ),
        "the one-commit comparison-base lag is disclosed: {report}"
    );
    assert!(
        !chart_lines(&report).is_empty(),
        "the branch finding draws a chart: {report}"
    );
}

/// In history mode a benchmark whose latest observation for one metric lags the
/// analyzed tip renders a trailing gap: the chart still spans every commit through
/// the tip, with a blank column where the tip carries no datum for that metric. A
/// sibling metric keeps the benchmark present at the tip, so the lagging metric's
/// series is retained rather than dropped as a ghost.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_history_no_newer_data_renders_a_trailing_gap() {
    // Two identical seven-commit histories. `conditional_branches` steps up at c4
    // (a sustained regression); `instruction_count` stays flat and never flags. Only
    // the tip commit c7 differs: the full run records both metrics there while the
    // lagging run records instruction_count alone, so the benchmark stays present at
    // the tip (not a ghost) yet its conditional_branches series ends at c6.
    let steps = [
        ("2024-01-01", "c1", 100.0),
        ("2024-01-02", "c2", 100.0),
        ("2024-01-03", "c3", 100.0),
        ("2024-01-04", "c4", 130.0),
        ("2024-01-05", "c5", 130.0),
        ("2024-01-06", "c6", 130.0),
    ];

    let full = Workspace::repo(&storage_only_config());
    let lag = Workspace::repo(&storage_only_config());
    for workspace in [&full, &lag] {
        for (date, label, cb) in steps {
            workspace.commit_dated(date, label);
            workspace.seed_metrics(
                label,
                vec![
                    Metric::new(MetricKind::InstructionCount, 500.0),
                    Metric::new(MetricKind::ConditionalBranches, cb),
                ],
            );
        }
    }
    // The shared tip commit c7: the full run records both metrics; the lagging run
    // records only the sibling, leaving conditional_branches one commit behind.
    full.commit_dated("2024-01-07", "c7");
    full.seed_metrics(
        "c7",
        vec![
            Metric::new(MetricKind::InstructionCount, 500.0),
            Metric::new(MetricKind::ConditionalBranches, 130.0),
        ],
    );
    lag.commit_dated("2024-01-07", "c7");
    lag.seed_metrics("c7", vec![Metric::new(MetricKind::InstructionCount, 500.0)]);

    let RunOutcome::Analyzed {
        regressions: full_regressions,
        report: full_report,
        ..
    } = full.drive(&["analyze"]).await.unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    let RunOutcome::Analyzed {
        regressions: lag_regressions,
        report: lag_report,
        ..
    } = lag.drive(&["analyze"]).await.unwrap()
    else {
        panic!("expected an analyzed outcome");
    };

    assert_eq!(
        full_regressions, 1,
        "the full history flags the conditional_branches step: {full_report}"
    );
    assert_eq!(
        lag_regressions, 1,
        "the lagging history flags the same step even though the tip lacks the metric: {lag_report}"
    );

    // The lagging series ends at c6 but its chart is filled with a trailing gap column
    // up to the analyzed tip c7, so it spans exactly as many columns — and renders
    // exactly as wide — as the full history's chart. Without that trailing fill the
    // lagging chart would stop one column short of the tip and be one character
    // narrower. (`rasciigraph` draws a trailing gap and a continued flat plateau
    // identically — both a blank final column — so the rendered width, not the glyph
    // content, is what proves the fill reached the tip.)
    assert!(
        chart_width(&full_report) > 0,
        "the full history draws a chart: {full_report}"
    );
    assert_eq!(
        chart_width(&lag_report),
        chart_width(&full_report),
        "the trailing 'no newer data' gap extends the lagging chart to the tip\n\
         FULL:\n{full_report}\nLAG:\n{lag_report}"
    );
}
