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

/// `--format json` renders a structured, parseable report.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_json_format_is_structured() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let outcome = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .unwrap();
    let RunOutcome::Analyzed { report, .. } = outcome else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["project"], "testproj");
    assert_eq!(parsed["regressions"], 1);
    assert_eq!(parsed["findings"][0]["direction"], "regression");
}

/// `--format markdown` renders a findings table.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_markdown_format_renders_table() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let outcome = workspace
        .drive(&["analyze", "--format", "markdown"])
        .await
        .unwrap();
    let RunOutcome::Analyzed { report, .. } = outcome else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
    assert!(
        report.contains("# Benchmark history analysis: testproj"),
        "{report}"
    );
    assert!(report.contains("| Change | Direction |"), "{report}");
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

    let outcome = workspace
        .drive(&["analyze", "--since", "2024-01-01", "--format", "json"])
        .await
        .unwrap();
    let RunOutcome::Analyzed { report, .. } = outcome else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
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
        "v1/testproj/criterion/x86_64-pc-windows-msvc/m1/abc123/clean.json",
        &ir_result_set(1, "c1", 100.0),
    );

    let outcome = workspace
        .drive(&["analyze", "--engine", "callgrind", "--format", "json"])
        .await
        .unwrap();
    let RunOutcome::Analyzed { report, .. } = outcome else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["runs"], 1,
        "only the callgrind object is loaded: {report}"
    );
}

/// An unknown `--format` is reported as an analysis error.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_rejects_unknown_format() {
    let workspace = Workspace::repo(&storage_only_config());

    let error = workspace
        .drive(&["analyze", "--format", "yaml"])
        .await
        .unwrap_err();
    let RunError::Analyze { message } = error else {
        panic!("expected an analyze error, got {error:?}");
    };
    assert!(message.contains("unknown report format"), "{message}");
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

/// Cache hit counts are higher-is-better, so a sustained drop is a regression
/// end to end (guards the metric-polarity logic through the real pipeline).
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_falling_cache_hits_is_a_regression() {
    let workspace = Workspace::repo(&storage_only_config());
    for (date, commit, hits) in [
        ("2024-01-01", "c1", 1000.0),
        ("2024-01-02", "c2", 1000.0),
        ("2024-01-03", "c3", 1000.0),
        ("2024-01-04", "c4", 700.0),
        ("2024-01-05", "c5", 700.0),
        ("2024-01-06", "c6", 700.0),
    ] {
        workspace.commit_dated(date, commit);
        workspace.seed_metrics(commit, vec![Metric::new(MetricKind::L1CacheHits, hits)]);
    }

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
    assert_eq!(regressions, 1, "fewer cache hits is a regression: {report}");
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["findings"][0]["direction"], "regression");
    assert_eq!(parsed["findings"][0]["kind"], "l1_cache_hits");
}

/// Conversely, a rise in L1 cache hits is an improvement, not a regression.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_rising_l1_cache_hits_is_an_improvement() {
    let workspace = Workspace::repo(&storage_only_config());
    for (date, commit, hits) in [
        ("2024-01-01", "c1", 700.0),
        ("2024-01-02", "c2", 700.0),
        ("2024-01-03", "c3", 700.0),
        ("2024-01-04", "c4", 1000.0),
        ("2024-01-05", "c5", 1000.0),
        ("2024-01-06", "c6", 1000.0),
    ] {
        workspace.commit_dated(date, commit);
        workspace.seed_metrics(commit, vec![Metric::new(MetricKind::L1CacheHits, hits)]);
    }

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json", "--include-improvements"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "more L1 cache hits is not a regression: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["findings"][0]["direction"], "improvement");
}

/// The deeper cache tiers (`LLhits`, `RamHits`) are the expensive ones: an access
/// served there missed the faster levels, so more of them is worse. A sustained
/// rise in `RamHits` must therefore read as a regression end to end.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_rising_ram_hits_is_a_regression() {
    let workspace = Workspace::repo(&storage_only_config());
    for (date, commit, hits) in [
        ("2024-01-01", "c1", 700.0),
        ("2024-01-02", "c2", 700.0),
        ("2024-01-03", "c3", 700.0),
        ("2024-01-04", "c4", 1000.0),
        ("2024-01-05", "c5", 1000.0),
        ("2024-01-06", "c6", 1000.0),
    ] {
        workspace.commit_dated(date, commit);
        workspace.seed_metrics(commit, vec![Metric::new(MetricKind::RamHits, hits)]);
    }

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
    assert_eq!(regressions, 1, "more RAM hits is a regression: {report}");
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["findings"][0]["direction"], "regression");
    assert_eq!(parsed["findings"][0]["kind"], "ram_hits");
}

/// Two benchmarks regressing by different magnitudes produce two findings that the
/// report ranks largest-relative-move first, end to end through real storage and
/// rendering.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_ranks_findings_by_relative_move_across_benchmarks() {
    let workspace = Workspace::repo(&storage_only_config());
    // `alpha` doubles (+100%); `beta` ticks up ~2%.
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_two_benchmarks("c1", 100.0, 100.0);
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_two_benchmarks("c2", 100.0, 100.0);
    workspace.commit_dated("2024-01-03", "c3");
    workspace.seed_two_benchmarks("c3", 100.0, 100.0);
    workspace.commit_dated("2024-01-04", "c4");
    workspace.seed_two_benchmarks("c4", 200.0, 102.0);
    workspace.commit_dated("2024-01-05", "c5");
    workspace.seed_two_benchmarks("c5", 200.0, 102.0);
    workspace.commit_dated("2024-01-06", "c6");
    workspace.seed_two_benchmarks("c6", 200.0, 102.0);

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
    assert_eq!(regressions, 2, "both benchmarks regress: {report}");

    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    let findings = parsed["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 2, "{report}");
    // The larger relative move (alpha, +100%) ranks ahead of the smaller (beta, +2%).
    assert_eq!(findings[0]["segments"][0], "alpha", "{report}");
    assert_eq!(findings[0]["segments"][1], "alpha::bench", "{report}");
    assert_eq!(findings[1]["segments"][0], "beta", "{report}");

    // The Markdown rendering preserves the same ranked order in its table rows.
    let RunOutcome::Analyzed {
        report: markdown, ..
    } = workspace
        .drive(&["analyze", "--format", "markdown"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
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

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json", "--include-improvements"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "a drop in instructions is not a regression: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
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
            "--format",
            "json",
        ])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "jitter must not read as a regression: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert!(
        parsed["findings"].as_array().unwrap().is_empty(),
        "noisy jitter around a flat mean produces no findings at all: {report}"
    );
}

/// A slow, monotonic Criterion `wall_time` drift is flagged as a `drift` finding
/// (not a change-point): the Mann-Kendall trend is significant, the Theil-Sen line
/// fits the gentle ramp better than any single step, and the total movement clears
/// the noise floor. This exercises the second detector and the model-fit
/// arbitration that routes ramps to drift and steps to change-point.
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
            "--format",
            "json",
        ])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the upward drift is a regression: {report}");
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["findings"][0]["method"], "drift", "{report}");
    assert_eq!(parsed["findings"][0]["direction"], "regression", "{report}");
}

/// A Callgrind series whose instruction count steps up by a single count - far
/// below the 3% practical floor a noisy engine demands - is still flagged, because
/// deterministic engines carry no measurement noise and trust any sustained step.
/// This proves the engine-aware split: the same tiny move would be discarded as
/// noise on a Criterion series but is a real change on a deterministic one.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_callgrind_tiny_deterministic_step_is_flagged() {
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
        "a one-count deterministic step is real and must flag below the noise floor: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["findings"][0]["method"], "change_point", "{report}");
    assert_eq!(parsed["findings"][0]["direction"], "regression", "{report}");
}

/// An `alloc_tracker` series whose allocated-bytes count steps up is flagged as a
/// `change_point`: allocation statistics are a deterministic property of the code,
/// so - like Callgrind instruction counts - any sustained step is a real change,
/// trusted below the noise floor a noisy engine would demand.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_alloc_tracker_step_is_flagged_as_change_point() {
    let workspace = Workspace::repo(&storage_only_config());
    // A flat allocated-bytes baseline that steps up by one byte at the fourth
    // commit; the allocation count stays constant throughout.
    workspace.commit_dated("2024-03-01", "a1");
    workspace.seed_alloc_tracker("a1", "allocate_vec", 200.0, 2.0);
    workspace.commit_dated("2024-03-02", "a2");
    workspace.seed_alloc_tracker("a2", "allocate_vec", 200.0, 2.0);
    workspace.commit_dated("2024-03-03", "a3");
    workspace.seed_alloc_tracker("a3", "allocate_vec", 200.0, 2.0);
    workspace.commit_dated("2024-03-04", "a4");
    workspace.seed_alloc_tracker("a4", "allocate_vec", 201.0, 2.0);
    workspace.commit_dated("2024-03-05", "a5");
    workspace.seed_alloc_tracker("a5", "allocate_vec", 201.0, 2.0);
    workspace.commit_dated("2024-03-06", "a6");
    workspace.seed_alloc_tracker("a6", "allocate_vec", 201.0, 2.0);

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
        "a one-byte deterministic step is real and must flag: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
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
    workspace.commit_dated("2024-04-01", "g1");
    workspace.seed_alloc_tracker("g1", "allocate_vec", 200.0, 2.0);
    workspace.commit_dated("2024-04-02", "g2");
    workspace.seed_alloc_tracker("g2", "free_box", 8.0, 1.0);
    workspace.commit_dated("2024-04-03", "g3");
    workspace.seed_alloc_tracker("g3", "allocate_vec", 200.0, 2.0);
    workspace.commit_dated("2024-04-04", "g4");
    workspace.seed_alloc_tracker("g4", "free_box", 8.0, 1.0);
    workspace.commit_dated("2024-04-05", "g5");
    workspace.seed_alloc_tracker("g5", "allocate_vec", 260.0, 2.0);
    workspace.commit_dated("2024-04-06", "g6");
    workspace.seed_alloc_tracker("g6", "allocate_vec", 260.0, 2.0);

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
    // Only `allocate_vec`'s allocated-bytes step is a finding. A gap read as a drop
    // to zero would manufacture wild swings; instead the series is contiguous.
    assert_eq!(
        regressions, 1,
        "the gapped series' real step flags: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["findings"][0]["method"], "change_point", "{report}");
    assert_eq!(parsed["findings"][0]["direction"], "regression", "{report}");
    assert!(report.contains("allocate_vec"), "{report}");
    assert!(
        !report.contains("free_box"),
        "the flat two-point free_box series produces no finding: {report}"
    );
}

/// A slow, monotonic `all_the_time` processor-time drift is flagged as a `drift`
/// finding. Processor time is a noisy, hardware-dependent measurement (like
/// Criterion wall time), so a gentle ramp clears the noise floor only via the
/// trend detector - even though `all_the_time` reports no confidence interval, the
/// detector falls back gracefully to the Mann-Kendall trend and practical floor.
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

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&[
            "analyze",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--machine-key",
            "mk",
            "--format",
            "json",
        ])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the upward drift is a regression: {report}");
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
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

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&[
            "analyze",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--machine-key",
            "mk",
            "--format",
            "json",
        ])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "jitter must not read as a regression: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert!(
        parsed["findings"].as_array().unwrap().is_empty(),
        "noisy jitter around a flat mean produces no findings at all: {report}"
    );
}

/// A sustained processor-time step whose per-regime confidence intervals are
/// tight and non-overlapping clears the noise detector's CI gate and is reported
/// as a regression. This proves the bootstrap confidence interval the
/// `all_the_time` engine now records flows through the adapter into the
/// CI-non-overlap gate for processor time.
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

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&[
            "analyze",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--machine-key",
            "mk",
            "--format",
            "json",
        ])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 1,
        "a sustained step with disjoint intervals is a regression: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
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

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&[
            "analyze",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--machine-key",
            "mk",
            "--format",
            "json",
        ])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "an overlapping-interval step is suppressed by the CI gate: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
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
    assert_eq!(regressions, 0, "the dirty spike must be excluded: {report}");
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["runs"], 4,
        "only the four clean runs are loaded, not the dirty snapshot: {report}"
    );
}

/// When every stored run on the default branch is a dirty (uncommitted-tree)
/// snapshot — the trap a user hits when the configuration file is never committed,
/// so each `run` records a `dirty-*.json` on the base-side tip — `analyze` finds
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

    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
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
    // Two dirty regression snapshots on the tip commit (c3) complete a sustained
    // step over the clean baseline.
    workspace.seed_dirty_callgrind("2024-01-04", "c3", 130.0);
    workspace.seed_dirty_callgrind("2024-01-05", "c3", 130.0);
    // Make the working tree genuinely dirty (an uncommitted, non-ignored file),
    // matching the "evaluating the tool" / "working on the base branch" scenario.
    workspace.make_dirty("uncommitted.txt");

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
        "the dirty tip regression is admitted: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["runs"], 5,
        "the dirty tip snapshots join the three clean runs: {report}"
    );
    let warning = parsed["warning"].as_str().unwrap();
    assert!(
        warning.contains("Switch to a new branch"),
        "the warning should advise switching to a branch: {warning}"
    );

    // `--no-dirty` skips the probe and the exception, so the dirty tip is dropped.
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json", "--no-dirty"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
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
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["mode"], "history",
        "a dirty tree with only clean runs is still the official history view: {report}"
    );
    assert_eq!(
        regressions, 1,
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
    // Branch off master and add a clean point plus two dirty regression snapshots
    // on it (two raised points satisfy the change-point persistence requirement).
    workspace.checkout_new_branch("feature");
    workspace.commit_dated("2024-01-04", "f1");
    workspace.seed_callgrind("f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-05", "f1", 200.0);
    workspace.seed_dirty_callgrind("2024-01-06", "f1", 200.0);

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
        "topology order must place the 130 step last and flag it: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["findings"][0]["baseline"], 100.0, "{report}");
    assert_eq!(parsed["findings"][0]["latest"], 130.0, "{report}");
}

/// Branch mode judges a feature branch by its *latest* regime, not its journey:
/// a branch that first improved and then regressed is reported as a regression,
/// and `flipped_at` points at the commit where the latest (worse) regime began.
/// The JSON also advertises the resolved mode and the downstream `notable` signal.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_branch_mode_reports_the_latest_regime_with_a_flip() {
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
        "the latest (worse) regime must be reported, not the intermediate improvement: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["mode"], "branch", "{report}");
    assert_eq!(parsed["notable"], true, "{report}");
    let finding = &parsed["findings"][0];
    assert_eq!(finding["direction"], "regression", "{report}");
    assert_eq!(finding["baseline"], 100.0, "{report}");
    assert_eq!(finding["latest"], 130.0, "{report}");
    assert!(
        finding["flipped_at"].is_string(),
        "the flip should name the commit the latest regime began at: {report}"
    );
    assert!(
        finding["series"].as_array().is_some_and(|s| !s.is_empty()),
        "the finding should carry the underlying series for charting: {report}"
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
    assert_eq!(regressions, 0, "a flat branch must not flag: {report}");
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
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
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["mode"], "history", "{report}");
    assert_eq!(parsed["notable"], false, "{report}");
    assert_eq!(parsed["improvements"], 0, "{report}");
    assert!(
        parsed["findings"].as_array().is_some_and(Vec::is_empty),
        "improvements are suppressed by default in history mode: {report}"
    );

    // Opting in surfaces the very same improvement.
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json", "--include-improvements"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["improvements"], 1, "{report}");
    assert_eq!(
        parsed["findings"][0]["direction"], "improvement",
        "{report}"
    );
}

/// `--mode tip` overrides auto-detection (which would pick history on a clean base
/// checkout) and runs the fast tip guard: it flags a regression at the latest
/// commit but stays silent on an improvement, since the guard cares only about
/// the level getting worse.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_tip_mode_flags_only_a_tip_regression() {
    let regressing = Workspace::repo(&storage_only_config());
    regressing.commit_dated("2024-01-01", "c1");
    regressing.seed_callgrind("c1", 100.0);
    regressing.commit_dated("2024-01-02", "c2");
    regressing.seed_callgrind("c2", 100.0);
    regressing.commit_dated("2024-01-03", "c3");
    regressing.seed_callgrind("c3", 100.0);
    regressing.commit_dated("2024-01-04", "c4");
    regressing.seed_callgrind("c4", 100.0);
    regressing.commit_dated("2024-01-05", "c5");
    regressing.seed_callgrind("c5", 130.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = regressing
        .drive(&["analyze", "--mode", "tip", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the tip step up must flag: {report}");
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["mode"], "tip", "the override must win: {report}");

    let improving = Workspace::repo(&storage_only_config());
    improving.commit_dated("2024-01-01", "c1");
    improving.seed_callgrind("c1", 100.0);
    improving.commit_dated("2024-01-02", "c2");
    improving.seed_callgrind("c2", 100.0);
    improving.commit_dated("2024-01-03", "c3");
    improving.seed_callgrind("c3", 100.0);
    improving.commit_dated("2024-01-04", "c4");
    improving.seed_callgrind("c4", 100.0);
    improving.commit_dated("2024-01-05", "c5");
    improving.seed_callgrind("c5", 70.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = improving
        .drive(&["analyze", "--mode", "tip", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "tip mode reports regressions only, so a drop stays silent: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["improvements"], 0, "{report}");
}

/// An unrecognized `--mode` is a clear up-front error, not a silent fallback.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_rejects_an_unknown_mode() {
    let workspace = Workspace::repo(&storage_only_config());
    let error = workspace
        .drive(&["analyze", "--mode", "weekly"])
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("unknown analysis mode"),
        "the error should name the problem: {error}"
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
        workspace.seed_callgrind_in("x86_64-unknown-linux-gnu", "synthetic", label, linux);
        workspace.seed_callgrind_in("x86_64-pc-windows-msvc", "synthetic", label, windows);
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

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--context", "master", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 1,
        "the master regression is selected, the feature snapshot ignored: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
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
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["runs"], 6,
        "only the first-parent line c1 -> M -> c3 -> c4 -> c5 -> c6 is analyzed, not \
         the side branch: {report}"
    );
    assert_eq!(
        regressions, 1,
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

    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
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
    // Same commits, same Windows/x86_64 triple, two distinct machine keys.
    workspace.seed_rising_criterion_history("mk-rising");
    workspace.commit_dated("2024-02-01", "d1");
    workspace.seed_criterion("d1", "mk-flat", 20.0);
    workspace.commit_dated("2024-02-02", "d2");
    workspace.seed_criterion("d2", "mk-flat", 20.0);
    workspace.commit_dated("2024-02-03", "d3");
    workspace.seed_criterion("d3", "mk-flat", 20.0);
    workspace.commit_dated("2024-02-04", "d4");
    workspace.seed_criterion("d4", "mk-flat", 20.0);

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
            "all",
            "--format",
            "json",
        ])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    let sets = parsed["sets"].as_array().unwrap();
    assert_eq!(
        sets.len(),
        2,
        "the two machine keys form two comparable sets: {report}"
    );
    assert_eq!(
        regressions, 1,
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
        workspace.seed_callgrind_in("x86_64-unknown-linux-gnu", "synthetic", label, x64);
        workspace.seed_callgrind_in("aarch64-unknown-linux-gnu", "synthetic", label, arm);
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

/// Loads a history larger than one parse batch so the loader's mid-stream batch
/// flush actually fires.
///
/// `select_dataset` parses fetched objects in bounded batches and only flushes a
/// batch mid-stream once it fills; a dataset that fits in a single batch never
/// exercises that path (the final remainder flush handles it instead). A large CI
/// fleet running the same wall-clock benchmark on many machines produces one
/// comparable set per machine key, so analyzing all of them (`--machine-key all`)
/// reaches a multi-batch object count with a handful of commits rather than
/// hundreds. Every machine's series carries the same rising step, so each is
/// flagged — proving all the objects were fetched, parsed and folded through the
/// batched load.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_loads_a_history_larger_than_one_parse_batch() {
    let workspace = Workspace::repo(&storage_only_config());

    // The four-flat-then-four-raised shape mirrors `seed_rising_criterion_history`,
    // the proven wall-clock regression fixture.
    let history = [
        ("2024-02-01", "d1", 20.0),
        ("2024-02-02", "d2", 20.0),
        ("2024-02-03", "d3", 20.0),
        ("2024-02-04", "d4", 20.0),
        ("2024-02-05", "d5", 30.0),
        ("2024-02-06", "d6", 30.0),
        ("2024-02-07", "d7", 30.0),
        ("2024-02-08", "d8", 30.0),
    ];

    // Fan the same rising series across enough machine-key partitions that the
    // loaded object count clears two full parse batches. Every commit stores one
    // object per machine, so `machines * commits` objects are fetched, and the loader
    // only flushes a batch mid-stream once that count exceeds its parse batch
    // (`PARSE_CHUNK`). Sizing the fan-out from `PARSE_CHUNK` keeps this path covered
    // even if the batch size is later raised, rather than hard-coding a margin that
    // could silently erode. A large CI fleet running one wall-clock benchmark across
    // many machines produces exactly this shape (`--machine-key all`).
    let commits = history.len();
    let machines = cargo_bench_history::__private::PARSE_CHUNK
        .saturating_mul(2)
        .div_ceil(commits)
        .saturating_add(1);
    for (date, label, value) in history {
        workspace.commit_dated(date, label);
        for machine in 0..machines {
            workspace.seed_criterion(label, &format!("mk-{machine}"), value);
        }
    }

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
            "all",
            "--format",
            "json",
        ])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, machines,
        "every machine partition carries the same rising step, so each one regresses\n{report}"
    );

    // Prove the load actually drove the mid-stream flush rather than a single
    // remainder flush: the report counts every object that entered the analysis, all
    // of them must have loaded, and that count must exceed one parse batch (which is
    // exactly when a flush fires).
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    let runs = usize::try_from(
        parsed["runs"]
            .as_u64()
            .expect("the report carries a numeric runs count"),
    )
    .unwrap();
    assert_eq!(
        runs,
        machines.saturating_mul(commits),
        "every seeded object should load through the batched path\n{report}"
    );
    assert!(
        runs > cargo_bench_history::__private::PARSE_CHUNK,
        "the fixture must clear one parse batch so a mid-stream flush fires: {runs} runs vs \
         PARSE_CHUNK {}",
        cargo_bench_history::__private::PARSE_CHUNK
    );
}
