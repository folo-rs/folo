//! Integration tests exercising the public command surface end to end: parse
//! arguments through `clap`, translate to the typed command, and dispatch through
//! `run` against the real process, filesystem-harvest, and local-storage adapters.
#![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]
#![allow(
    clippy::float_cmp,
    reason = "metric values are exact integer-derived counts"
)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cargo_bench_history::{
    BenchmarkId, CiInfo, Cli, Command, GitInfo, Metric, MetricKind, Overrides, ResultRecord,
    ResultSet, RunContext, RunError, RunOutcome, SCHEMA_VERSION, Timestamps, ToolchainInfo,
    default_template, run, run_with_overrides,
};
use jiff::Timestamp;
use serial_test::serial;

#[path = "support/cwd_guard.rs"]
mod cwd_guard;
use cwd_guard::CwdGuard;

/// The mock engine binary path, provided by Cargo for the `[[bin]]` target. It
/// writes Gungraun summary fixtures into the target tree and exits with a chosen
/// code, standing in for a real benchmark engine.
const MOCK_ENGINE: &str = env!("CARGO_BIN_EXE_cargo-bench-history-mock-engine");

/// The tool version recorded with each run. The integration test compiles within
/// the package, so its `CARGO_PKG_VERSION` matches the version `run` records.
const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A fixed clock anchor for `analyze`/`list`'s history-mode default `--since`
/// window. Seed data lands across 2024; anchoring "now" at 2024-06-01 keeps the
/// default six-month look-back (cutoff 2023-12-01) inclusive of that data, so the
/// default window does not silently drop seeded points as wall-clock time passes.
fn analysis_now() -> Timestamp {
    "2024-06-01T00:00:00Z"
        .parse::<Timestamp>()
        .expect("the fixed analysis anchor is a valid timestamp")
}

fn command_from(args: &[&str]) -> Command {
    Cli::from_args(&["cargo-bench-history"], args)
        .expect("arguments should parse")
        .into_command()
}

// ===========================================================================
// CLI surface (pure argument parsing; no IO, Miri-safe).
// ===========================================================================

#[test]
fn run_forwards_passthrough_after_separator() {
    let Command::Run(options) = command_from(&["run", "--", "--noplot"]) else {
        panic!("expected run command");
    };
    assert_eq!(options.passthrough, vec!["--noplot".to_owned()]);
}

#[test]
fn run_collects_package_and_bench_scope() {
    let Command::Run(options) = command_from(&["run", "-p", "nm", "--bench", "nm_observe"]) else {
        panic!("expected run command");
    };
    assert_eq!(options.packages, vec!["nm".to_owned()]);
    assert_eq!(options.benches, vec!["nm_observe".to_owned()]);
}

#[test]
fn unknown_subcommand_is_rejected() {
    Cli::from_args(&["cargo-bench-history"], &["frobnicate"])
        .expect_err("an unknown subcommand should be rejected");
}

#[test]
fn run_rejects_unknown_flag() {
    Cli::from_args(&["cargo-bench-history"], &["run", "--frobnicate"])
        .expect_err("an unknown flag should be rejected");
}

#[test]
fn help_request_lists_subcommands() {
    let early_exit = Cli::from_args(&["cargo-bench-history"], &["--help"])
        .expect_err("help is reported as an early exit");
    assert!(
        early_exit.output.contains("run"),
        "help should list subcommands: {}",
        early_exit.output
    );
    assert!(
        early_exit.output.contains("install"),
        "{}",
        early_exit.output
    );
}

#[test]
fn help_text_describes_each_command_in_alphabetical_order() {
    let help = Cli::help("cargo-bench-history");

    // Each command is accompanied by its description, not just its bare name.
    assert!(
        help.contains("Analyze stored history"),
        "help should describe `analyze`: {help}"
    );
    assert!(
        help.contains("Replay `run` across a range"),
        "help should describe `backfill`: {help}"
    );

    // The commands appear in alphabetical order.
    let order = ["analyze", "backfill", "install", "list", "prune", "run"];
    let positions: Vec<usize> = order
        .iter()
        .map(|name| {
            help.find(&format!("\n  {name} "))
                .unwrap_or_else(|| panic!("help should list `{name}`: {help}"))
        })
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "commands should be listed alphabetically: {help}"
    );
}

// ===========================================================================
// Binary entry point: exit codes and output stream routing.
//
// These spawn the built binary to verify that `main` distinguishes a `--help`
// early exit (success, stdout) from a parse error (failure, stderr), rather than
// inferring success from the presence of the word "help" in the output.
// ===========================================================================

#[test]
#[cfg_attr(miri, ignore)] // Spawns a real process, which Miri cannot do.
fn binary_help_exits_success_on_stdout() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-bench-history"))
        .arg("--help")
        .output()
        .expect("binary should run");

    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("run"), "help should reach stdout: {stdout}");
    assert!(
        output.stderr.is_empty(),
        "help should not write to stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[cfg_attr(miri, ignore)] // Spawns a real process, which Miri cannot do.
fn binary_parse_error_exits_failure_on_stderr() {
    // An unknown flag is a parse error whose usage text still mentions `--help`;
    // the exit code must come from the parse status, not a substring match.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-bench-history"))
        .args(["run", "--frobnicate"])
        .output()
        .expect("binary should run");

    assert!(
        !output.status.success(),
        "a parse error should exit non-zero"
    );
    assert!(
        output.stdout.is_empty(),
        "a parse error should not write to stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !output.stderr.is_empty(),
        "a parse error should write a diagnostic to stderr"
    );
}

#[test]
#[cfg_attr(miri, ignore)] // Spawns a real process, which Miri cannot do.
fn binary_without_a_subcommand_prints_descriptive_help_to_stderr() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-bench-history"))
        .output()
        .expect("binary should run");

    assert!(
        !output.status.success(),
        "a missing subcommand should exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Analyze stored history"),
        "no-subcommand help should describe the commands on stderr: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "no-subcommand help should not write to stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

// ===========================================================================
// Real-adapter `install` scenarios.
// ===========================================================================

#[tokio::test]
#[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot do.
async fn install_writes_config_to_the_default_path() {
    let workspace = Workspace::empty();

    let outcome = workspace.drive(&["install"]).await.unwrap();

    let RunOutcome::Completed { message } = outcome else {
        panic!("install should complete: {outcome:?}");
    };
    assert!(
        message.contains("Wrote a starter configuration"),
        "{message}"
    );
    assert!(message.contains("Next steps"), "{message}");
    assert_eq!(
        workspace.read(".cargo/bench_history.toml").as_deref(),
        Some(default_template()),
        "the default template should be written verbatim"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot do.
async fn install_never_overwrites_an_existing_config() {
    let workspace = Workspace::empty();
    workspace.drive(&["install"]).await.unwrap();

    // Hand-edit the generated file, then install again: it must be untouched.
    let edited = format!("{}\n# hand-edited\n", default_template());
    std::fs::write(workspace.root().join(".cargo/bench_history.toml"), &edited).unwrap();

    let outcome = workspace.drive(&["install"]).await.unwrap();

    let RunOutcome::Completed { message } = outcome else {
        panic!("install should complete: {outcome:?}");
    };
    assert!(message.contains("already exists"), "{message}");
    assert_eq!(
        workspace.read(".cargo/bench_history.toml").as_deref(),
        Some(edited.as_str()),
        "the hand-edited file must be left unchanged"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot do.
async fn install_honors_an_explicit_config_path() {
    let workspace = Workspace::empty();

    workspace
        .drive(&["install", "--config", "config/custom.toml"])
        .await
        .unwrap();

    assert_eq!(
        workspace.read("config/custom.toml").as_deref(),
        Some(default_template()),
        "the template should be written to the requested path"
    );
    assert!(
        workspace.read(".cargo/bench_history.toml").is_none(),
        "the default path should be left alone"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot do.
#[serial]
async fn run_entry_dispatches_without_a_target_root_override() {
    // The production `run` entry passes no target-root override; drive `install`
    // (which ignores it) through it so the public default path is exercised.
    let workspace = Workspace::empty();
    let _cwd = CwdGuard::enter(workspace.root());

    let result = run(&command_from(&["install"])).await;

    result.unwrap();
    assert!(
        workspace.read(".cargo/bench_history.toml").is_some(),
        "install via the production entry should write the default config"
    );
}

// ===========================================================================
// Real-adapter `analyze` scenarios.
//
// These seed a project's history directly into the local store (the same layout
// `run` writes), then drive `analyze` through the lib entry against the real
// `LocalStorage` adapter. No engine is launched. Each test operates on its own
// temporary workspace (passed explicitly through the API), so they run in
// parallel; they are miri-ignored (real filesystem).
// ===========================================================================

/// An empty history analyzes cleanly and reports nothing.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_empty_history_reports_no_changes() {
    let workspace = Workspace::repo(&storage_only_config());

    let outcome = workspace
        .drive(&["analyze"])
        .await
        .expect("analysis should succeed");
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

    let outcome = workspace
        .drive(&["analyze"])
        .await
        .expect("analysis should succeed");
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
    assert!(report.contains("Ir"), "{report}");
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
        .expect("analysis should succeed");
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

    let outcome = workspace
        .drive(&["analyze"])
        .await
        .expect("analysis should succeed");
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
        .expect("analysis should succeed");
    let RunOutcome::Analyzed { report, .. } = outcome else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
    let parsed: serde_json::Value =
        serde_json::from_str(&report).expect("the json report should parse");
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
        .expect("analysis should succeed");
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
    workspace.seed_callgrind("2020-01-01", "c0", 999.0);
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "c2", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 100.0);

    let outcome = workspace
        .drive(&["analyze", "--since", "2024-01-01", "--format", "json"])
        .await
        .expect("analysis should succeed");
    let RunOutcome::Analyzed { report, .. } = outcome else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
    let parsed: serde_json::Value =
        serde_json::from_str(&report).expect("the json report should parse");
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
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    // A criterion-partition object that the callgrind filter must skip. Its commit
    // segment is never read because the engine facet excludes it from listing.
    workspace.seed(
        "v2/testproj/criterion/x86_64-pc-windows-msvc/m1/abc123/clean.json",
        &ir_result_set(1, "c1", 100.0),
    );

    let outcome = workspace
        .drive(&["analyze", "--engine", "callgrind", "--format", "json"])
        .await
        .expect("analysis should succeed");
    let RunOutcome::Analyzed { report, .. } = outcome else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };
    let parsed: serde_json::Value =
        serde_json::from_str(&report).expect("the json report should parse");
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
        .expect_err("an unknown format should be rejected");
    let RunError::Analyze { message } = error else {
        panic!("expected an analyze error, got {error:?}");
    };
    assert!(message.contains("unknown report format"), "{message}");
}

/// `--metric` narrows analysis to one metric: a regression in another metric is
/// invisible when the filter selects a flat one.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_metric_filter_excludes_other_metrics() {
    let workspace = Workspace::repo(&storage_only_config());
    // `Ir` stays flat while `EstimatedCycles` climbs into a regression.
    for (date, commit, cycles) in [
        ("2024-01-01", "c1", 100.0),
        ("2024-01-02", "c2", 100.0),
        ("2024-01-03", "c3", 100.0),
        ("2024-01-04", "c4", 130.0),
        ("2024-01-05", "c5", 130.0),
        ("2024-01-06", "c6", 130.0),
    ] {
        workspace.seed_metrics(
            date,
            commit,
            vec![
                Metric::new(
                    "Ir".to_owned(),
                    MetricKind::InstructionCount,
                    50.0,
                    Some("count".to_owned()),
                ),
                Metric::new(
                    "EstimatedCycles".to_owned(),
                    MetricKind::EstimatedCycles,
                    cycles,
                    Some("count".to_owned()),
                ),
            ],
        );
    }

    // Unfiltered, the cycles climb flags as a regression.
    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&["analyze"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the cycles climb should flag unfiltered");

    // Restricting to the flat `Ir` metric hides it.
    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--metric", "Ir"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 0, "the Ir series is flat: {report}");
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
        workspace.seed_metrics(
            date,
            commit,
            vec![Metric::new(
                "L1hits".to_owned(),
                MetricKind::CacheEvents,
                hits,
                Some("count".to_owned()),
            )],
        );
    }

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "fewer cache hits is a regression: {report}");
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert_eq!(parsed["findings"][0]["direction"], "regression");
    assert_eq!(parsed["findings"][0]["kind"], "cache_events");
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
        workspace.seed_metrics(
            date,
            commit,
            vec![Metric::new(
                "L1hits".to_owned(),
                MetricKind::CacheEvents,
                hits,
                Some("count".to_owned()),
            )],
        );
    }

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json", "--include-improvements"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "more L1 cache hits is not a regression: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
        workspace.seed_metrics(
            date,
            commit,
            vec![Metric::new(
                "RamHits".to_owned(),
                MetricKind::CacheEvents,
                hits,
                Some("count".to_owned()),
            )],
        );
    }

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "more RAM hits is a regression: {report}");
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert_eq!(parsed["findings"][0]["direction"], "regression");
    assert_eq!(parsed["findings"][0]["kind"], "cache_events");
}

/// Two benchmarks regressing by different magnitudes produce two findings that the
/// report ranks largest-relative-move first, end to end through real storage and
/// rendering.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_ranks_findings_by_relative_move_across_benchmarks() {
    let workspace = Workspace::repo(&storage_only_config());
    // `alpha` doubles (+100%); `beta` ticks up ~2%.
    workspace.seed_two_benchmarks("2024-01-01", "c1", 100.0, 100.0);
    workspace.seed_two_benchmarks("2024-01-02", "c2", 100.0, 100.0);
    workspace.seed_two_benchmarks("2024-01-03", "c3", 100.0, 100.0);
    workspace.seed_two_benchmarks("2024-01-04", "c4", 200.0, 102.0);
    workspace.seed_two_benchmarks("2024-01-05", "c5", 200.0, 102.0);
    workspace.seed_two_benchmarks("2024-01-06", "c6", 200.0, 102.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 2, "both benchmarks regress: {report}");

    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    let findings = parsed["findings"].as_array().expect("findings array");
    assert_eq!(findings.len(), 2, "{report}");
    // The larger relative move (alpha, +100%) ranks ahead of the smaller (beta, +2%).
    assert_eq!(findings[0]["package"], "alpha", "{report}");
    assert_eq!(findings[0]["group"], "alpha::bench", "{report}");
    assert_eq!(findings[1]["package"], "beta", "{report}");

    // The Markdown rendering preserves the same ranked order in its table rows.
    let RunOutcome::Analyzed {
        report: markdown, ..
    } = workspace
        .drive(&["analyze", "--format", "markdown"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    let alpha_row = markdown.find("alpha::bench").expect("an alpha row");
    let beta_row = markdown.find("beta::bench").expect("a beta row");
    assert!(
        alpha_row < beta_row,
        "the larger move must precede the smaller:\n{markdown}"
    );
}
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_reports_improvement_without_regression() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "c2", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 100.0);
    workspace.seed_callgrind("2024-01-04", "c4", 70.0);
    workspace.seed_callgrind("2024-01-05", "c5", 70.0);
    workspace.seed_callgrind("2024-01-06", "c6", 70.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json", "--include-improvements"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "a drop in instructions is not a regression: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
        workspace.seed_criterion(date, label, "mk", value);
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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "jitter must not read as a regression: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert!(
        parsed["findings"]
            .as_array()
            .expect("a findings array")
            .is_empty(),
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
        workspace.seed_criterion(date, label, "mk", value);
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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the upward drift is a regression: {report}");
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
    workspace.seed_callgrind("2024-01-01", "c1", 1000.0);
    workspace.seed_callgrind("2024-01-02", "c2", 1000.0);
    workspace.seed_callgrind("2024-01-03", "c3", 1000.0);
    workspace.seed_callgrind("2024-01-04", "c4", 1001.0);
    workspace.seed_callgrind("2024-01-05", "c5", 1001.0);
    workspace.seed_callgrind("2024-01-06", "c6", 1001.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 1,
        "a one-count deterministic step is real and must flag below the noise floor: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
    workspace.seed_alloc_tracker("2024-03-01", "a1", "allocate_vec", 200.0, 2.0);
    workspace.seed_alloc_tracker("2024-03-02", "a2", "allocate_vec", 200.0, 2.0);
    workspace.seed_alloc_tracker("2024-03-03", "a3", "allocate_vec", 200.0, 2.0);
    workspace.seed_alloc_tracker("2024-03-04", "a4", "allocate_vec", 201.0, 2.0);
    workspace.seed_alloc_tracker("2024-03-05", "a5", "allocate_vec", 201.0, 2.0);
    workspace.seed_alloc_tracker("2024-03-06", "a6", "allocate_vec", 201.0, 2.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 1,
        "a one-byte deterministic step is real and must flag: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert_eq!(parsed["findings"][0]["method"], "change_point", "{report}");
    assert_eq!(parsed["findings"][0]["direction"], "regression", "{report}");
    assert_eq!(
        parsed["findings"][0]["metric"], "allocated_bytes",
        "{report}"
    );
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
    workspace.seed_alloc_tracker("2024-04-01", "g1", "allocate_vec", 200.0, 2.0);
    workspace.seed_alloc_tracker("2024-04-02", "g2", "free_box", 8.0, 1.0);
    workspace.seed_alloc_tracker("2024-04-03", "g3", "allocate_vec", 200.0, 2.0);
    workspace.seed_alloc_tracker("2024-04-04", "g4", "free_box", 8.0, 1.0);
    workspace.seed_alloc_tracker("2024-04-05", "g5", "allocate_vec", 260.0, 2.0);
    workspace.seed_alloc_tracker("2024-04-06", "g6", "allocate_vec", 260.0, 2.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    // Only `allocate_vec`'s allocated-bytes step is a finding. A gap read as a drop
    // to zero would manufacture wild swings; instead the series is contiguous.
    assert_eq!(
        regressions, 1,
        "the gapped series' real step flags: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
        workspace.seed_all_the_time(date, label, "mk", "read_cell", value);
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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the upward drift is a regression: {report}");
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert_eq!(parsed["findings"][0]["method"], "drift", "{report}");
    assert_eq!(parsed["findings"][0]["direction"], "regression", "{report}");
    assert_eq!(
        parsed["findings"][0]["metric"], "processor_time",
        "{report}"
    );
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
        workspace.seed_all_the_time(date, label, "mk", "read_cell", value);
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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "jitter must not read as a regression: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert!(
        parsed["findings"]
            .as_array()
            .expect("a findings array")
            .is_empty(),
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
        workspace.seed_all_the_time_with_interval(date, label, "mk", "read_cell", value, 2.0);
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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 1,
        "a sustained step with disjoint intervals is a regression: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert_eq!(parsed["findings"][0]["method"], "change_point", "{report}");
    assert_eq!(parsed["findings"][0]["direction"], "regression", "{report}");
    assert_eq!(
        parsed["findings"][0]["metric"], "processor_time",
        "{report}"
    );
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
        workspace.seed_all_the_time_with_interval(date, label, "mk", "read_cell", value, 60.0);
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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "an overlapping-interval step is suppressed by the CI gate: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert!(
        parsed["findings"]
            .as_array()
            .expect("a findings array")
            .is_empty(),
        "wide overlapping intervals suppress the step entirely: {report}"
    );
}

// ===========================================================================
// Real-adapter `analyze` scenarios exercising git topology.
//
// These build a real git repository in the workspace (`Workspace::repo`) and seed
// history keyed by the commits' actual SHAs, so the real `SystemGitHistory`
// resolves the same timeline `analyze` reconstructs. They cover the repo-required
// guard, the official-vs-feature dirty admission split, topology ordering, and the
// discriminant facets — the behaviors the in-lib fake-driven unit tests prove in
// isolation, here verified end to end against `git`.
// ===========================================================================

/// Without a repository to resolve topology from, `analyze` is an error rather
/// than an empty success — a series' timeline comes from git, not storage.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_without_a_repository_errors() {
    let workspace = Workspace::new(&storage_only_config());

    let error = workspace
        .drive(&["analyze"])
        .await
        .expect_err("analysis without a repository should fail");
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
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "c2", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 100.0);
    workspace.seed_callgrind("2024-01-04", "c4", 100.0);
    // A dirty snapshot on the tip commit that would spike the series if admitted.
    workspace.seed_dirty_callgrind("2024-01-05", "c4", 200.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 0, "the dirty spike must be excluded: {report}");
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
    workspace.seed_dirty_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-02", "c2", 130.0);

    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert_eq!(
        parsed["runs"], 0,
        "every dirty-on-base snapshot is excluded: {report}"
    );
    let hint = parsed["hint"]
        .as_str()
        .unwrap_or_else(|| panic!("the empty result must carry a diagnostic hint: {report}"));
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
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "c2", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 100.0);
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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 1,
        "the dirty tip regression is admitted: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert_eq!(
        parsed["runs"], 5,
        "the dirty tip snapshots join the three clean runs: {report}"
    );
    let warning = parsed["warning"]
        .as_str()
        .unwrap_or_else(|| panic!("admitting a dirty base-branch run must warn: {report}"));
    assert!(
        warning.contains("Switch to a new branch"),
        "the warning should advise switching to a branch: {warning}"
    );

    // `--no-dirty` skips the probe and the exception, so the dirty tip is dropped.
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json", "--no-dirty"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "c2", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 100.0);
    workspace.seed_callgrind("2024-01-04", "c4", 130.0);
    workspace.seed_callgrind("2024-01-05", "c5", 130.0);
    workspace.seed_callgrind("2024-01-06", "c6", 130.0);
    // Dirty the working tree, even though no dirty run was ever stored.
    workspace.make_dirty("uncommitted.txt");

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "c2", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 100.0);
    // Branch off master and add a clean point plus two dirty regression snapshots
    // on it (two raised points satisfy the change-point persistence requirement).
    workspace.checkout_new_branch("feature");
    workspace.seed_callgrind("2024-01-04", "f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-05", "f1", 200.0);
    workspace.seed_dirty_callgrind("2024-01-06", "f1", 200.0);

    // By default (feature is HEAD; base is the detected master) the dirty snapshot
    // on the target side is admitted, so the spike flags.
    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&["analyze"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the dirty feature snapshot should flag");

    // `--no-dirty` drops it, leaving only the flat clean history.
    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--no-dirty"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "with --no-dirty only the flat clean series remains: {report}"
    );
}

/// The series timeline follows git topology, not the runs' effective timestamps:
/// seeding the regressing commit last in topology but earliest in time still flags
/// it, proving ingest/effective time does not reorder the history.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_orders_by_topology_not_effective_time() {
    let workspace = Workspace::repo(&storage_only_config());
    // Topology c1 -> .. -> c6 with a sustained step at c4, but effective times
    // strictly descend, so an effective-time ordering would reverse the step into
    // a non-regression (a drop). A flagged regression proves topology order won.
    workspace.seed_callgrind("2024-04-06", "c1", 100.0);
    workspace.seed_callgrind("2024-04-05", "c2", 100.0);
    workspace.seed_callgrind("2024-04-04", "c3", 100.0);
    workspace.seed_callgrind("2024-04-03", "c4", 130.0);
    workspace.seed_callgrind("2024-04-02", "c5", 130.0);
    workspace.seed_callgrind("2024-04-01", "c6", 130.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 1,
        "topology order must place the 130 step last and flag it: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "c2", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 100.0);
    // The feature branch first improves (80) then regresses hard (130): the latest
    // state is what matters, so this must be reported as a regression vs the base.
    workspace.checkout_new_branch("feature");
    workspace.seed_callgrind("2024-01-04", "f1", 80.0);
    workspace.seed_callgrind("2024-01-05", "f2", 80.0);
    workspace.seed_callgrind("2024-01-06", "f3", 80.0);
    workspace.seed_callgrind("2024-01-07", "f4", 130.0);
    workspace.seed_callgrind("2024-01-08", "f5", 130.0);
    workspace.seed_callgrind("2024-01-09", "f6", 130.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 1,
        "the latest (worse) regime must be reported, not the intermediate improvement: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "c2", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.seed_callgrind("2024-01-04", "f1", 100.0);
    workspace.seed_callgrind("2024-01-05", "f2", 100.0);
    workspace.seed_callgrind("2024-01-06", "f3", 100.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 0, "a flat branch must not flag: {report}");
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "c2", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 100.0);
    workspace.seed_callgrind("2024-01-04", "c4", 70.0);
    workspace.seed_callgrind("2024-01-05", "c5", 70.0);
    workspace.seed_callgrind("2024-01-06", "c6", 70.0);

    // By default history mode stays silent about the improvement.
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
    regressing.seed_callgrind("2024-01-01", "c1", 100.0);
    regressing.seed_callgrind("2024-01-02", "c2", 100.0);
    regressing.seed_callgrind("2024-01-03", "c3", 100.0);
    regressing.seed_callgrind("2024-01-04", "c4", 100.0);
    regressing.seed_callgrind("2024-01-05", "c5", 130.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = regressing
        .drive(&["analyze", "--mode", "tip", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the tip step up must flag: {report}");
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert_eq!(parsed["mode"], "tip", "the override must win: {report}");

    let improving = Workspace::repo(&storage_only_config());
    improving.seed_callgrind("2024-01-01", "c1", 100.0);
    improving.seed_callgrind("2024-01-02", "c2", 100.0);
    improving.seed_callgrind("2024-01-03", "c3", 100.0);
    improving.seed_callgrind("2024-01-04", "c4", 100.0);
    improving.seed_callgrind("2024-01-05", "c5", 70.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = improving
        .drive(&["analyze", "--mode", "tip", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "tip mode reports regressions only, so a drop stays silent: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
        .expect_err("an unknown mode should be rejected");
    assert!(
        error.to_string().contains("unknown analysis mode"),
        "the error should name the problem: {error}"
    );
}

/// `list discriminants` enumerates exactly the comparable sets present in
/// storage, deriving the os/architecture facets from each set's target triple.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn list_discriminants_lists_present_sets() {
    let workspace = Workspace::repo(&storage_only_config());
    // One commit, but two comparable sets: a Linux and a Windows callgrind pool.
    workspace.seed_callgrind_in(
        "x86_64-unknown-linux-gnu",
        "synthetic",
        "2024-01-01",
        "c1",
        100.0,
    );
    workspace.seed_callgrind_in(
        "x86_64-pc-windows-msvc",
        "synthetic",
        "2024-01-01",
        "c1",
        100.0,
    );

    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "discriminants", "--format", "json"])
        .await
        .expect("listing succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    let sets = parsed.as_array().expect("an array of sets");
    assert_eq!(sets.len(), 2, "exactly the two present sets: {message}");
    let oses: Vec<&str> = sets
        .iter()
        .map(|set| set["os"].as_str().expect("an os facet"))
        .collect();
    assert!(oses.contains(&"linux"), "{message}");
    assert!(oses.contains(&"windows"), "{message}");
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
        .expect("listing succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 6, "{message}");
    assert_eq!(parsed["totals"]["series"], 1, "{message}");
    assert_eq!(parsed["totals"]["discriminant_sets"], 1, "{message}");

    let sets = parsed["sets"].as_array().expect("an array of sets");
    assert_eq!(sets.len(), 1, "{message}");
    assert_eq!(sets[0]["engine"], "callgrind", "{message}");
    assert_eq!(sets[0]["runs"], 6, "{message}");

    let commits = sets[0]["commits"].as_array().expect("an array of commits");
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
    workspace.seed_callgrind_in(
        "x86_64-unknown-linux-gnu",
        "synthetic",
        "2024-01-01",
        "c1",
        100.0,
    );
    workspace.seed_callgrind_in(
        "x86_64-pc-windows-msvc",
        "synthetic",
        "2024-01-01",
        "c1",
        50.0,
    );

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
        .expect("listing succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["discriminant_sets"], 1, "{message}");
    assert_eq!(parsed["sets"][0]["os"], "linux", "{message}");
}

/// `list` admits and excludes dirty snapshots exactly as `analyze` does: on a
/// feature branch a dirty run on the target side is previewed by default and
/// dropped under `--no-dirty`.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn list_admits_and_excludes_dirty_like_analyze() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.seed_callgrind("2024-01-02", "f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-03", "f1", 200.0);

    // By default the dirty snapshot on the target side is included: f1 hosts both a
    // clean and a dirty run, for three runs across two commits.
    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "runs", "--format", "json"])
        .await
        .expect("listing succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 3, "{message}");
    let commits = parsed["sets"][0]["commits"].as_array().expect("commits");
    let f1 = commits.last().expect("the feature commit entry");
    assert_eq!(f1["runs"], 2, "{message}");
    assert_eq!(f1["clean"], 1, "{message}");
    assert_eq!(f1["dirty"], 1, "{message}");

    // `--no-dirty` drops the dirty snapshot, leaving only the two clean runs.
    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "runs", "--no-dirty", "--format", "json"])
        .await
        .expect("listing succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 2, "{message}");
}

/// Like `analyze`, `list` requires a repository to resolve the timeline from git
/// topology; without one it errors rather than reporting an empty data set.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn list_without_a_repository_errors() {
    let workspace = Workspace::new(&storage_only_config());

    let error = workspace
        .drive(&["list", "runs"])
        .await
        .expect_err("listing without a repository should fail");
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
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);

    let error = workspace
        .drive(&["list", "runs", "--all"])
        .await
        .expect_err("--all is rejected for the runs subject");
    let RunError::Analyze { message } = error else {
        panic!("expected an analyze error, got {error:?}");
    };
    assert!(message.contains("--all"), "{message}");
    assert!(message.contains("list blessings"), "{message}");
}

/// `prune --dirty` removes the dirty runs a matching `list`/`analyze` would include
/// on the target side of a feature branch, leaving the clean runs untouched. The
/// end state is verified through `list`, proving the production delete path reaches
/// the configured storage.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_dirty_removes_dirty_runs_on_a_feature_branch() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.seed_callgrind("2024-01-02", "f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-03", "f1", 200.0);

    // Before: the target-side dirty snapshot is part of the data set.
    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "runs", "--format", "json"])
        .await
        .expect("listing succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 3, "{message}");

    // `prune --dirty` removes exactly the one dirty run.
    let RunOutcome::Completed { message } = workspace
        .drive(&["prune", "--dirty", "--format", "json"])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["dry_run"], false, "{message}");
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");

    // After: only the two clean runs remain; the dirty snapshot is gone.
    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "runs", "--format", "json"])
        .await
        .expect("listing succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 2, "{message}");
    let commits = parsed["sets"][0]["commits"].as_array().expect("commits");
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
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-02", "c1", 200.0);

    // With a clean working tree, `list` excludes the base-tip dirty snapshot.
    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "runs", "--format", "json"])
        .await
        .expect("listing succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "list hides the base-tip dirty run with a clean tree: {message}"
    );

    // `prune --dirty` removes it anyway.
    let RunOutcome::Completed { message } = workspace
        .drive(&["prune", "--dirty", "--format", "json"])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");

    // A second pass finds nothing left to remove.
    let RunOutcome::Completed { message } = workspace
        .drive(&["prune", "--dirty", "--dry-run", "--format", "json"])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
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
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.seed_dirty_callgrind("2024-01-02", "f1", 200.0);

    let RunOutcome::Completed { message } = workspace
        .drive(&["prune", "--dirty", "--dry-run", "--format", "json"])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["dry_run"], true, "{message}");
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");

    // The dry run deleted nothing: a real prune still removes the same run.
    let RunOutcome::Completed { message } = workspace
        .drive(&["prune", "--dirty", "--format", "json"])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["dry_run"], false, "{message}");
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");
}

/// An engine facet scopes the prune: a callgrind-scoped `prune --dirty` leaves a
/// dirty criterion snapshot on the same commit untouched.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_dirty_scopes_by_engine() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.seed_dirty_callgrind("2024-01-02", "f1", 200.0);
    workspace.seed_dirty_criterion("2024-01-02", "f1", "m1", 20.0);

    // Prune only the callgrind set.
    let RunOutcome::Completed { message } = workspace
        .drive(&[
            "prune",
            "--dirty",
            "--engine",
            "callgrind",
            "--format",
            "json",
        ])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "only the callgrind dirty run: {message}"
    );
    assert_eq!(parsed["sets"][0]["engine"], "callgrind", "{message}");

    // The criterion dirty snapshot survives: a criterion-scoped prune still finds it
    // once the windows triple and its non-host machine key are named explicitly.
    let RunOutcome::Completed { message } = workspace
        .drive(&[
            "prune",
            "--dirty",
            "--engine",
            "criterion",
            "--target-triple",
            "x86_64-pc-windows-msvc",
            "--machine-key",
            "m1",
            "--dry-run",
            "--format",
            "json",
        ])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
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
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.seed_dirty_callgrind("2024-01-02", "f1", 200.0);
    workspace.seed_dirty_criterion("2024-01-02", "f1", "m1", 20.0);

    // The Linux triple removes only the callgrind (linux) dirty run.
    let RunOutcome::Completed { message } = workspace
        .drive(&[
            "prune",
            "--dirty",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--format",
            "json",
        ])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");
    assert_eq!(parsed["sets"][0]["os"], "linux", "{message}");
    assert_eq!(parsed["sets"][0]["engine"], "callgrind", "{message}");

    // The Windows (criterion) dirty run survives: a Windows-triple pass still finds
    // it once the machine facet is widened to its non-host machine key.
    let RunOutcome::Completed { message } = workspace
        .drive(&[
            "prune",
            "--dirty",
            "--target-triple",
            "x86_64-pc-windows-msvc",
            "--machine-key",
            "m1",
            "--dry-run",
            "--format",
            "json",
        ])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "windows dirty survived: {message}"
    );
    assert_eq!(parsed["sets"][0]["os"], "windows", "{message}");
}

/// `prune` spans every selected discriminant set in one pass: a dirty callgrind and
/// a dirty criterion run on the same target commit form two sets, and an unfiltered
/// `prune --dirty` removes both — exercising the multi-set plan and its plural
/// rendering.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_dirty_removes_runs_across_multiple_discriminant_sets() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.checkout_new_branch("feature");
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
            "--format",
            "text",
        ])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    assert!(
        message.contains("Removed 2 runs across 2 discriminant sets"),
        "{message}"
    );

    // Both sets are now empty: a second pass finds nothing to remove.
    let RunOutcome::Completed { message } = workspace
        .drive(&[
            "prune",
            "--dirty",
            "--target-triple",
            "all",
            "--machine-key",
            "all",
            "--dry-run",
            "--format",
            "json",
        ])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 0, "{message}");
}

/// `--since` removes only the dirty runs on or after the cutoff — one of the two
/// scopes that read object bodies in production (to recover each run's effective
/// time).
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_dirty_since_only_removes_runs_on_or_after_the_cutoff() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.seed_dirty_callgrind("2024-01-02", "f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-05", "f2", 200.0);

    // Only the 2024-01-05 run is on or after the cutoff.
    let RunOutcome::Completed { message } = workspace
        .drive(&[
            "prune",
            "--dirty",
            "--since",
            "2024-01-04",
            "--format",
            "json",
        ])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");

    // The on-or-after run is gone; the earlier run survives the cutoff.
    let RunOutcome::Completed { message } = workspace
        .drive(&[
            "prune",
            "--dirty",
            "--since",
            "2024-01-04",
            "--dry-run",
            "--format",
            "json",
        ])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(
        parsed["totals"]["runs"], 0,
        "the late run was removed: {message}"
    );

    let RunOutcome::Completed { message } = workspace
        .drive(&["prune", "--dirty", "--dry-run", "--format", "json"])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "the early run survived the cutoff: {message}"
    );
}

/// `--until` removes only the runs on or before the cutoff: the mirror of `--since`,
/// also reading object bodies to recover each run's effective time.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_dirty_until_only_removes_runs_on_or_before_the_cutoff() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.seed_dirty_callgrind("2024-01-02", "f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-05", "f2", 200.0);

    // Only the 2024-01-02 run is on or before the cutoff.
    let RunOutcome::Completed { message } = workspace
        .drive(&[
            "prune",
            "--dirty",
            "--until",
            "2024-01-03",
            "--format",
            "json",
        ])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");

    // The later run survives the cutoff; only the on-or-before run was removed.
    let RunOutcome::Completed { message } = workspace
        .drive(&["prune", "--dirty", "--dry-run", "--format", "json"])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "the later run survived the cutoff: {message}"
    );
}

/// The default scope deletes clean *and* dirty runs (and their blessing sidecars)
/// for the narrowed selection. A `<commit>` argument selecting one feature commit removes its
/// clean and dirty runs while leaving the base-branch run intact.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_default_removes_clean_and_dirty_for_a_commit() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.seed_callgrind("2024-01-02", "f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-03", "f1", 200.0);
    let f1 = workspace.commit("f1");

    // Narrowed to f1, the default scope removes its clean and dirty runs.
    let RunOutcome::Completed { message } = workspace
        .drive(&["prune", &f1, "--format", "json"])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 2, "clean + dirty f1: {message}");

    // Only the base-branch clean run remains.
    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "runs", "--context", "feature", "--format", "json"])
        .await
        .expect("listing succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");
}

/// Deleting clean history is guarded: a default-scope `prune` with no narrowing
/// predicate refuses to run unless `--all` is given, protecting against an
/// accidental wipe of the entire data set.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_refuses_an_unnarrowed_clean_scope_without_all() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);

    let error = workspace
        .drive(&["prune"])
        .await
        .expect_err("an un-narrowed clean prune should be refused");
    let RunError::Analyze { message } = error else {
        panic!("expected an analyze error, got {error:?}");
    };
    assert!(message.contains("--all"), "{message}");

    // Nothing was deleted: the run is still listed.
    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "runs", "--format", "json"])
        .await
        .expect("listing succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 1, "{message}");
}

/// `--all` overrides the narrowing guard and deletes the whole selected data set.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_all_overrides_the_narrowing_guard() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.seed_callgrind("2024-01-02", "f1", 100.0);

    let RunOutcome::Completed { message } = workspace
        .drive(&["prune", "--all", "--context", "feature", "--format", "json"])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(
        parsed["totals"]["runs"], 2,
        "both clean runs deleted: {message}"
    );

    // The data set is empty afterwards.
    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "runs", "--context", "feature", "--format", "json"])
        .await
        .expect("listing succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 0, "{message}");
}

/// `--clean` deletes clean runs (and their blessings) while leaving dirty snapshots
/// in place — the inverse of `--dirty`.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_clean_scope_removes_clean_and_keeps_dirty() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.checkout_new_branch("feature");
    workspace.seed_callgrind("2024-01-02", "f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-03", "f1", 200.0);
    let f1 = workspace.commit("f1");

    // `--clean` narrowed to f1 removes only its clean run.
    let RunOutcome::Completed { message } = workspace
        .drive(&["prune", &f1, "--clean", "--format", "json"])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(
        parsed["totals"]["runs"], 1,
        "only the clean f1 run: {message}"
    );

    // The dirty f1 snapshot survives: a `--dirty` pass still finds it.
    let RunOutcome::Completed { message } = workspace
        .drive(&["prune", "--dirty", "--dry-run", "--format", "json"])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
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

    workspace
        .drive(&["bless", "nm/nm::observe"])
        .await
        .expect("blessing the base-branch tip succeeds");

    // The blessing is recorded at HEAD.
    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "blessings", "--format", "json"])
        .await
        .expect("listing blessings succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(
        parsed["blessings"].as_array().expect("an array").len(),
        1,
        "{message}"
    );

    // Pruning HEAD's clean run also deletes the blessing that rode on it.
    let RunOutcome::Completed { message } = workspace
        .drive(&["prune", &head, "--format", "json"])
        .await
        .expect("prune succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["totals"]["runs"], 1, "the clean HEAD run: {message}");
    assert_eq!(parsed["totals"]["blessings"], 1, "{message}");

    // The blessing is gone.
    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "blessings", "--format", "json"])
        .await
        .expect("listing blessings succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert!(
        parsed["blessings"].as_array().expect("an array").is_empty(),
        "the blessing was removed with its clean run: {message}"
    );
}

/// Like `analyze`/`list`, `prune` requires a repository to resolve the topology;
/// without one it errors rather than removing nothing silently.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_without_a_repository_errors() {
    let workspace = Workspace::new(&storage_only_config());

    let error = workspace
        .drive(&["prune", "--dirty"])
        .await
        .expect_err("prune without a repository should fail");
    let RunError::Analyze { message } = error else {
        panic!("expected an analyze error, got {error:?}");
    };
    assert!(message.contains("requires a git repository"), "{message}");
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
        workspace.seed_callgrind_in("x86_64-unknown-linux-gnu", "synthetic", date, label, linux);
        workspace.seed_callgrind_in("x86_64-pc-windows-msvc", "synthetic", date, label, windows);
    }

    // The Linux triple sees the regression.
    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&["analyze", "--target-triple", "x86_64-unknown-linux-gnu"])
        .await
        .expect("analysis succeeds")
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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 0, "the Windows series is flat: {report}");
}

/// From a feature checkout, `--branch master` analyzes the default branch's
/// official line (clean only), independent of the currently checked-out branch.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_branch_selects_official_line_from_a_feature_checkout() {
    let workspace = Workspace::repo(&storage_only_config());
    // Master carries a clean sustained regression.
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "c2", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 100.0);
    workspace.seed_callgrind("2024-01-04", "c4", 130.0);
    workspace.seed_callgrind("2024-01-05", "c5", 130.0);
    workspace.seed_callgrind("2024-01-06", "c6", 130.0);
    // A feature branch with an unrelated dirty improvement that master must ignore.
    workspace.checkout_new_branch("feature");
    workspace.seed_dirty_callgrind("2024-01-07", "f1", 10.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--context", "master", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 1,
        "the master regression is selected, the feature snapshot ignored: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "M", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 100.0);
    workspace.seed_callgrind("2024-01-04", "c4", 130.0);
    workspace.seed_callgrind("2024-01-05", "c5", 130.0);
    workspace.seed_callgrind("2024-01-06", "c6", 130.0);
    // Side-branch points sit on the second-parent side and must never leak into the
    // official line; they carry wild values that would distort the series if read.
    workspace.seed_callgrind("2024-01-02", "sf1", 999.0);
    workspace.seed_callgrind("2024-01-02", "sf2", 999.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "f1", 100.0);
    workspace.seed_callgrind("2024-01-05", "f2", 100.0);
    // A dirty snapshot on c1. Without the merge, c1 would be base-side (clean
    // only) and this would be excluded; the off-chain merge-base makes the whole
    // line target-side, so it is admitted.
    workspace.seed_dirty_callgrind("2024-01-03", "c1", 200.0);
    // Merged-in master commits c2/c3 are off the first-parent line and excluded.
    workspace.seed_callgrind("2024-01-03", "c2", 999.0);
    workspace.seed_callgrind("2024-01-04", "c3", 999.0);

    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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
    workspace.seed_criterion("2024-02-01", "d1", "mk-flat", 20.0);
    workspace.seed_criterion("2024-02-02", "d2", "mk-flat", 20.0);
    workspace.seed_criterion("2024-02-03", "d3", "mk-flat", 20.0);
    workspace.seed_criterion("2024-02-04", "d4", "mk-flat", 20.0);

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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    let sets = parsed["sets"].as_array().expect("a per-set breakdown");
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
        workspace.seed_callgrind_in("x86_64-unknown-linux-gnu", "synthetic", date, label, x64);
        workspace.seed_callgrind_in("aarch64-unknown-linux-gnu", "synthetic", date, label, arm);
    }

    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&["analyze", "--target-triple", "x86_64-unknown-linux-gnu"])
        .await
        .expect("analysis succeeds")
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
        .expect("analysis succeeds")
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
    workspace.seed_criterion("2024-02-01", "d1", "mk-flat", 20.0);
    workspace.seed_criterion("2024-02-02", "d2", "mk-flat", 20.0);
    workspace.seed_criterion("2024-02-03", "d3", "mk-flat", 20.0);
    workspace.seed_criterion("2024-02-04", "d4", "mk-flat", 20.0);

    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&[
            "analyze",
            "--target-triple",
            "x86_64-pc-windows-msvc",
            "--machine-key",
            "mk-rising",
        ])
        .await
        .expect("analysis succeeds")
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
        .expect("analysis succeeds")
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
    workspace.seed_criterion("2024-02-01", "c1", "mk", 20.0);
    workspace.seed_criterion("2024-02-02", "c2", "mk", 20.0);
    workspace.seed_criterion("2024-02-03", "c3", "mk", 20.0);
    // Branch off master, add a clean point, then several dirty snapshots that step
    // up — enough points on each side for the rank-sum gate to clear the noise.
    workspace.checkout_new_branch("feature");
    workspace.seed_criterion("2024-02-04", "f1", "mk", 20.0);
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
        .expect("analysis succeeds")
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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "with --no-dirty only the flat clean Criterion series remains: {report}"
    );
}

// ===========================================================================
// Real-adapter `run` end-to-end scenarios.
//
// These drive `run`/`backfill` against the mock engine within a temporary
// workspace passed explicitly through the API, so they run in parallel without
// mutating the process environment. They are miri-ignored because they spawn
// processes and touch the filesystem.
// ===========================================================================

/// End-to-end happy path: a successful no-op engine command, one harvested
/// summary, and a stored set with the expected object key and context.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_callgrind_end_to_end_stores_results() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&["--summary", "grp=single"]);

    let outcome = workspace
        .drive(&["run", "--timestamp", "2020-01-01T00:00:00Z"])
        .await
        .expect("run should succeed");
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 1"), "{message}");

    let (key, set) = workspace.single_object();

    // Synthetic partition (Callgrind is hardware-independent) under the resolved
    // triple. `run` auto-detects the triple (Callgrind pins the OS to Linux), so
    // derive it from the stored context to keep the assertion host-portable. The
    // temp workspace is outside any git repository, so the commit resolves to the
    // `unknown` fallback and the clean tree yields `clean.json`.
    let effective: Timestamp = "2020-01-01T00:00:00Z".parse().unwrap();
    let triple = &set.context.toolchain.target_triple;
    assert!(triple.ends_with("-unknown-linux-gnu"), "{triple}");
    assert_eq!(
        key,
        format!("v2/testproj/callgrind/{triple}/synthetic/unknown/clean.json")
    );

    assert_eq!(set.schema_version, SCHEMA_VERSION);
    assert_eq!(set.context.tool_version, TOOL_VERSION);
    assert_eq!(set.context.timestamps.effective, effective);

    assert_eq!(set.results.len(), 1);
    let record = &set.results[0];
    assert!(record.id.value.is_none());
    assert_eq!(ir_of(record), 36.0);
    let ir = record
        .metrics
        .iter()
        .find(|metric| metric.name == "Ir")
        .expect("instruction count should be present");
    assert_eq!(ir.kind, MetricKind::InstructionCount);
}

/// Regression: a benchmark binary launched by `cargo bench --package X` runs with
/// its working directory set to the package directory, so the harvest must inject
/// an *absolute* `CARGO_TARGET_DIR`. A relative one would be resolved by an engine
/// that honors it (such as Criterion) against that package directory, depositing
/// output where the workspace-rooted harvest never looks — storing nothing, the
/// exact symptom this guards against. Driving without a target-root override
/// exercises the real `resolve_target_root`, and the mock changes into a package
/// subdirectory before writing, standing in for cargo's per-package cwd.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_harvests_output_when_the_engine_runs_in_a_package_directory() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&[
        "--chdir",
        "subpkg",
        "--summary",
        "grp=single",
    ]);
    std::fs::create_dir_all(workspace.root().join("subpkg")).unwrap();

    let outcome = workspace
        .drive_resolving_target_root(&["run"])
        .await
        .expect("run should succeed");

    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 1"), "{message}");
    assert_eq!(
        workspace.stored_objects().len(),
        1,
        "the summary written from the package directory should be harvested and stored"
    );
}

/// Two summaries under the target tree yield one stored set with one record each.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_stores_a_record_per_summary() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&[
        "--summary",
        "a=single",
        "--summary",
        "b=parametrized",
    ]);

    let outcome = workspace.drive(&["run"]).await.expect("run should succeed");
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("covering 2"), "{message}");

    let (_, set) = workspace.single_object();
    assert_eq!(set.results.len(), 2);

    let parametrized = set
        .results
        .iter()
        .find(|record| record.id.value.as_deref() == Some("two_instants"))
        .expect("parametrized record should be present");
    assert_eq!(ir_of(parametrized), 87.0);

    let unparametrized = set
        .results
        .iter()
        .find(|record| record.id.value.is_none())
        .expect("unparametrized record should be present");
    assert_eq!(ir_of(unparametrized), 36.0);
}

/// Two bench targets that share a `module_path` but live in different packages
/// stay distinct: their records differ only in `package`, so they never collapse
/// into one series. Without the package component they would silently merge.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_distinguishes_same_module_path_across_packages() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&[
        "--summary",
        "a=single",
        "--summary",
        "b=single-alt-pkg",
    ]);

    let outcome = workspace.drive(&["run"]).await.expect("run should succeed");
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("covering 2"), "{message}");

    let (_, set) = workspace.single_object();
    assert_eq!(set.results.len(), 2);

    // Both records share group, case, and value; only the package differs.
    let groups: Vec<&str> = set.results.iter().map(|r| r.id.group.as_str()).collect();
    assert_eq!(
        groups[0], groups[1],
        "the colliding module_path is identical"
    );

    let mut packages: Vec<Option<&str>> = set
        .results
        .iter()
        .map(|r| r.id.package.as_deref())
        .collect();
    packages.sort_unstable();
    assert_eq!(packages, vec![Some("fast_time"), Some("other_pkg")]);

    // The identities differ, so analyze would build two series rather than one.
    assert_ne!(set.results[0].id, set.results[1].id);
}

/// Two bench harnesses that compile to the *same* binary name in different
/// packages (`foo/benches/a.rs` and `bar/benches/a.rs`) write their summaries
/// under the same top-level `gungraun/shared/` directory but in distinct nested
/// ones. Both must be harvested — the recursive walk finds each — and kept
/// distinct by package, so the on-disk name collision never collapses or drops a
/// result.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_harvests_colliding_bench_binary_names_in_distinct_packages() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&[
        "--summary",
        "shared/foo=single",
        "--summary",
        "shared/bar=single-alt-pkg",
    ]);

    let outcome = workspace.drive(&["run"]).await.expect("run should succeed");
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("covering 2"), "{message}");

    let (_, set) = workspace.single_object();
    assert_eq!(
        set.results.len(),
        2,
        "both colliding-name summaries are harvested from their nested directories"
    );

    // Same group/case, distinct package — exactly the cross-package collision shape.
    let mut packages: Vec<Option<&str>> = set
        .results
        .iter()
        .map(|r| r.id.package.as_deref())
        .collect();
    packages.sort_unstable();
    assert_eq!(packages, vec![Some("fast_time"), Some("other_pkg")]);
    assert_ne!(set.results[0].id, set.results[1].id);
}

/// `--no-store` still harvests the output but writes nothing to storage.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_no_store_harvests_without_storing() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&["--summary", "grp=single"]);

    let outcome = workspace
        .drive(&["run", "--no-store"])
        .await
        .expect("run should succeed");
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Harvested 1"), "{message}");
    assert!(message.contains("nothing stored"), "{message}");

    assert!(
        workspace.stored_objects().is_empty(),
        "nothing should be stored"
    );
}

/// A non-zero engine exit aborts the run with the engine name and exit code, and
/// nothing is stored.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_propagates_nonzero_engine_exit() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&["--exit-code", "7"]);

    let error = workspace
        .drive(&["run"])
        .await
        .expect_err("a non-zero engine exit should fail the run");
    let RunError::Engine { engine, code } = error else {
        panic!("expected an engine error, got {error:?}");
    };
    assert_eq!(engine, "cargo bench");
    assert_eq!(code, Some(7));

    assert!(
        workspace.stored_objects().is_empty(),
        "a failed run stores nothing"
    );
}

/// A Criterion run stores a wall-time result set in a machine-fingerprinted
/// partition. With no engine configuration, the criterion output the mock writes
/// is auto-detected and harvested.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_criterion_stores_results() {
    let workspace =
        Workspace::new(&storage_only_config()).with_bench(&["--criterion", "grp|capture|now=26.9"]);

    let outcome = workspace.drive(&["run"]).await.expect("run should succeed");
    assert!(matches!(outcome, RunOutcome::Completed { .. }));

    let (key, set) = workspace.single_object();
    // Criterion partitions by the host triple and a machine fingerprint (never the
    // `synthetic` segment Callgrind uses).
    assert!(key.contains("/criterion/"), "{key}");
    assert!(!key.contains("/synthetic/"), "{key}");
    assert_eq!(set.results.len(), 1);
    let metric = &set.results[0].metrics[0];
    assert_eq!(metric.kind, MetricKind::WallTime);
    assert_eq!(metric.value, 26.9);
    assert_eq!(metric.unit.as_deref(), Some("ns"));
    assert!(metric.std_dev.is_some(), "dispersion should be recorded");
}

/// A single benchmark run harvests every engine that produced output: the mock
/// writes a Callgrind summary, a Criterion case, an `alloc_tracker` operation and
/// an `all_the_time` operation, so the run stores one result set per engine in its
/// own partition.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_harvests_every_engine_that_produced_output() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&[
        "--summary",
        "grp=single",
        "--criterion",
        "grp|capture|now=12.5",
        "--alloc-tracker",
        "allocate_vec=200/2",
        "--all-the-time",
        "read_cell=20",
    ]);

    let outcome = workspace.drive(&["run"]).await.expect("run should succeed");
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 4"), "{message}");

    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 4, "{objects:?}");
    // Deterministic engines (Callgrind instruction counts, allocation statistics)
    // land in the `synthetic` partition; hardware-dependent engines (Criterion
    // wall time, `all_the_time` processor time) carry a machine fingerprint.
    assert!(
        objects
            .iter()
            .any(|(key, _)| key.contains("/callgrind/") && key.contains("/synthetic/")),
        "{objects:?}"
    );
    assert!(
        objects
            .iter()
            .any(|(key, _)| key.contains("/criterion/") && !key.contains("/synthetic/")),
        "{objects:?}"
    );
    assert!(
        objects
            .iter()
            .any(|(key, _)| key.contains("/alloc_tracker/") && key.contains("/synthetic/")),
        "{objects:?}"
    );
    assert!(
        objects
            .iter()
            .any(|(key, _)| key.contains("/all_the_time/") && !key.contains("/synthetic/")),
        "{objects:?}"
    );
}

/// An `alloc_tracker` run stores allocation statistics in the `synthetic`
/// partition (allocation counts are a deterministic property of the code, not the
/// hardware), carrying both the byte and the count metric.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_alloc_tracker_stores_results() {
    let workspace = Workspace::new(&storage_only_config())
        .with_bench(&["--alloc-tracker", "allocate_vec=200/2"]);

    let outcome = workspace.drive(&["run"]).await.expect("run should succeed");
    assert!(matches!(outcome, RunOutcome::Completed { .. }));

    let (key, set) = workspace.single_object();
    assert!(key.contains("/alloc_tracker/"), "{key}");
    assert!(key.contains("/synthetic/"), "{key}");
    assert_eq!(set.results.len(), 1);
    let record = &set.results[0];
    assert_eq!(record.id.group, "allocate_vec");
    assert_eq!(record.metrics.len(), 2, "{:?}", record.metrics);

    let bytes = metric_named(record, "allocated_bytes");
    assert_eq!(bytes.kind, MetricKind::AllocationBytes);
    assert_eq!(bytes.value, 200.0);
    assert_eq!(bytes.unit.as_deref(), Some("bytes"));

    let count = metric_named(record, "allocations");
    assert_eq!(count.kind, MetricKind::AllocationCount);
    assert_eq!(count.value, 2.0);
    assert_eq!(count.unit.as_deref(), Some("count"));
}

/// An `all_the_time` run stores processor time in a machine-fingerprinted
/// partition (processor time is hardware-dependent), and `--machine-key` overrides
/// the fingerprint.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_all_the_time_is_partitioned_by_machine_key() {
    let workspace =
        Workspace::new(&storage_only_config()).with_bench(&["--all-the-time", "read_cell=20"]);

    workspace
        .drive(&["run", "--machine-key", "ci-pool-b"])
        .await
        .expect("run should succeed");

    let (key, set) = workspace.single_object();
    let triple = &set.context.toolchain.target_triple;
    assert!(
        key.contains(&format!("/all_the_time/{triple}/ci-pool-b/")),
        "{key}"
    );
    assert!(!key.contains("/synthetic/"), "{key}");
    assert_eq!(set.results.len(), 1);
    let record = &set.results[0];
    assert_eq!(record.id.group, "read_cell");
    let processor_time = metric_named(record, "processor_time");
    assert_eq!(processor_time.kind, MetricKind::ProcessorTime);
    assert_eq!(processor_time.value, 20.0);
    assert_eq!(processor_time.unit.as_deref(), Some("ns"));
}

/// An `all_the_time` run whose emitted output carries a bootstrap confidence
/// interval stores that dispersion on the metric, so the noise detector can
/// later apply its interval-overlap gate to processor time. This proves the
/// dispersion fields flow from the engine's JSON through the harvest and adapter
/// into the stored result set, end to end through the real adapter.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_all_the_time_records_dispersion() {
    let workspace = Workspace::new(&storage_only_config())
        .with_bench(&["--all-the-time", "read_cell=20@19:21"]);

    workspace.drive(&["run"]).await.expect("run should succeed");

    let (_key, set) = workspace.single_object();
    let processor_time = metric_named(&set.results[0], "processor_time");
    assert_eq!(processor_time.value, 20.0);
    assert_eq!(processor_time.interval_low, Some(19.0));
    assert_eq!(processor_time.interval_high, Some(21.0));
    assert_eq!(processor_time.std_dev, Some(1.0));
}

/// `--machine-key` overrides the machine fingerprint in a Criterion partition.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_criterion_honors_machine_key_override() {
    let workspace =
        Workspace::new(&storage_only_config()).with_bench(&["--criterion", "grp|capture|now=9"]);

    // `run` auto-detects the triple; this test asserts the machine-key override
    // segment, so derive the triple from the stored context for a portable key.
    workspace
        .drive(&["run", "--machine-key", "ci-pool-a"])
        .await
        .expect("run should succeed");

    let (key, set) = workspace.single_object();
    let triple = &set.context.toolchain.target_triple;
    assert!(
        key.contains(&format!("/criterion/{triple}/ci-pool-a/")),
        "{key}"
    );
}

/// A Criterion run collects every harvested case into one result set, keeping
/// distinct group/function/value identities as separate records.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_criterion_collects_distinct_cases_as_records() {
    // Same function name in two different groups, plus a parametrized case: all
    // three identities are distinct and must survive as separate records.
    let workspace = Workspace::new(&storage_only_config()).with_bench(&[
        "--criterion",
        "timestamp/capture|std_instant|now=27",
        "--criterion",
        "timestamp/elapsed|std_instant|now=41",
        "--criterion",
        "timestamp/capture|fast_clock|=13",
    ]);

    workspace.drive(&["run"]).await.expect("run should succeed");

    let (_, set) = workspace.single_object();
    assert_eq!(set.results.len(), 3, "{:?}", set.results);
    let mut ids: Vec<(String, Option<String>)> = set
        .results
        .iter()
        .map(|record| (record.id.group.clone(), record.id.case.clone()))
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            (
                "timestamp/capture".to_owned(),
                Some("fast_clock".to_owned())
            ),
            (
                "timestamp/capture".to_owned(),
                Some("std_instant".to_owned())
            ),
            (
                "timestamp/elapsed".to_owned(),
                Some("std_instant".to_owned())
            ),
        ]
    );
    // Every record carries no package (Criterion output is package-agnostic).
    assert!(set.results.iter().all(|record| record.id.package.is_none()));
}

/// A project id containing characters that require sanitizing (a space and a `/`)
/// round-trips: `run` stores under the sanitized partition and `analyze` finds the
/// very same history. Both sides must derive the identical storage segment, so this
/// guards against writer/reader sanitization drift through the real pipeline.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_then_analyze_round_trips_a_sanitizing_project_id() {
    let workspace = Workspace::clean_repo(&storage_only_config_with_id("my proj/sub"))
        .with_bench(&["--summary", "grp=single"]);

    workspace
        .drive(&["run", "--timestamp", "2024-03-01T00:00:00Z"])
        .await
        .expect("run should succeed");

    // The writer sanitizes `my proj/sub` to `my_proj_sub` for the partition.
    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 1, "{objects:?}");
    assert!(
        objects
            .iter()
            .all(|(key, _)| key.starts_with("v2/my_proj_sub/callgrind/")),
        "{objects:?}"
    );

    // The reader must sanitize the same way; otherwise it lists an empty history.
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert_eq!(
        parsed["runs"], 1,
        "analyze must find the run the sanitized partition stored: {report}"
    );
    // The single Callgrind record carries several metrics, each its own series; the
    // exact count is incidental, but the history must be non-empty.
    assert!(
        parsed["series"].as_u64().expect("series count") >= 1,
        "{report}"
    );
}

/// Unusual characters in a benchmark identity (spaces, a dot, and non-ASCII
/// letters) belong to the object's JSON body, never to the storage partition key.
/// They survive `run` -> store -> `analyze` verbatim while the key stays sanitized
/// and the reader keeps both runs in one series.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_then_analyze_preserves_unusual_identity_characters() {
    let workspace = Workspace::clean_repo(&storage_only_config())
        .with_bench(&["--criterion", "time.capture|mide tiempo|tamaño 4=18.5"]);

    workspace
        .drive(&[
            "run",
            "--machine-key",
            "pool",
            "--timestamp",
            "2024-03-01T00:00:00Z",
        ])
        .await
        .expect("run should succeed");

    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 1, "{objects:?}");
    let (key, set) = &objects[0];
    // The partition key is identity-free and fully sanitized: none of the
    // identity's spaces or non-ASCII letters leak into it. `run` auto-detects the
    // triple, so derive it from the stored context for a portable prefix.
    let triple = &set.context.toolchain.target_triple;
    assert!(
        key.starts_with(&format!("v2/testproj/criterion/{triple}/pool/")),
        "{key}"
    );
    assert!(
        !key.contains(' ') && !key.contains("tamaño") && !key.contains("mide"),
        "{key}"
    );

    // The identity survives verbatim in the stored result set.
    assert_eq!(set.results.len(), 1, "{:?}", set.results);
    let id = &set.results[0].id;
    assert_eq!(id.group, "time.capture");
    assert_eq!(id.case.as_deref(), Some("mide tiempo"));
    assert_eq!(id.value.as_deref(), Some("tamaño 4"));

    // The reader reconstructs the series, proving the unusual identity is a stable
    // series key end to end.
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--machine-key", "pool", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert_eq!(parsed["runs"], 1, "{report}");
    assert_eq!(parsed["series"], 1, "{report}");
}

/// `--config` loads the configuration from a non-default path.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_uses_explicit_config_path() {
    // Only the custom path holds a configuration; the default discovery path is
    // absent, so a successful run proves `--config` was honored.
    let workspace = Workspace::with_config_at("config/bench.toml", &storage_only_config())
        .with_bench(&["--summary", "grp=single"]);

    let outcome = workspace
        .drive(&["run", "--config", "config/bench.toml"])
        .await
        .expect("run should succeed");
    assert!(matches!(outcome, RunOutcome::Completed { .. }));
    assert_eq!(workspace.stored_objects().len(), 1);
}

/// A clean re-run of the same commit collides with the deterministic clean key,
/// so the second run is refused unless an overwrite is requested.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn re_running_the_same_commit_is_refused_as_a_duplicate() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&["--summary", "grp=single"]);

    workspace
        .drive(&["run"])
        .await
        .expect("first run should succeed");

    let error = workspace
        .drive(&["run"])
        .await
        .expect_err("a second clean run of the same commit must be refused");
    let RunError::Duplicate { key } = error else {
        panic!("expected a duplicate error, got {error:?}");
    };
    assert!(key.ends_with("/clean.json"), "{key}");

    // The refused run left the single stored object in place.
    assert_eq!(workspace.stored_objects().len(), 1);
}

/// `--overwrite` replaces an already-stored clean result rather than refusing it.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn overwrite_replaces_the_stored_result() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&["--summary", "grp=single"]);

    workspace
        .drive(&["run"])
        .await
        .expect("first run should succeed");

    workspace
        .drive(&["run", "--overwrite"])
        .await
        .expect("an overwrite run should replace the existing object");

    let objects = workspace.stored_objects();
    assert_eq!(
        objects.len(),
        1,
        "overwrite must not create a second object"
    );
    assert!(objects[0].0.ends_with("/clean.json"), "{:?}", objects[0].0);
}

// ===========================================================================
// Real-adapter `backfill` scenarios.
//
// These drive `backfill` against a real git repository (a temporary checkout
// with empty commits) and the real worktree, process, harvest, and storage
// adapters: each commit of the range is checked out in a dedicated worktree and
// the mock engine writes one summary there. Each test operates on its own
// temporary repository passed explicitly through the API, so they run in
// parallel; they are `#[cfg_attr(miri, ignore)]` (real git, processes, filesystem).
// ===========================================================================

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

    let RunOutcome::Completed { message } = workspace
        .drive(&["backfill", &c1, &c3])
        .await
        .expect("backfill should succeed")
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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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

    let RunOutcome::Completed { message } = workspace
        .drive(&["backfill", &c1, &c3])
        .await
        .expect("backfill should succeed")
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

    workspace
        .drive(&["backfill", &c1, &c2])
        .await
        .expect("the first backfill should store both commits");
    assert_eq!(workspace.stored_objects().len(), 2);

    let RunOutcome::Completed { message } = workspace
        .drive(&["backfill", &c1, &c2])
        .await
        .expect("a re-run should succeed by skipping")
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

    workspace
        .drive(&["backfill", &c1, &c2])
        .await
        .expect("the first backfill should store both commits");
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
        .expect("recorded commits must be skipped before the engine runs")
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

    workspace
        .drive(&["backfill", &c1, &c2])
        .await
        .expect("the first backfill should store both commits");

    let RunOutcome::Completed { message } = workspace
        .drive(&["backfill", &c1, &c2, "--overwrite"])
        .await
        .expect("an overwrite backfill should replace in place")
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

    let error = workspace
        .drive(&["backfill", &c1, &c3])
        .await
        .expect_err("a failing commit must stop the backfill");
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
        .expect("--ignore-errors should complete past the failure");
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

    let outcome = workspace
        .drive(&["backfill", &c1, &c1])
        .await
        .expect("a dirty primary tree must not block backfill");
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

    let error = workspace
        .drive(&["backfill", &c2, &c1])
        .await
        .expect_err("a reversed range must be rejected");
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
    let early_exit = Cli::from_args(&["cargo-bench-history"], &["backfill", "--help"])
        .expect_err("help is reported as an early exit");
    assert!(early_exit.output.contains("FROM"), "{}", early_exit.output);
    assert!(early_exit.output.contains("TO"), "{}", early_exit.output);
}

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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the rising history should flag once");

    // Accept the current level at the base-branch tip.
    let RunOutcome::Completed { message } = workspace
        .drive(&["bless", "nm/nm::observe"])
        .await
        .expect("blessing the base-branch tip succeeds")
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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "a blessed regression must not be re-flagged: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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

    let RunOutcome::Completed { message } = workspace
        .drive(&["bless", "nm/nm::observe"])
        .await
        .expect("a dirty base-branch tree warns rather than erroring")
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
        .expect("analysis succeeds")
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

    workspace
        .drive(&["bless", "nm/nm::observe", "--reason", "accepted tradeoff"])
        .await
        .expect("blessing succeeds");

    let RunOutcome::Completed { message } = workspace
        .drive(&["list", "blessings", "--format", "json"])
        .await
        .expect("listing blessings succeeds")
    else {
        panic!("expected a completed outcome");
    };
    let short_head = head.get(..12).expect("sha has at least twelve characters");
    let parsed: serde_json::Value = serde_json::from_str(&message).expect("valid JSON");
    assert_eq!(parsed["scope"], "head", "{message}");
    assert_eq!(parsed["commit"], short_head, "{message}");
    let blessings = parsed["blessings"].as_array().expect("an array");
    assert_eq!(blessings.len(), 1, "{message}");
    assert_eq!(blessings[0]["commit"], short_head, "{message}");
    assert_eq!(blessings[0]["reason"], "accepted tradeoff", "{message}");
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

    workspace
        .drive(&["bless", "nm/nm::observe"])
        .await
        .expect("blessing succeeds");

    // The blessing currently suppresses the regression.
    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(
        regressions, 0,
        "the blessing should suppress the regression"
    );

    // Revoking it brings the finding back.
    let RunOutcome::Completed { message } = workspace
        .drive(&["unbless"])
        .await
        .expect("unblessing succeeds")
    else {
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
        .expect("analysis succeeds")
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
        .expect_err("a feature-branch blessing must be rejected");
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
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert_eq!(parsed["mode"], "history", "{report}");
    assert!(
        parsed["findings"].as_array().is_some_and(Vec::is_empty),
        "a recovered spike is suppressed by default: {report}"
    );

    // Opting in surfaces it as an inactive finding.
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json", "--include-inactive"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
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

// ===========================================================================
// Test harness and helpers.
// ===========================================================================

/// A hermetic workspace for driving `run` against the real adapters.
///
/// Writes a configuration and (optionally) fake engine output under a temporary
/// directory, then drives a command with that directory as the process current
/// directory, restoring the original directory before returning so assertions and
/// temp-dir cleanup happen outside it (required on Windows).
struct Workspace {
    dir: tempfile::TempDir,
    /// Maps a short commit label (for example `c1`) to the full SHA of the empty
    /// commit created for it, so seeded storage keys and the live git topology
    /// agree on the commit-directory segment. Interior mutability lets the
    /// `&self` seed helpers create commits lazily.
    commits: RefCell<HashMap<String, String>>,
    /// Arguments passed to the mock benchmark engine that `run`/`backfill` invoke
    /// in place of `cargo bench`. They tell the mock which fixtures to emit (which
    /// `--summary` / `--criterion` cases, or an `--exit-code`), so each engine's
    /// output tree is produced by the single benchmark command `run` invokes.
    bench: Vec<String>,
}

impl Workspace {
    /// Creates a workspace with `config` at the default `.cargo/bench_history.toml`.
    fn new(config: &str) -> Self {
        let workspace = Self {
            dir: tempfile::tempdir().expect("temp dir should be created"),
            commits: RefCell::new(HashMap::new()),
            bench: Vec::new(),
        };
        let cargo_dir = workspace.root().join(".cargo");
        std::fs::create_dir_all(&cargo_dir).unwrap();
        std::fs::write(cargo_dir.join("bench_history.toml"), config).unwrap();
        workspace
    }

    /// Creates a workspace with `config` plus an initialized git repository (on a
    /// `master` branch with one empty root commit), as `analyze` requires a
    /// repository to resolve a series' timeline from git topology.
    fn repo(config: &str) -> Self {
        let workspace = Self::new(config);
        workspace.init_repo();
        workspace
    }

    /// Alias for [`repo`](Self::repo): the standard repository already keeps its
    /// working tree clean across a `run` (the volatile `.cargo`/`store`/`target`
    /// directories are git-ignored). Retained for call sites that want to spell out
    /// that a clean tree — and therefore `history`-mode analysis — is intended.
    fn clean_repo(config: &str) -> Self {
        Self::repo(config)
    }

    /// Creates an empty workspace with no configuration file written.
    fn empty() -> Self {
        Self {
            dir: tempfile::tempdir().expect("temp dir should be created"),
            commits: RefCell::new(HashMap::new()),
            bench: Vec::new(),
        }
    }

    /// Creates a workspace with `config` at a non-default `relative` path.
    fn with_config_at(relative: &str, config: &str) -> Self {
        let workspace = Self {
            dir: tempfile::tempdir().expect("temp dir should be created"),
            commits: RefCell::new(HashMap::new()),
            bench: Vec::new(),
        };
        let path = workspace.root().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, config).unwrap();
        workspace
    }

    /// Sets the arguments the mock benchmark engine receives, describing the
    /// fixtures it should emit (consuming builder used at construction). A run
    /// invokes the mock once with these arguments and harvests every engine's
    /// output tree it wrote.
    fn with_bench(mut self, args: &[&str]) -> Self {
        self.bench = args.iter().map(|arg| (*arg).to_owned()).collect();
        self
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Runs `git -C <root> <args>`, returning its captured output. The repository
    /// directory is addressed explicitly so commit creation never depends on the
    /// process current directory.
    ///
    /// Every invocation injects the committer identity plus throughput-oriented
    /// config: `core.fsync=none`/`core.fsyncObjectFiles=false` skip the per-object
    /// disk flush (by far the largest per-commit cost on Windows, where real-time
    /// antivirus compounds it) and `gc.auto=0` keeps a background repack from firing
    /// mid-test. The identity is supplied here rather than via separate `git config`
    /// subprocesses so a fresh repository needs only `init`/`add`/`commit`, not five
    /// extra process spawns. These settings are safe for throwaway test repositories
    /// and unknown keys are ignored by older git, so they are inert where unsupported.
    fn git(&self, args: &[&str]) -> std::process::Output {
        let root = self.root().to_string_lossy().into_owned();
        let mut full: Vec<&str> = vec![
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Bench History Test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.fsync=none",
            "-c",
            "core.fsyncObjectFiles=false",
            "-c",
            "gc.auto=0",
            "-C",
            root.as_str(),
        ];
        full.extend_from_slice(args);
        let output = std::process::Command::new("git")
            .args(&full)
            .output()
            .expect("git should be available");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Initializes a `master`-branch repository with one empty root commit so `HEAD`
    /// always resolves. The volatile directories a run touches (`.cargo`, `store`,
    /// `target`) are excluded via the repository-local `.git/info/exclude` file —
    /// untracked, so the root commit can be empty and no `git add` spawn is needed —
    /// leaving the working tree clean unless a test deliberately dirties it (via
    /// [`make_dirty`]). A clean tree on the base branch is what makes `analyze` pick
    /// `history` mode.
    ///
    /// [`make_dirty`]: Self::make_dirty
    fn init_repo(&self) {
        self.git(&["init", "-b", "master"]);
        std::fs::write(
            self.root().join(".git").join("info").join("exclude"),
            "/.cargo/\n/store/\n/target/\n",
        )
        .unwrap();
        self.git(&["commit", "--allow-empty", "-m", "root"]);
    }

    /// Reads `HEAD`'s commit SHA straight from the `.git` directory, avoiding a
    /// `git rev-parse` subprocess (process startup dominates these tests on
    /// Windows). For the small, freshly-created repositories the harness builds the
    /// branch ref is always loose; a `git rev-parse` fallback covers the unexpected
    /// (packed refs, detached `HEAD`).
    fn head_sha(&self) -> String {
        let git_dir = self.root().join(".git");
        if let Ok(head) = std::fs::read_to_string(git_dir.join("HEAD")) {
            let head = head.trim();
            if let Some(reference) = head.strip_prefix("ref: ") {
                // `refs/heads/<branch>` uses `/`, which the Windows path APIs accept.
                if let Ok(sha) = std::fs::read_to_string(git_dir.join(reference)) {
                    let sha = sha.trim();
                    if sha.len() == 40 {
                        return sha.to_owned();
                    }
                }
            } else if head.len() == 40 {
                return head.to_owned();
            }
        }
        let head = self.git(&["rev-parse", "HEAD"]);
        String::from_utf8(head.stdout)
            .expect("a commit SHA is ASCII")
            .trim()
            .to_owned()
    }

    /// Lazily creates an empty commit labeled `label` on the current branch and
    /// returns its full SHA, reusing the SHA if the label was already created.
    ///
    /// Creation order is the first-parent topology order, so seeding in oldest-first
    /// order makes the storage keys line up with the live git history `analyze`
    /// reconstructs the series from.
    fn commit(&self, label: &str) -> String {
        if let Some(sha) = self.commits.borrow().get(label) {
            return sha.clone();
        }
        self.git(&["commit", "--allow-empty", "-m", label]);
        let sha = self.head_sha();
        self.commits
            .borrow_mut()
            .insert(label.to_owned(), sha.clone());
        sha
    }

    /// Creates and checks out a new branch off the current `HEAD`.
    fn checkout_new_branch(&self, name: &str) {
        self.git(&["checkout", "-b", name]);
    }

    /// Checks out an existing branch.
    fn checkout(&self, name: &str) {
        self.git(&["checkout", name]);
    }

    /// Merges `branch` into the current branch with an always-materialized merge
    /// commit (`--no-ff`), labels the resulting merge commit `label`, and returns
    /// its full SHA. The merge commit has the current branch's tip as its first
    /// parent and `branch`'s tip as its second, so it sits on the current branch's
    /// first-parent line while the merged-in commits stay off it — the topology the
    /// git-aware `analyze`/`backfill` selection must respect.
    fn merge(&self, branch: &str, label: &str) -> String {
        self.git(&["merge", "--no-ff", "--no-edit", "-m", label, branch]);
        let sha = self.head();
        self.commits
            .borrow_mut()
            .insert(label.to_owned(), sha.clone());
        sha
    }

    /// Commits a tracked file with `contents` at `relative` on the current branch
    /// and returns the new commit's full SHA. Unlike [`commit`](Self::commit), the
    /// commit changes the tree, so the file appears in any worktree checked out to
    /// it — used to stand in for a "broken" commit the mock engine reacts to.
    fn commit_with_file(&self, message: &str, relative: &str, contents: &str) -> String {
        std::fs::write(self.root().join(relative), contents).unwrap();
        self.git(&["add", relative]);
        self.git(&["commit", "-m", message]);
        self.head()
    }

    /// Removes a previously tracked file at `relative`, commits the removal on the
    /// current branch, and returns the new commit's full SHA.
    fn commit_removing_file(&self, message: &str, relative: &str) -> String {
        self.git(&["rm", relative]);
        self.git(&["commit", "-m", message]);
        self.head()
    }

    /// The full SHA of the current `HEAD`.
    fn head(&self) -> String {
        let output = self.git(&["rev-parse", "HEAD"]);
        String::from_utf8(output.stdout)
            .expect("a commit SHA is ASCII")
            .trim()
            .to_owned()
    }

    /// The name of the currently checked-out branch.
    fn current_branch(&self) -> String {
        let output = self.git(&["rev-parse", "--abbrev-ref", "HEAD"]);
        String::from_utf8(output.stdout)
            .expect("a branch name is ASCII")
            .trim()
            .to_owned()
    }

    /// Writes an untracked file at `relative`, leaving the working tree dirty (it
    /// is neither committed nor git-ignored).
    fn make_dirty(&self, relative: &str) {
        std::fs::write(self.root().join(relative), "uncommitted\n").unwrap();
    }

    /// Reads a file relative to the workspace root, if it exists.
    fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.root().join(relative)).ok()
    }

    /// Drives a command with `args` against this workspace.
    async fn drive(&self, args: &[&str]) -> Result<RunOutcome, RunError> {
        // Point the harvest at this workspace's own `target/` explicitly, so a
        // shared ambient `CARGO_TARGET_DIR` (as `cargo llvm-cov` sets during
        // coverage runs) cannot make the harvester pick up summaries written by
        // other tests sharing that directory. Passing the root through the API
        // keeps the test hermetic without mutating the process environment.
        let target_root = self.root().join("target");

        // Drive `run`/`backfill` against the mock engine instead of `cargo bench`:
        // the program plus its fixture-describing arguments form the benchmark
        // command, which the single bench invocation runs to produce engine output.
        let mut bench_command = vec![MOCK_ENGINE.to_owned()];
        bench_command.extend(self.bench.iter().cloned());

        run_with_overrides(
            &command_from(args),
            Overrides {
                workspace_dir: Some(self.root().to_path_buf()),
                target_root: Some(target_root),
                bench_command: Some(bench_command),
                now: Some(analysis_now()),
            },
        )
        .await
    }

    /// Drives a command with an explicit benchmark command, overriding the
    /// workspace's configured one for this invocation only. This lets a test make
    /// the engine behave differently across two drives — for example, succeed on a
    /// first backfill, then exit non-zero if it is invoked at all on a re-run.
    async fn drive_with_bench(
        &self,
        bench: &[&str],
        args: &[&str],
    ) -> Result<RunOutcome, RunError> {
        let target_root = self.root().join("target");
        let mut bench_command = vec![MOCK_ENGINE.to_owned()];
        bench_command.extend(bench.iter().map(|arg| (*arg).to_owned()));

        run_with_overrides(
            &command_from(args),
            Overrides {
                workspace_dir: Some(self.root().to_path_buf()),
                target_root: Some(target_root),
                bench_command: Some(bench_command),
                now: Some(analysis_now()),
            },
        )
        .await
    }
    ///
    /// [`drive`]: Self::drive
    async fn drive_resolving_target_root(&self, args: &[&str]) -> Result<RunOutcome, RunError> {
        let mut bench_command = vec![MOCK_ENGINE.to_owned()];
        bench_command.extend(self.bench.iter().cloned());

        run_with_overrides(
            &command_from(args),
            Overrides {
                workspace_dir: Some(self.root().to_path_buf()),
                target_root: None,
                bench_command: Some(bench_command),
                now: Some(analysis_now()),
            },
        )
        .await
    }

    /// All stored objects as `(object key, parsed result set)` pairs, sorted by key.
    fn stored_objects(&self) -> Vec<(String, ResultSet)> {
        let store = self.root().join("store");
        let mut files = Vec::new();
        collect_json_files(&store, &mut files);

        let mut objects: Vec<(String, ResultSet)> = files
            .into_iter()
            .map(|path| {
                let key = path
                    .strip_prefix(&store)
                    .unwrap()
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                let set = ResultSet::from_json(&std::fs::read_to_string(&path).unwrap())
                    .expect("stored object should parse");
                (key, set)
            })
            .collect();
        objects.sort_by(|left, right| left.0.cmp(&right.0));
        objects
    }

    /// Returns the single stored object, failing if there is not exactly one.
    fn single_object(&self) -> (String, ResultSet) {
        let mut objects = self.stored_objects();
        assert_eq!(
            objects.len(),
            1,
            "expected exactly one stored object, found {}",
            objects.len()
        );
        objects.pop().unwrap()
    }

    /// Writes `set` to `key` (a `/`-separated object key) under the local store,
    /// mirroring the layout `run` produces.
    fn seed(&self, key: &str, set: &ResultSet) {
        let mut path = self.root().join("store");
        for part in key.split('/') {
            path.push(part);
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, set.to_json().unwrap()).unwrap();
    }

    /// Seeds one Callgrind result set with an `Ir` value at the given `date`
    /// (`YYYY-MM-DD`, taken at UTC midnight) and commit `label`, creating the
    /// corresponding empty git commit so the storage key and live topology agree.
    fn seed_callgrind(&self, date: &str, label: &str, value: f64) {
        self.seed_callgrind_in("x86_64-unknown-linux-gnu", "synthetic", date, label, value);
    }

    /// Seeds one clean Callgrind result set into the `triple`/`machine` partition
    /// for commit `label`, so a single commit can host objects in several
    /// comparable sets (as parallel CI pools produce).
    fn seed_callgrind_in(&self, triple: &str, machine: &str, date: &str, label: &str, value: f64) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key = format!("v2/testproj/callgrind/{triple}/{machine}/{sha}/clean.json");
        self.seed(&key, &ir_result_set(effective.as_second(), &sha, value));
    }

    /// Seeds one *dirty* (uncommitted-tree) Callgrind snapshot with an `Ir` value
    /// at commit `label`, keyed by the effective second like a real dirty run.
    fn seed_dirty_callgrind(&self, date: &str, label: &str, value: f64) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key = format!(
            "v2/testproj/callgrind/x86_64-unknown-linux-gnu/synthetic/{sha}/dirty-{}.json",
            effective.as_second()
        );
        self.seed(&key, &ir_result_set(effective.as_second(), &sha, value));
    }

    /// Seeds one Callgrind result set carrying `metrics` for benchmark
    /// `nm::observe/pull` at the given `date` (`YYYY-MM-DD`, UTC midnight) and
    /// commit `label`.
    fn seed_metrics(&self, date: &str, label: &str, metrics: Vec<Metric>) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key =
            format!("v2/testproj/callgrind/x86_64-unknown-linux-gnu/synthetic/{sha}/clean.json");
        self.seed(&key, &result_set_with(effective.as_second(), &sha, metrics));
    }

    /// Seeds a flat history followed by a clear, sustained upward step — a
    /// regression with enough points on each side to satisfy the change-point
    /// detector's persistence requirement.
    fn seed_rising_callgrind_history(&self) {
        self.seed_callgrind("2024-01-01", "c1", 100.0);
        self.seed_callgrind("2024-01-02", "c2", 100.0);
        self.seed_callgrind("2024-01-03", "c3", 100.0);
        self.seed_callgrind("2024-01-04", "c4", 130.0);
        self.seed_callgrind("2024-01-05", "c5", 130.0);
        self.seed_callgrind("2024-01-06", "c6", 130.0);
    }

    /// Seeds one Criterion `wall_time` result set at the given `date` (`YYYY-MM-DD`,
    /// UTC midnight), commit `label`, and machine-key partition `machine`.
    fn seed_criterion(&self, date: &str, label: &str, machine: &str, value: f64) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key =
            format!("v2/testproj/criterion/x86_64-pc-windows-msvc/{machine}/{sha}/clean.json");
        self.seed(
            &key,
            &criterion_result_set(effective.as_second(), &sha, value),
        );
    }

    /// Seeds one *dirty* (uncommitted-tree) Criterion `wall_time` snapshot at the
    /// given `date`, commit `label`, and machine-key partition `machine`, keyed by
    /// the effective second like a real dirty run.
    fn seed_dirty_criterion(&self, date: &str, label: &str, machine: &str, value: f64) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key = format!(
            "v2/testproj/criterion/x86_64-pc-windows-msvc/{machine}/{sha}/dirty-{}.json",
            effective.as_second()
        );
        self.seed(
            &key,
            &criterion_result_set(effective.as_second(), &sha, value),
        );
    }

    /// Seeds a flat Criterion `wall_time` history then a clear, sustained upward
    /// step. Four points on each side give the rank-sum gate enough power to
    /// distinguish the step from noise.
    fn seed_rising_criterion_history(&self, machine: &str) {
        self.seed_criterion("2024-02-01", "d1", machine, 20.0);
        self.seed_criterion("2024-02-02", "d2", machine, 20.0);
        self.seed_criterion("2024-02-03", "d3", machine, 20.0);
        self.seed_criterion("2024-02-04", "d4", machine, 20.0);
        self.seed_criterion("2024-02-05", "d5", machine, 30.0);
        self.seed_criterion("2024-02-06", "d6", machine, 30.0);
        self.seed_criterion("2024-02-07", "d7", machine, 30.0);
        self.seed_criterion("2024-02-08", "d8", machine, 30.0);
    }

    /// Seeds one Callgrind result set carrying two distinct benchmark identities,
    /// `alpha::bench/wide` and `beta::bench/narrow`, each with an `Ir` metric at the
    /// given value, stamped at `date` (`YYYY-MM-DD`, UTC midnight) and commit `label`.
    fn seed_two_benchmarks(&self, date: &str, label: &str, alpha: f64, beta: f64) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key =
            format!("v2/testproj/callgrind/x86_64-unknown-linux-gnu/synthetic/{sha}/clean.json");
        self.seed(
            &key,
            &two_benchmark_result_set(effective.as_second(), &sha, alpha, beta),
        );
    }

    /// Seeds one clean `alloc_tracker` result set for `operation` at the given
    /// `date` and commit `label`, recording `bytes` mean bytes and `allocs` mean
    /// allocations per iteration. Allocation counts are deterministic, so the
    /// partition is `synthetic` (no machine key).
    fn seed_alloc_tracker(
        &self,
        date: &str,
        label: &str,
        operation: &str,
        bytes: f64,
        allocs: f64,
    ) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key = format!(
            "v2/testproj/alloc_tracker/x86_64-unknown-linux-gnu/synthetic/{sha}/clean.json"
        );
        self.seed(
            &key,
            &alloc_result_set(effective.as_second(), &sha, operation, bytes, allocs),
        );
    }

    /// Seeds one clean `all_the_time` result set for `operation` at the given
    /// `date` and commit `label`, recording `nanos` mean processor-time nanoseconds
    /// per iteration in the `machine`-keyed partition (processor time is
    /// hardware-dependent).
    fn seed_all_the_time(
        &self,
        date: &str,
        label: &str,
        machine: &str,
        operation: &str,
        nanos: f64,
    ) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key =
            format!("v2/testproj/all_the_time/x86_64-unknown-linux-gnu/{machine}/{sha}/clean.json");
        self.seed(
            &key,
            &time_result_set(effective.as_second(), &sha, operation, nanos),
        );
    }

    /// Seeds one clean `all_the_time` result set for `operation`, like
    /// [`Self::seed_all_the_time`], but additionally recording a confidence
    /// interval of `nanos` ± `half_width` so the noise detector's
    /// CI-non-overlap gate has dispersion to compare.
    fn seed_all_the_time_with_interval(
        &self,
        date: &str,
        label: &str,
        machine: &str,
        operation: &str,
        nanos: f64,
        half_width: f64,
    ) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key =
            format!("v2/testproj/all_the_time/x86_64-unknown-linux-gnu/{machine}/{sha}/clean.json");
        self.seed(
            &key,
            &time_result_set_with_dispersion(
                effective.as_second(),
                &sha,
                operation,
                nanos,
                half_width,
            ),
        );
    }
}

/// Builds a Callgrind result set with two records — `alpha::bench/wide` and
/// `beta::bench/narrow` — each carrying a single `Ir` metric, stamped with the
/// effective second and abbreviated `commit`.
fn two_benchmark_result_set(effective: i64, commit: &str, alpha: f64, beta: f64) -> ResultSet {
    let time = Timestamp::from_second(effective).expect("seconds within range");
    let mut git = GitInfo::default();
    git.commit = Some(format!("{commit}full"));
    git.short_commit = Some(commit.to_owned());
    git.branch = Some("main".to_owned());
    let context = RunContext::new(
        Timestamps::new(time, time, time),
        git,
        CiInfo::default(),
        ToolchainInfo::default(),
        TOOL_VERSION.to_owned(),
    );
    let ir = |value: f64| {
        vec![Metric::new(
            "Ir".to_owned(),
            MetricKind::InstructionCount,
            value,
            Some("count".to_owned()),
        )]
    };
    let records = vec![
        ResultRecord::new(
            BenchmarkId::new(
                Some("alpha".to_owned()),
                "alpha::bench".to_owned(),
                Some("wide".to_owned()),
                None,
            ),
            ir(alpha),
        ),
        ResultRecord::new(
            BenchmarkId::new(
                Some("beta".to_owned()),
                "beta::bench".to_owned(),
                Some("narrow".to_owned()),
                None,
            ),
            ir(beta),
        ),
    ];
    ResultSet::new(context, records)
}

/// Builds a Criterion result set for benchmark `time/capture/std_instant`
/// carrying a single `wall_time` metric at `value` nanoseconds, stamped with the
/// effective second and abbreviated `commit`.
fn criterion_result_set(effective: i64, commit: &str, value: f64) -> ResultSet {
    let time = Timestamp::from_second(effective).expect("seconds within range");
    let mut git = GitInfo::default();
    git.commit = Some(format!("{commit}full"));
    git.short_commit = Some(commit.to_owned());
    git.branch = Some("main".to_owned());
    let context = RunContext::new(
        Timestamps::new(time, time, time),
        git,
        CiInfo::default(),
        ToolchainInfo::default(),
        TOOL_VERSION.to_owned(),
    );
    let record = ResultRecord::new(
        BenchmarkId::new(
            None,
            "time/capture".to_owned(),
            Some("std_instant".to_owned()),
            None,
        ),
        vec![Metric::new(
            "wall_time".to_owned(),
            MetricKind::WallTime,
            value,
            Some("ns".to_owned()),
        )],
    );
    ResultSet::new(context, vec![record])
}

/// Builds an `alloc_tracker` result set for `operation` carrying an
/// `allocation_bytes` and an `allocation_count` metric, stamped with the effective
/// second and abbreviated `commit`.
fn alloc_result_set(
    effective: i64,
    commit: &str,
    operation: &str,
    bytes: f64,
    allocs: f64,
) -> ResultSet {
    let time = Timestamp::from_second(effective).expect("seconds within range");
    let mut git = GitInfo::default();
    git.commit = Some(format!("{commit}full"));
    git.short_commit = Some(commit.to_owned());
    git.branch = Some("main".to_owned());
    let context = RunContext::new(
        Timestamps::new(time, time, time),
        git,
        CiInfo::default(),
        ToolchainInfo::default(),
        TOOL_VERSION.to_owned(),
    );
    let record = ResultRecord::new(
        BenchmarkId::new(None, operation.to_owned(), None, None),
        vec![
            Metric::new(
                "allocated_bytes".to_owned(),
                MetricKind::AllocationBytes,
                bytes,
                Some("bytes".to_owned()),
            ),
            Metric::new(
                "allocations".to_owned(),
                MetricKind::AllocationCount,
                allocs,
                Some("count".to_owned()),
            ),
        ],
    );
    ResultSet::new(context, vec![record])
}

/// Builds an `all_the_time` result set for `operation` carrying a single
/// `processor_time` metric at `value` nanoseconds, stamped with the effective
/// second and abbreviated `commit`.
fn time_result_set(effective: i64, commit: &str, operation: &str, value: f64) -> ResultSet {
    let time = Timestamp::from_second(effective).expect("seconds within range");
    let mut git = GitInfo::default();
    git.commit = Some(format!("{commit}full"));
    git.short_commit = Some(commit.to_owned());
    git.branch = Some("main".to_owned());
    let context = RunContext::new(
        Timestamps::new(time, time, time),
        git,
        CiInfo::default(),
        ToolchainInfo::default(),
        TOOL_VERSION.to_owned(),
    );
    let record = ResultRecord::new(
        BenchmarkId::new(None, operation.to_owned(), None, None),
        vec![Metric::new(
            "processor_time".to_owned(),
            MetricKind::ProcessorTime,
            value,
            Some("ns".to_owned()),
        )],
    );
    ResultSet::new(context, vec![record])
}

/// Builds an `all_the_time` result set for `operation` carrying a single
/// `processor_time` metric at `value` nanoseconds, like [`time_result_set`],
/// but additionally recording a confidence interval of `value` ± `half_width`
/// (and a matching standard deviation) so the noise detector can apply its
/// CI-non-overlap gate to processor time.
fn time_result_set_with_dispersion(
    effective: i64,
    commit: &str,
    operation: &str,
    value: f64,
    half_width: f64,
) -> ResultSet {
    let time = Timestamp::from_second(effective).expect("seconds within range");
    let mut git = GitInfo::default();
    git.commit = Some(format!("{commit}full"));
    git.short_commit = Some(commit.to_owned());
    git.branch = Some("main".to_owned());
    let context = RunContext::new(
        Timestamps::new(time, time, time),
        git,
        CiInfo::default(),
        ToolchainInfo::default(),
        TOOL_VERSION.to_owned(),
    );
    let record = ResultRecord::new(
        BenchmarkId::new(None, operation.to_owned(), None, None),
        vec![
            Metric::new(
                "processor_time".to_owned(),
                MetricKind::ProcessorTime,
                value,
                Some("ns".to_owned()),
            )
            .with_dispersion(
                Some(half_width),
                Some(value - half_width),
                Some(value + half_width),
            ),
        ],
    );
    ResultSet::new(context, vec![record])
}
fn ir_of(record: &ResultRecord) -> f64 {
    record
        .metrics
        .iter()
        .find(|metric| metric.name == "Ir")
        .expect("instruction count should be present")
        .value
}

/// The metric named `name` within a record.
fn metric_named<'a>(record: &'a ResultRecord, name: &str) -> &'a Metric {
    record
        .metrics
        .iter()
        .find(|metric| metric.name == name)
        .unwrap_or_else(|| panic!("metric {name:?} should be present"))
}

/// Escapes a string for embedding in a TOML basic (double-quoted) string, so a
/// project id with unusual characters survives as a literal.
fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A configuration with local storage but no engines, used by `analyze` tests
/// that seed history directly and never launch an engine.
fn storage_only_config() -> String {
    "[project]\n\
     id = \"testproj\"\n\n\
     [storage.local]\n\
     path = \"./store\"\n"
        .to_owned()
}

/// Builds a Callgrind result set for benchmark `nm::observe/pull` carrying the
/// given `metrics`, stamped with the effective second and abbreviated `commit`.
fn result_set_with(effective: i64, commit: &str, metrics: Vec<Metric>) -> ResultSet {
    let time = Timestamp::from_second(effective).expect("seconds within range");
    let mut git = GitInfo::default();
    git.commit = Some(format!("{commit}full"));
    git.short_commit = Some(commit.to_owned());
    git.branch = Some("main".to_owned());
    let context = RunContext::new(
        Timestamps::new(time, time, time),
        git,
        CiInfo::default(),
        ToolchainInfo::default(),
        TOOL_VERSION.to_owned(),
    );
    let record = ResultRecord::new(
        BenchmarkId::new(
            Some("nm".to_owned()),
            "nm::observe".to_owned(),
            Some("pull".to_owned()),
            None,
        ),
        metrics,
    );
    ResultSet::new(context, vec![record])
}

/// Builds a Callgrind result set with a single `Ir` metric at `value`, stamped
/// with the given effective second and abbreviated `commit`.
fn ir_result_set(effective: i64, commit: &str, value: f64) -> ResultSet {
    result_set_with(
        effective,
        commit,
        vec![Metric::new(
            "Ir".to_owned(),
            MetricKind::InstructionCount,
            value,
            Some("count".to_owned()),
        )],
    )
}

/// A configuration with local storage under an explicit project `id` (which may
/// contain characters that require sanitizing for the storage partition).
fn storage_only_config_with_id(id: &str) -> String {
    format!(
        "[project]\n\
         id = \"{}\"\n\n\
         [storage.local]\n\
         path = \"./store\"\n",
        toml_escape(id)
    )
}

fn collect_json_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_json_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "json") {
            out.push(path);
        }
    }
}
