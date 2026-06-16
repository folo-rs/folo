//! Integration tests exercising the public command surface end to end: parse
//! arguments through `argh`, translate to the typed command, and dispatch through
//! `run` against the real process, filesystem-harvest, and local-storage adapters.
#![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]
#![allow(
    clippy::float_cmp,
    reason = "metric values are exact integer-derived counts"
)]

use std::path::{Path, PathBuf};

use argh::FromArgs;
use cargo_bench_history::{
    BenchmarkId, CiInfo, Cli, Command, GitInfo, Metric, MetricKind, ResultRecord, ResultSet,
    RunContext, RunError, RunOutcome, SCHEMA_VERSION, Timestamps, ToolchainInfo, default_template,
    parse_config, run, run_with_target_root,
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
    let Command::Run(options) = command_from(&["run", "--engine", "callgrind", "--", "-p", "nm"])
    else {
        panic!("expected run command");
    };
    assert_eq!(options.engine.as_deref(), Some("callgrind"));
    assert_eq!(options.passthrough, vec!["-p".to_owned(), "nm".to_owned()]);
}

#[test]
#[serial]
fn default_template_enables_both_engines() {
    let config = parse_config(default_template()).expect("template should parse");
    assert_eq!(config.engines.len(), 2);
    assert!(config.engines.contains_key("callgrind"));
    assert!(config.engines.contains_key("criterion"));
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
    let workspace = Workspace::new(&storage_only_config());

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
    let workspace = Workspace::new(&storage_only_config());
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
    let workspace = Workspace::new(&storage_only_config());
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
    let workspace = Workspace::new(&storage_only_config());
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
    let workspace = Workspace::new(&storage_only_config());
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
    let workspace = Workspace::new(&storage_only_config());
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
    let workspace = Workspace::new(&storage_only_config());
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
    let workspace = Workspace::new(&storage_only_config());
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

/// `--system` restricts analysis to one engine's partition.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn analyze_system_filters_partition() {
    let workspace = Workspace::new(&storage_only_config());
    workspace.seed_callgrind("2024-01-01", "c1", 100.0);
    // A criterion-partition object that the callgrind filter must skip.
    workspace.seed(
        "v1/testproj/criterion/x86_64-pc-windows-msvc/abc123/1-c1-r1.json",
        &ir_result_set(1, "c1", 100.0),
    );

    let outcome = workspace
        .drive(&["analyze", "--system", "callgrind", "--format", "json"])
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
    let workspace = Workspace::new(&storage_only_config());

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
    let workspace = Workspace::new(&storage_only_config());
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
    let workspace = Workspace::new(&storage_only_config());
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
    let workspace = Workspace::new(&storage_only_config());
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

/// A falling instruction count is reported as an improvement, not a regression.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn analyze_reports_improvement_without_regression() {
    let workspace = Workspace::new(&storage_only_config());
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
// Real-adapter `run` scenarios.
//
// Each drives `run` against the real process/filesystem/storage adapters from
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
    let workspace = Workspace::new(&callgrind_config(&mock_command("--summary grp=single")));

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
    // triple and the backfilled effective second; the run-id suffix is dynamic.
    let effective: Timestamp = "2020-01-01T00:00:00Z".parse().unwrap();
    let prefix = format!(
        "v1/testproj/callgrind/x86_64-unknown-linux-gnu/synthetic/{}-unknown-",
        effective.as_second()
    );
    assert!(key.starts_with(&prefix), "unexpected key: {key}");
    assert!(key.ends_with(".json"), "unexpected key: {key}");

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

/// Two summaries under the target tree yield one stored set with one record each.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_stores_a_record_per_summary() {
    let workspace = Workspace::new(&callgrind_config(&mock_command(
        "--summary a=single --summary b=parametrized",
    )));

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
    let workspace = Workspace::new(&callgrind_config(&mock_command(
        "--summary a=single --summary b=single-alt-pkg",
    )));

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

/// `--no-store` still harvests the output but writes nothing to storage.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_no_store_harvests_without_storing() {
    let workspace = Workspace::new(&callgrind_config(&mock_command("--summary grp=single")));

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
    let workspace = Workspace::new(&callgrind_config(&mock_command("--exit-code 7")));

    let error = workspace
        .drive(&["run"])
        .await
        .expect_err("a non-zero engine exit should fail the run");
    let RunError::Engine { engine, code } = error else {
        panic!("expected an engine error, got {error:?}");
    };
    assert_eq!(engine, "callgrind");
    assert_eq!(code, Some(7));

    assert!(
        workspace.stored_objects().is_empty(),
        "a failed run stores nothing"
    );
}

/// Requesting an engine that is not a known engine system is rejected.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_rejects_unknown_requested_engine() {
    let workspace = Workspace::new(&callgrind_config(&mock_command("")));

    let error = workspace
        .drive(&["run", "--engine", "dhat"])
        .await
        .expect_err("an unknown engine should be rejected");
    let RunError::NoEngine { message } = error else {
        panic!("expected a no-engine error, got {error:?}");
    };
    assert!(message.contains("unknown engine"), "{message}");
}

/// Requesting a known engine that the configuration does not list is rejected.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_rejects_unconfigured_requested_engine() {
    let workspace = Workspace::new(&callgrind_config(&mock_command("")));

    let error = workspace
        .drive(&["run", "--engine", "criterion"])
        .await
        .expect_err("an unconfigured engine should be rejected");
    let RunError::NoEngine { message } = error else {
        panic!("expected a no-engine error, got {error:?}");
    };
    assert!(message.contains("not configured"), "{message}");
}

/// Explicitly selecting the Criterion engine runs it end to end, storing a
/// wall-time result set in a machine-fingerprinted partition.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_explicit_criterion_selection_stores_results() {
    let workspace = Workspace::new(&criterion_config(&mock_command(
        "--criterion grp|capture|now=26.9",
    )));

    let outcome = workspace
        .drive(&["run", "--engine", "criterion"])
        .await
        .expect("run should succeed");
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

/// Without an explicit `--engine`, every configured engine runs and each stores
/// its own result set.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_executes_every_configured_engine() {
    let workspace = Workspace::new(&callgrind_and_criterion_config());

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

/// Explicitly selecting the supported engine works end to end.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_explicit_callgrind_selection_stores_results() {
    let workspace = Workspace::new(&callgrind_config(&mock_command("--summary grp=single")));

    let outcome = workspace
        .drive(&["run", "--engine", "callgrind"])
        .await
        .expect("run should succeed");
    assert!(matches!(outcome, RunOutcome::Completed { .. }));
    assert_eq!(workspace.stored_objects().len(), 1);
}

/// `--machine-key` overrides the machine fingerprint in a Criterion partition.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_criterion_honors_machine_key_override() {
    let workspace = Workspace::new(&criterion_config(&mock_command(
        "--criterion grp|capture|now=9",
    )));

    workspace
        .drive(&["run", "--engine", "criterion", "--machine-key", "ci-pool-a"])
        .await
        .expect("run should succeed");

    let (key, _) = workspace.single_object();
    assert!(
        key.contains("/criterion/x86_64-pc-windows-msvc/ci-pool-a/"),
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
    let command = mock_command(
        "--criterion timestamp/capture|std_instant|now=27 \
         --criterion timestamp/elapsed|std_instant|now=41 \
         --criterion timestamp/capture|fast_clock|=13",
    );
    let workspace = Workspace::new(&criterion_config(&command));

    workspace
        .drive(&["run", "--engine", "criterion"])
        .await
        .expect("run should succeed");

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

/// `--config` loads the configuration from a non-default path.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_uses_explicit_config_path() {
    // Only the custom path holds a configuration; the default discovery path is
    // absent, so a successful run proves `--config` was honored.
    let config = callgrind_config(&mock_command("--summary grp=single"));
    let workspace = Workspace::with_config_at("config/bench.toml", &config);

    let outcome = workspace
        .drive(&["run", "--config", "config/bench.toml"])
        .await
        .expect("run should succeed");
    assert!(matches!(outcome, RunOutcome::Completed { .. }));
    assert_eq!(workspace.stored_objects().len(), 1);
}

/// Two sequential runs store two distinct objects without clobbering.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn two_runs_store_two_distinct_objects() {
    let workspace = Workspace::new(&callgrind_config(&mock_command("--summary grp=single")));

    workspace
        .drive(&["run"])
        .await
        .expect("first run should succeed");

    // The engine rewrites the summary on each run, so its modification time stays
    // inside the second run's harvest window.
    workspace
        .drive(&["run"])
        .await
        .expect("second run should succeed");

    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 2, "each run should store a distinct object");
    assert_ne!(objects[0].0, objects[1].0, "object keys must differ");
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
}

impl Workspace {
    /// Creates a workspace with `config` at the default `.cargo/bench_history.toml`.
    fn new(config: &str) -> Self {
        let workspace = Self {
            dir: tempfile::tempdir().expect("temp dir should be created"),
        };
        let cargo_dir = workspace.root().join(".cargo");
        std::fs::create_dir_all(&cargo_dir).unwrap();
        std::fs::write(cargo_dir.join("bench_history.toml"), config).unwrap();
        workspace
    }

    /// Creates an empty workspace with no configuration file written.
    fn empty() -> Self {
        Self {
            dir: tempfile::tempdir().expect("temp dir should be created"),
        }
    }

    /// Creates a workspace with `config` at a non-default `relative` path.
    fn with_config_at(relative: &str, config: &str) -> Self {
        let workspace = Self {
            dir: tempfile::tempdir().expect("temp dir should be created"),
        };
        let path = workspace.root().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, config).unwrap();
        workspace
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Reads a file relative to the workspace root, if it exists.
    fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.root().join(relative)).ok()
    }

    /// Drives `run` with `args` from inside this workspace.
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
        run_with_target_root(&command_from(args), Some(target_root)).await
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
    /// (`YYYY-MM-DD`, taken at UTC midnight) and abbreviated `commit`.
    fn seed_callgrind(&self, date: &str, commit: &str, value: f64) {
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key = format!(
            "v1/testproj/callgrind/x86_64-unknown-linux-gnu/synthetic/{}-{commit}-run.json",
            effective.as_second()
        );
        self.seed(&key, &ir_result_set(effective.as_second(), commit, value));
    }

    /// Seeds one Callgrind result set carrying `metrics` for benchmark
    /// `nm::observe/pull` at the given `date` (`YYYY-MM-DD`, UTC midnight) and
    /// abbreviated `commit`.
    fn seed_metrics(&self, date: &str, commit: &str, metrics: Vec<Metric>) {
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key = format!(
            "v1/testproj/callgrind/x86_64-unknown-linux-gnu/synthetic/{}-{commit}-run.json",
            effective.as_second()
        );
        self.seed(
            &key,
            &result_set_with(effective.as_second(), commit, metrics),
        );
    }

    /// Seeds a flat history followed by a clear upward step — a regression.
    fn seed_rising_callgrind_history(&self) {
        self.seed_callgrind("2024-01-01", "c1", 100.0);
        self.seed_callgrind("2024-01-02", "c2", 100.0);
        self.seed_callgrind("2024-01-03", "c3", 100.0);
        self.seed_callgrind("2024-01-04", "c4", 130.0);
    }

    /// Seeds one Criterion `wall_time` result set at the given `date` (`YYYY-MM-DD`,
    /// UTC midnight), abbreviated `commit`, and machine-key partition `machine`.
    fn seed_criterion(&self, date: &str, commit: &str, machine: &str, value: f64) {
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key = format!(
            "v1/testproj/criterion/x86_64-pc-windows-msvc/{machine}/{}-{commit}-run.json",
            effective.as_second()
        );
        self.seed(
            &key,
            &criterion_result_set(effective.as_second(), commit, value),
        );
    }

    /// Seeds a flat Criterion `wall_time` history then a clear upward step.
    fn seed_rising_criterion_history(&self, machine: &str) {
        self.seed_criterion("2024-02-01", "d1", machine, 20.0);
        self.seed_criterion("2024-02-02", "d2", machine, 20.0);
        self.seed_criterion("2024-02-03", "d3", machine, 20.0);
        self.seed_criterion("2024-02-04", "d4", machine, 30.0);
    }
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

/// Builds a config `command` line that invokes the mock engine with `args`.
///
/// The engine path is POSIX single-quoted so shell-word tokenization treats it
/// as a single argument even when it contains spaces or backslashes (as on
/// Windows); `args` is appended verbatim.
fn mock_command(args: &str) -> String {
    let quoted = format!("'{}'", MOCK_ENGINE.replace('\'', r"'\''"));
    if args.is_empty() {
        quoted
    } else {
        format!("{quoted} {args}")
    }
}

/// Escapes a string for embedding in a TOML basic (double-quoted) string, so a
/// Windows path's backslashes survive as literal characters.
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

/// A single-engine Callgrind configuration whose command is `command`.
fn callgrind_config(command: &str) -> String {
    format!(
        "[project]\n\
         id = \"testproj\"\n\n\
         [storage.local]\n\
         path = \"./store\"\n\n\
         [engines.callgrind]\n\
         command = \"{}\"\n",
        toml_escape(command)
    )
}

/// A single-engine Criterion configuration whose command is `command`.
fn criterion_config(command: &str) -> String {
    format!(
        "[project]\n\
         id = \"testproj\"\n\n\
         [storage.local]\n\
         path = \"./store\"\n\n\
         [engines.criterion]\n\
         command = \"{}\"\n",
        toml_escape(command)
    )
}

/// A configuration with both the Callgrind and Criterion engines; the Callgrind
/// command writes one summary and the Criterion command writes one case, so a run
/// stores one result set per engine.
fn callgrind_and_criterion_config() -> String {
    format!(
        "[project]\n\
         id = \"testproj\"\n\n\
         [storage.local]\n\
         path = \"./store\"\n\n\
         [engines.callgrind]\n\
         command = \"{}\"\n\n\
         [engines.criterion]\n\
         command = \"{}\"\n",
        toml_escape(&mock_command("--summary grp=single")),
        toml_escape(&mock_command("--criterion grp|capture|now=12.5"))
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
