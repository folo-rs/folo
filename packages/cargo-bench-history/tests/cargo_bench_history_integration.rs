//! Integration tests exercising the public command surface end to end: parse
//! arguments through `argh`, translate to the typed command, and dispatch through
//! `run` against the real process, filesystem-harvest, and local-storage adapters.
#![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]
#![allow(
    clippy::float_cmp,
    reason = "metric values are exact integer-derived counts"
)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use argh::FromArgs;
use cargo_bench_history::{
    BenchmarkId, CiInfo, Cli, Command, GitInfo, Metric, MetricKind, ResultRecord, ResultSet,
    RunContext, RunError, RunOutcome, SCHEMA_VERSION, Timestamps, ToolchainInfo, default_template,
    run, run_with_overrides,
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

fn command_from(args: &[&str]) -> Command {
    Cli::from_args(&["cargo-bench-history"], args)
        .expect("arguments should parse")
        .into_command()
}

// ===========================================================================
// CLI surface (pure argument parsing; no IO, Miri-safe).
// ===========================================================================

#[test]
#[serial]
fn run_forwards_passthrough_after_separator() {
    let Command::Run(options) = command_from(&["run", "--", "--noplot"]) else {
        panic!("expected run command");
    };
    assert_eq!(options.passthrough, vec!["--noplot".to_owned()]);
}

#[test]
#[serial]
fn run_collects_package_and_bench_scope() {
    let Command::Run(options) = command_from(&["run", "-p", "nm", "--bench", "nm_observe"]) else {
        panic!("expected run command");
    };
    assert_eq!(options.packages, vec!["nm".to_owned()]);
    assert_eq!(options.benches, vec!["nm_observe".to_owned()]);
}

#[test]
#[serial]
fn unknown_subcommand_is_rejected() {
    Cli::from_args(&["cargo-bench-history"], &["frobnicate"])
        .expect_err("an unknown subcommand should be rejected");
}

#[test]
#[serial]
fn run_rejects_unknown_flag() {
    Cli::from_args(&["cargo-bench-history"], &["run", "--frobnicate"])
        .expect_err("an unknown flag should be rejected");
}

#[test]
#[serial]
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

// ===========================================================================
// Binary entry point: exit codes and output stream routing.
//
// These spawn the built binary to verify that `main` distinguishes a `--help`
// early exit (success, stdout) from a parse error (failure, stderr), rather than
// inferring success from the presence of the word "help" in the output.
// ===========================================================================

#[test]
#[cfg_attr(miri, ignore)] // Spawns a real process, which Miri cannot do.
#[serial]
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
#[serial]
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

// ===========================================================================
// Real-adapter `install` scenarios.
// ===========================================================================

#[tokio::test]
#[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot do.
#[serial]
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
#[serial]
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
#[serial]
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
// `LocalStorage` adapter. No engine is launched. They are `#[serial]` (CWD) and
// miri-ignored (real filesystem).
// ===========================================================================

/// An empty history analyzes cleanly and reports nothing.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
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
#[serial]
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
    assert!(report.contains("nm::observe/pull/Ir"), "{report}");
}

/// A rising Criterion `wall_time` history is flagged as a regression, proving the
/// analyzer reconstructs series across a machine-keyed partition too.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn analyze_detects_criterion_wall_time_regression() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_criterion_history("mk-fixed");

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
    assert!(
        report.contains("time/capture/std_instant/wall_time"),
        "{report}"
    );
}

/// `--fail-on-regression` makes a flagged regression an unsuccessful outcome.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn analyze_fail_on_regression_yields_failure() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let outcome = workspace
        .drive(&["analyze", "--fail-on-regression"])
        .await
        .expect("analysis should succeed");
    assert!(
        !outcome.is_success(),
        "a gated regression must be unsuccessful: {outcome:?}"
    );
}

/// Without the gate, a flagged regression is still a successful outcome.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn analyze_regression_without_gate_is_successful() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_callgrind_history();

    let outcome = workspace
        .drive(&["analyze"])
        .await
        .expect("analysis should succeed");
    assert!(outcome.is_success(), "{outcome:?}");
}

/// `--format json` renders a structured, parseable report.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
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
#[serial]
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
    assert!(report.contains("| Severity | Direction |"), "{report}");
}

/// `--since` excludes points before the cutoff, so an early spike is dropped and
/// the remaining flat history shows no change.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
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
#[serial]
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
#[serial]
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
#[serial]
async fn analyze_metric_filter_excludes_other_metrics() {
    let workspace = Workspace::repo(&storage_only_config());
    // `Ir` stays flat while `EstimatedCycles` climbs into a regression.
    for (date, commit, cycles) in [
        ("2024-01-01", "c1", 100.0),
        ("2024-01-02", "c2", 100.0),
        ("2024-01-03", "c3", 100.0),
        ("2024-01-04", "c4", 130.0),
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
#[serial]
async fn analyze_falling_cache_hits_is_a_regression() {
    let workspace = Workspace::repo(&storage_only_config());
    for (date, commit, hits) in [
        ("2024-01-01", "c1", 1000.0),
        ("2024-01-02", "c2", 1000.0),
        ("2024-01-03", "c3", 1000.0),
        ("2024-01-04", "c4", 700.0),
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

/// Conversely, a rise in cache hits is an improvement, not a regression.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn analyze_rising_cache_hits_is_an_improvement() {
    let workspace = Workspace::repo(&storage_only_config());
    for (date, commit, hits) in [
        ("2024-01-01", "c1", 700.0),
        ("2024-01-02", "c2", 700.0),
        ("2024-01-03", "c3", 700.0),
        ("2024-01-04", "c4", 1000.0),
    ] {
        workspace.seed_metrics(
            date,
            commit,
            vec![Metric::new(
                "LLhits".to_owned(),
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
    assert_eq!(
        regressions, 0,
        "more cache hits is not a regression: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert_eq!(parsed["findings"][0]["direction"], "improvement");
}

/// Two benchmarks regressing by different magnitudes produce two findings that the
/// report ranks most-notable first (major before minor), end to end through real
/// storage and rendering.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn analyze_ranks_mixed_severity_findings_across_benchmarks() {
    let workspace = Workspace::repo(&storage_only_config());
    // `alpha` doubles (a major regression); `beta` ticks up ~2% (a minor one).
    workspace.seed_two_benchmarks("2024-01-01", "c1", 100.0, 100.0);
    workspace.seed_two_benchmarks("2024-01-02", "c2", 100.0, 100.0);
    workspace.seed_two_benchmarks("2024-01-03", "c3", 100.0, 100.0);
    workspace.seed_two_benchmarks("2024-01-04", "c4", 200.0, 102.0);

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
    // The major regression (alpha, +100%) ranks ahead of the minor one (beta, +2%).
    assert_eq!(findings[0]["severity"], "major", "{report}");
    assert_eq!(findings[0]["package"], "alpha", "{report}");
    assert_eq!(findings[0]["group"], "alpha::bench", "{report}");
    assert_eq!(findings[1]["severity"], "minor", "{report}");
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
    let major_row = markdown
        .find("| major | regression |")
        .expect("a major row");
    let minor_row = markdown
        .find("| minor | regression |")
        .expect("a minor row");
    assert!(
        major_row < minor_row,
        "major must precede minor:\n{markdown}"
    );
}
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn analyze_reports_improvement_without_regression() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "c2", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 100.0);
    workspace.seed_callgrind("2024-01-04", "c4", 70.0);

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
        "a drop in instructions is not a regression: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert_eq!(parsed["findings"][0]["direction"], "improvement");
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
#[serial]
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
#[serial]
async fn analyze_official_view_excludes_dirty_runs() {
    let workspace = Workspace::repo(&storage_only_config());
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

/// A feature view admits dirty snapshots on the branch's own commits (after the
/// merge-base): a dirty regression on the feature tip flags by default, and
/// `--no-dirty` suppresses it.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn analyze_feature_branch_admits_dirty_snapshots() {
    let workspace = Workspace::repo(&storage_only_config());
    // A flat clean baseline on master.
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "c2", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 100.0);
    // Branch off master and add a clean point plus a dirty regression on it.
    workspace.checkout_new_branch("feature");
    workspace.seed_callgrind("2024-01-04", "f1", 100.0);
    workspace.seed_dirty_callgrind("2024-01-05", "f1", 200.0);

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
#[serial]
async fn analyze_orders_by_topology_not_effective_time() {
    let workspace = Workspace::repo(&storage_only_config());
    // Topology c1 -> c2 -> c3 -> c4, but effective times strictly descend, so an
    // effective-time ordering would put the 130 first (a non-regression).
    workspace.seed_callgrind("2024-04-04", "c1", 100.0);
    workspace.seed_callgrind("2024-04-03", "c2", 100.0);
    workspace.seed_callgrind("2024-04-02", "c3", 100.0);
    workspace.seed_callgrind("2024-04-01", "c4", 130.0);

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
        "topology order must place the 130 last and flag it: {report}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
    assert_eq!(parsed["findings"][0]["baseline"], 100.0, "{report}");
    assert_eq!(parsed["findings"][0]["latest"], 130.0, "{report}");
}

/// `--list-discriminants` enumerates exactly the comparable sets present in
/// storage, deriving the os/architecture facets from each set's target triple.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn analyze_list_discriminants_lists_present_sets() {
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
        .drive(&["analyze", "--list-discriminants", "--format", "json"])
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

/// A facet filter (`--os`) restricts analysis to the matching set: the same commits
/// host a regressing Linux series and a flat Windows series, and each `--os`
/// selection sees only its own.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn analyze_os_facet_selects_one_set() {
    let workspace = Workspace::repo(&storage_only_config());
    // Each commit carries both a Linux point (rising into a regression) and a
    // Windows point (flat).
    for (date, label, linux, windows) in [
        ("2024-01-01", "c1", 100.0, 50.0),
        ("2024-01-02", "c2", 100.0, 50.0),
        ("2024-01-03", "c3", 100.0, 50.0),
        ("2024-01-04", "c4", 130.0, 50.0),
    ] {
        workspace.seed_callgrind_in("x86_64-unknown-linux-gnu", "synthetic", date, label, linux);
        workspace.seed_callgrind_in("x86_64-pc-windows-msvc", "synthetic", date, label, windows);
    }

    // The Linux facet sees the regression.
    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&["analyze", "--os", "linux"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the Linux series regresses");

    // The Windows facet sees only the flat series.
    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--os", "windows"])
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
#[serial]
async fn analyze_branch_selects_official_line_from_a_feature_checkout() {
    let workspace = Workspace::repo(&storage_only_config());
    // Master carries a clean regression.
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "c2", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 100.0);
    workspace.seed_callgrind("2024-01-04", "c4", 130.0);
    // A feature branch with an unrelated dirty improvement that master must ignore.
    workspace.checkout_new_branch("feature");
    workspace.seed_dirty_callgrind("2024-01-05", "f1", 10.0);

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--branch", "master", "--format", "json"])
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
#[serial]
async fn analyze_official_line_follows_first_parent_across_a_merge() {
    let workspace = Workspace::repo(&storage_only_config());
    // master:  root - c1 - M - c3   (M merges the side branch into master)
    //                  \   /
    //  side:            sf1 - sf2
    workspace.commit("c1");
    workspace.checkout_new_branch("side");
    workspace.commit("sf1");
    workspace.commit("sf2");
    workspace.checkout("master");
    workspace.merge("side", "M");
    workspace.commit("c3");

    // The first-parent line carries a clean regression on its tip.
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    workspace.seed_callgrind("2024-01-02", "M", 100.0);
    workspace.seed_callgrind("2024-01-03", "c3", 130.0);
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
        parsed["runs"], 3,
        "only the first-parent line c1 -> M -> c3 is analyzed, not the side branch: {report}"
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
#[serial]
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
#[serial]
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
        .drive(&["analyze", "--format", "json"])
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

/// `--architecture` selects a single comparable set: the same commits host a
/// regressing `x86_64` series and a flat `aarch64` series, and each `--architecture`
/// selection sees only its own.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn analyze_architecture_facet_selects_one_set() {
    let workspace = Workspace::repo(&storage_only_config());
    for (date, label, x64, arm) in [
        ("2024-01-01", "c1", 100.0, 50.0),
        ("2024-01-02", "c2", 100.0, 50.0),
        ("2024-01-03", "c3", 100.0, 50.0),
        ("2024-01-04", "c4", 130.0, 50.0),
    ] {
        workspace.seed_callgrind_in("x86_64-unknown-linux-gnu", "synthetic", date, label, x64);
        workspace.seed_callgrind_in("aarch64-unknown-linux-gnu", "synthetic", date, label, arm);
    }

    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&["analyze", "--architecture", "x86_64"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 1, "the x86_64 series regresses");

    let RunOutcome::Analyzed {
        regressions,
        report,
        ..
    } = workspace
        .drive(&["analyze", "--architecture", "aarch64"])
        .await
        .expect("analysis succeeds")
    else {
        panic!("expected an analyzed outcome");
    };
    assert_eq!(regressions, 0, "the aarch64 series is flat: {report}");
}

/// `--machine-key` selects a single comparable set: with a regressing machine and
/// a flat machine on the same triple, each `--machine-key` selection sees only
/// the named machine's series.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn analyze_machine_key_facet_selects_one_set() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.seed_rising_criterion_history("mk-rising");
    workspace.seed_criterion("2024-02-01", "d1", "mk-flat", 20.0);
    workspace.seed_criterion("2024-02-02", "d2", "mk-flat", 20.0);
    workspace.seed_criterion("2024-02-03", "d3", "mk-flat", 20.0);
    workspace.seed_criterion("2024-02-04", "d4", "mk-flat", 20.0);

    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&["analyze", "--machine-key", "mk-rising"])
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
        .drive(&["analyze", "--machine-key", "mk-flat"])
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
#[serial]
async fn analyze_criterion_feature_branch_admits_dirty_snapshots() {
    let workspace = Workspace::repo(&storage_only_config());
    // A flat clean Criterion baseline on master.
    workspace.seed_criterion("2024-02-01", "c1", "mk", 20.0);
    workspace.seed_criterion("2024-02-02", "c2", "mk", 20.0);
    workspace.seed_criterion("2024-02-03", "c3", "mk", 20.0);
    // Branch off master and add a clean point plus a dirty regression on it.
    workspace.checkout_new_branch("feature");
    workspace.seed_criterion("2024-02-04", "f1", "mk", 20.0);
    workspace.seed_dirty_criterion("2024-02-05", "f1", "mk", 40.0);

    let RunOutcome::Analyzed { regressions, .. } = workspace
        .drive(&["analyze"])
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
        .drive(&["analyze", "--no-dirty"])
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
// within a temporary workspace. They are `#[serial]` because they mutate the
// process-wide current directory and `CARGO_TARGET_DIR`, and miri-ignored because
// they spawn processes and touch the filesystem. Every test in this file is
// `#[serial]` so that the environment is never accessed concurrently (see
// `TargetDirGuard`).
// ===========================================================================

/// End-to-end happy path: a successful no-op engine command, one harvested
/// summary, and a stored set with the expected object key and context.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_callgrind_end_to_end_stores_results() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&["--summary", "grp=single"]);

    let outcome = workspace
        .drive(&[
            "run",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--timestamp",
            "2020-01-01T00:00:00Z",
        ])
        .await
        .expect("run should succeed");
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 1"), "{message}");

    let (key, set) = workspace.single_object();

    // Synthetic partition (Callgrind is hardware-independent) under the resolved
    // triple. The temp workspace is outside any git repository, so the commit
    // resolves to the `unknown` fallback and the clean tree yields `clean.json`.
    let effective: Timestamp = "2020-01-01T00:00:00Z".parse().unwrap();
    assert_eq!(
        key,
        "v2/testproj/callgrind/x86_64-unknown-linux-gnu/synthetic/unknown/clean.json"
    );

    assert_eq!(set.schema_version, SCHEMA_VERSION);
    assert_eq!(
        set.context.toolchain.target_triple,
        "x86_64-unknown-linux-gnu"
    );
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
#[serial]
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
#[serial]
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
#[serial]
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
#[serial]
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
#[serial]
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
#[serial]
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
#[serial]
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
/// writes both a Callgrind summary and a Criterion case, so the run stores one
/// result set per engine in its own partition.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_harvests_every_engine_that_produced_output() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&[
        "--summary",
        "grp=single",
        "--criterion",
        "grp|capture|now=12.5",
    ]);

    let outcome = workspace.drive(&["run"]).await.expect("run should succeed");
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 2"), "{message}");

    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 2, "{objects:?}");
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
}

/// `--machine-key` overrides the machine fingerprint in a Criterion partition.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_criterion_honors_machine_key_override() {
    let workspace =
        Workspace::new(&storage_only_config()).with_bench(&["--criterion", "grp|capture|now=9"]);

    // Pin the triple so the full partition is deterministic across host platforms;
    // the override segment is what this test asserts.
    workspace
        .drive(&[
            "run",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--machine-key",
            "ci-pool-a",
        ])
        .await
        .expect("run should succeed");

    let (key, _) = workspace.single_object();
    assert!(
        key.contains("/criterion/x86_64-unknown-linux-gnu/ci-pool-a/"),
        "{key}"
    );
}

/// A Criterion run collects every harvested case into one result set, keeping
/// distinct group/function/value identities as separate records.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
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
#[serial]
async fn run_then_analyze_round_trips_a_sanitizing_project_id() {
    let workspace = Workspace::clean_repo(&storage_only_config_with_id("my proj/sub"))
        .with_bench(&["--summary", "grp=single"]);

    workspace
        .drive(&[
            "run",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--timestamp",
            "2020-01-01T00:00:00Z",
        ])
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
#[serial]
async fn run_then_analyze_preserves_unusual_identity_characters() {
    let workspace = Workspace::clean_repo(&storage_only_config())
        .with_bench(&["--criterion", "time.capture|mide tiempo|tamaño 4=18.5"]);

    workspace
        .drive(&[
            "run",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--machine-key",
            "pool",
            "--timestamp",
            "2020-01-01T00:00:00Z",
        ])
        .await
        .expect("run should succeed");

    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 1, "{objects:?}");
    let (key, set) = &objects[0];
    // The partition key is identity-free and fully sanitized: none of the
    // identity's spaces or non-ASCII letters leak into it.
    assert!(
        key.starts_with("v2/testproj/criterion/x86_64-unknown-linux-gnu/pool/"),
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
        .drive(&["analyze", "--format", "json"])
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
#[serial]
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
#[serial]
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
#[serial]
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
// the mock engine writes one summary there. They are `#[serial]` (current
// directory) and `#[cfg_attr(miri, ignore)]` (real git, processes, filesystem).
// ===========================================================================

/// A backfill stores one clean result per commit in the range, leaves the primary
/// checkout and branch untouched, and the backfilled points then surface through
/// `analyze` in git-topology order.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn backfill_stores_one_clean_object_per_commit_and_restores_checkout() {
    let workspace =
        Workspace::clean_repo(&storage_only_config()).with_bench(&["--summary", "grp=single"]);
    let c1 = workspace.commit("c1");
    let c2 = workspace.commit("c2");
    let c3 = workspace.commit("c3");
    let branch_before = workspace.current_branch();
    let head_before = workspace.head();

    let RunOutcome::Completed { message } = workspace
        .drive(&[
            "backfill",
            "--from",
            &c1,
            "--to",
            &c3,
            "--target-triple",
            "x86_64-unknown-linux-gnu",
        ])
        .await
        .expect("backfill should succeed")
    else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("3 stored"), "{message}");

    // One clean object per commit, keyed by that commit's full SHA.
    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 3, "{objects:?}");
    for sha in [&c1, &c2, &c3] {
        let expected =
            format!("v2/testproj/callgrind/x86_64-unknown-linux-gnu/synthetic/{sha}/clean.json");
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
#[serial]
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
        .drive(&[
            "backfill",
            "--from",
            &c1,
            "--to",
            &c3,
            "--target-triple",
            "x86_64-unknown-linux-gnu",
        ])
        .await
        .expect("backfill should succeed")
    else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("3 stored"), "{message}");

    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 3, "{objects:?}");
    for sha in [&c1, &m, &c3] {
        let expected =
            format!("v2/testproj/callgrind/x86_64-unknown-linux-gnu/synthetic/{sha}/clean.json");
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
#[serial]
async fn backfill_skips_already_stored_commits_on_rerun() {
    let workspace =
        Workspace::clean_repo(&storage_only_config()).with_bench(&["--summary", "grp=single"]);
    let c1 = workspace.commit("c1");
    let c2 = workspace.commit("c2");

    workspace
        .drive(&["backfill", "--from", &c1, "--to", &c2])
        .await
        .expect("the first backfill should store both commits");
    assert_eq!(workspace.stored_objects().len(), 2);

    let RunOutcome::Completed { message } = workspace
        .drive(&["backfill", "--from", &c1, "--to", &c2])
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

/// `--overwrite` replaces every already-stored commit in the range rather than
/// skipping it, leaving exactly one clean object per commit.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn backfill_overwrite_replaces_already_stored_commits() {
    let workspace =
        Workspace::clean_repo(&storage_only_config()).with_bench(&["--summary", "grp=single"]);
    let c1 = workspace.commit("c1");
    let c2 = workspace.commit("c2");

    workspace
        .drive(&["backfill", "--from", &c1, "--to", &c2])
        .await
        .expect("the first backfill should store both commits");

    let RunOutcome::Completed { message } = workspace
        .drive(&["backfill", "--from", &c1, "--to", &c2, "--overwrite"])
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
#[serial]
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
        .drive(&["backfill", "--from", &c1, "--to", &c3])
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
#[serial]
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
        .drive(&["backfill", "--from", &c1, "--to", &c3, "--ignore-errors"])
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

/// A dirty working tree is refused before any worktree work, so nothing is stored.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn backfill_refuses_a_dirty_working_tree() {
    let workspace =
        Workspace::clean_repo(&storage_only_config()).with_bench(&["--summary", "grp=single"]);
    let c1 = workspace.commit("c1");
    workspace.make_dirty("uncommitted.txt");

    let error = workspace
        .drive(&["backfill", "--from", &c1, "--to", &c1])
        .await
        .expect_err("a dirty tree must be refused");
    let RunError::Backfill { message } = error else {
        panic!("expected a backfill error, got {error:?}");
    };
    assert!(message.contains("uncommitted changes"), "{message}");
    assert!(workspace.stored_objects().is_empty());
}

/// A range whose `--from` is not a first-parent ancestor of `--to` (here, a
/// reversed range) is rejected before any worktree work, storing nothing.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn backfill_rejects_a_range_outside_the_first_parent_history() {
    let workspace =
        Workspace::clean_repo(&storage_only_config()).with_bench(&["--summary", "grp=single"]);
    let c1 = workspace.commit("c1");
    let c2 = workspace.commit("c2");

    let error = workspace
        .drive(&["backfill", "--from", &c2, "--to", &c1])
        .await
        .expect_err("a reversed range must be rejected");
    let RunError::Backfill { message } = error else {
        panic!("expected a backfill error, got {error:?}");
    };
    assert!(message.contains("not a first-parent ancestor"), "{message}");
    assert!(workspace.stored_objects().is_empty());
}

/// `backfill --help` is an early exit whose usage text documents the range flags.
#[test]
#[serial]
fn backfill_help_documents_range_flags() {
    let early_exit = Cli::from_args(&["cargo-bench-history"], &["backfill", "--help"])
        .expect_err("help is reported as an early exit");
    assert!(
        early_exit.output.contains("--from"),
        "{}",
        early_exit.output
    );
    assert!(early_exit.output.contains("--to"), "{}", early_exit.output);
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
    /// output tree is produced by the single benchmark command vNext runs.
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

    /// Creates a workspace with `config` plus an initialized git repository whose
    /// working tree stays *clean* across a `run`: the volatile directories a run
    /// touches (`.cargo`, `store`, `target`) are git-ignored, so the real probe
    /// records the root commit as a clean (not dirty) run. Used by the end-to-end
    /// `run` -> `analyze` round-trip tests.
    fn clean_repo(config: &str) -> Self {
        let workspace = Self::new(config);
        std::fs::write(
            workspace.root().join(".gitignore"),
            "/.cargo/\n/store/\n/target/\n",
        )
        .unwrap();
        workspace.git(&["init", "-b", "master"]);
        workspace.git(&["config", "user.email", "test@example.invalid"]);
        workspace.git(&["config", "user.name", "Bench History Test"]);
        workspace.git(&["config", "commit.gpgsign", "false"]);
        workspace.git(&["add", ".gitignore"]);
        workspace.git(&["commit", "-m", "root"]);
        workspace
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
    fn git(&self, args: &[&str]) -> std::process::Output {
        let root = self.root().to_string_lossy().into_owned();
        let mut full: Vec<&str> = vec!["-C", root.as_str()];
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

    /// Initializes a `master`-branch repository with a deterministic identity and
    /// one empty root commit so `HEAD` always resolves.
    fn init_repo(&self) {
        self.git(&["init", "-b", "master"]);
        self.git(&["config", "user.email", "test@example.invalid"]);
        self.git(&["config", "user.name", "Bench History Test"]);
        self.git(&["config", "commit.gpgsign", "false"]);
        self.git(&["commit", "--allow-empty", "-m", "root"]);
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
        let head = self.git(&["rev-parse", "HEAD"]);
        let sha = String::from_utf8(head.stdout)
            .expect("a commit SHA is ASCII")
            .trim()
            .to_owned();
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

    /// Drives a command with `args` from inside this workspace.
    async fn drive(&self, args: &[&str]) -> Result<RunOutcome, RunError> {
        // Restore the working directory when this returns (and before the temp
        // dir is dropped, which on Windows fails while it is the current
        // directory), even if the harvest panics.
        let _cwd = CwdGuard::enter(self.root());

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

        run_with_overrides(&command_from(args), Some(target_root), Some(bench_command)).await
    }

    /// Drives a command the way the production `run` entry does: with no target-root
    /// override, so the real `resolve_target_root` chooses the cargo target
    /// directory and injects it as `CARGO_TARGET_DIR`. Unlike [`drive`], this
    /// exercises that resolution rather than pinning the root explicitly, which is
    /// what lets a test observe whether the injected root is absolute.
    ///
    /// [`drive`]: Self::drive
    async fn drive_resolving_target_root(&self, args: &[&str]) -> Result<RunOutcome, RunError> {
        let _cwd = CwdGuard::enter(self.root());

        let mut bench_command = vec![MOCK_ENGINE.to_owned()];
        bench_command.extend(self.bench.iter().cloned());

        run_with_overrides(&command_from(args), None, Some(bench_command)).await
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

    /// Seeds a flat history followed by a clear upward step — a regression.
    fn seed_rising_callgrind_history(&self) {
        self.seed_callgrind("2024-01-01", "c1", 100.0);
        self.seed_callgrind("2024-01-02", "c2", 100.0);
        self.seed_callgrind("2024-01-03", "c3", 100.0);
        self.seed_callgrind("2024-01-04", "c4", 130.0);
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

    /// Seeds a flat Criterion `wall_time` history then a clear upward step.
    fn seed_rising_criterion_history(&self, machine: &str) {
        self.seed_criterion("2024-02-01", "d1", machine, 20.0);
        self.seed_criterion("2024-02-02", "d2", machine, 20.0);
        self.seed_criterion("2024-02-03", "d3", machine, 20.0);
        self.seed_criterion("2024-02-04", "d4", machine, 30.0);
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

/// The `Ir` (instruction count) metric value of a record.
fn ir_of(record: &ResultRecord) -> f64 {
    record
        .metrics
        .iter()
        .find(|metric| metric.name == "Ir")
        .expect("instruction count should be present")
        .value
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
