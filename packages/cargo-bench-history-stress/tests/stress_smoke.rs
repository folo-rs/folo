//! End-to-end smoke tests for the stress harness binary.
//!
//! Each test runs the compiled binary against local-filesystem storage at a tiny
//! scenario size, so the full pipeline (git fast-import, write the synthetic object
//! tree, then analyze every requested mode through the real public entry point) is
//! exercised quickly. They spawn real processes (the harness and `git`), so they are
//! `#[cfg_attr(miri, ignore)]`.

#![allow(
    clippy::indexing_slicing,
    reason = "parsing fixed-shape report tables in test code"
)]

use std::collections::BTreeMap;
use std::process::{Command, Output};

/// Runs the stress binary with the given arguments plus a tiny default scenario,
/// returning the captured output.
fn run_stress(extra: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-bench-history-stress"));
    command.args([
        "--storage",
        "local",
        "--benchmarks",
        "4",
        "--commits",
        "5",
        "--branch-commits",
        "2",
        "--dirty-runs",
        "1",
    ]);
    command.args(extra);
    command
        .output()
        .expect("the stress binary should be runnable")
}

/// Extracts the per-mode report rows, keyed by mode, with the non-deterministic
/// timing columns (duration, cpu, cpu%) dropped so the deterministic columns
/// (objects, series, regressions, improvements, notable) can be compared across
/// runs.
fn deterministic_columns(stdout: &str) -> BTreeMap<String, Vec<String>> {
    stdout
        .lines()
        .filter_map(|line| {
            let columns: Vec<&str> = line.split_whitespace().collect();
            if columns.len() == 9 && ["history", "branch", "tip"].contains(&columns[0]) {
                // Skip columns 1-3 (duration, cpu, cpu%); keep the rest.
                let kept = columns[4..].iter().map(|c| (*c).to_owned()).collect();
                Some((columns[0].to_owned(), kept))
            } else {
                None
            }
        })
        .collect()
}

#[test]
#[cfg_attr(miri, ignore)]
fn runs_all_three_modes_and_reports_a_table() {
    let output = run_stress(&[]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("cargo-bench-history stress results"),
        "{stdout}"
    );

    let rows = deterministic_columns(&stdout);
    assert!(rows.contains_key("history"), "{stdout}");
    assert!(rows.contains_key("branch"), "{stdout}");
    assert!(rows.contains_key("tip"), "{stdout}");

    // Every mode loaded a non-zero number of objects and series, proving the
    // replicated key layout is what analyze actually reads.
    for (mode, columns) in &rows {
        let objects: u64 = columns[0].parse().expect("objects column is numeric");
        let series: u64 = columns[1].parse().expect("series column is numeric");
        assert!(objects > 0, "mode {mode} loaded no objects: {stdout}");
        assert!(series > 0, "mode {mode} compared no series: {stdout}");
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn measures_only_the_requested_modes() {
    let output = run_stress(&["--modes", "history"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let rows = deterministic_columns(&stdout);
    assert_eq!(rows.keys().cloned().collect::<Vec<_>>(), vec!["history"]);
}

#[test]
#[cfg_attr(miri, ignore)]
fn finds_a_branch_regression() {
    // The synthetic dataset is shaped to elevate a subset of benchmarks on the
    // feature branch, so branch mode must surface regressions.
    let output = run_stress(&["--modes", "branch"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let rows = deterministic_columns(&stdout);
    let branch = &rows["branch"];
    let regressions: u64 = branch[2].parse().expect("regressions column is numeric");
    assert!(regressions > 0, "expected branch regressions: {stdout}");
}

#[test]
#[cfg_attr(miri, ignore)]
fn is_deterministic_across_runs() {
    let first = run_stress(&["--repeat", "2", "--seed", "424242"]);
    let second = run_stress(&["--repeat", "2", "--seed", "424242"]);
    assert!(first.status.success() && second.status.success());

    let first_rows = deterministic_columns(&String::from_utf8_lossy(&first.stdout));
    let second_rows = deterministic_columns(&String::from_utf8_lossy(&second.stdout));
    assert_eq!(
        first_rows, second_rows,
        "identical seed and sizing must reproduce identical findings"
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn keeps_seeded_data_under_the_versioned_prefix_when_asked() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().to_str().expect("temp path is valid UTF-8");

    let output = run_stress(&["--keep", "--dir", path, "--modes", "history"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Local storage writes the same versioned key layout the backends use; the
    // project partition must exist on disk after a --keep run.
    let project_dir = dir.path().join("v1").join("stress");
    assert!(
        project_dir.is_dir(),
        "expected seeded data under {}",
        project_dir.display()
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn rejects_an_empty_scenario() {
    let output = run_stress(&["--benchmarks", "0"]);
    assert!(!output.status.success(), "zero benchmarks must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("benchmarks"), "{stderr}");
}

#[test]
#[cfg_attr(miri, ignore)]
fn reports_progress_and_explains_only_under_verbose() {
    // Always-on phase markers go to stderr; explanatory detail lines appear there
    // only when --verbose is set.
    let quiet = run_stress(&["--modes", "history"]);
    assert!(
        quiet.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&quiet.stderr)
    );
    let quiet_err = String::from_utf8_lossy(&quiet.stderr);
    assert!(
        quiet_err.contains("==>"),
        "expected phase markers on stderr: {quiet_err}"
    );
    assert!(
        !quiet_err.contains("local store directory is"),
        "detail lines must stay silent without --verbose: {quiet_err}"
    );

    let verbose = run_stress(&["--modes", "history", "--verbose"]);
    assert!(
        verbose.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&verbose.stderr)
    );
    let verbose_err = String::from_utf8_lossy(&verbose.stderr);
    assert!(
        verbose_err.contains("==>"),
        "expected phase markers on stderr: {verbose_err}"
    );
    assert!(
        verbose_err.contains("local store directory is"),
        "expected explanatory detail under --verbose: {verbose_err}"
    );
}
