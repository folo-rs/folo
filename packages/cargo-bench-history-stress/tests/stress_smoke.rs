//! End-to-end smoke tests for the stress harness binary.
//!
//! Each test runs the compiled binary against local-filesystem storage at a small
//! scenario, so the full pipeline (git fast-import, write the synthetic object
//! tree, then analyze every requested mode through the real public entry point) is
//! exercised quickly. They spawn real processes (the harness and `git`), so they
//! are `#[cfg_attr(miri, ignore)]`.
//!
//! The scenario is sized from the detectors' evidence gates rather than picked for
//! speed alone, and the assertions pin the exact set of findings the synthetic
//! dataset is built to produce. Both matter: a scenario below the gates, or a
//! defect that stopped the pipeline short of detection, would report zero findings
//! while every "the binary ran" check still passed. Pinning the ground truth makes
//! this suite a liveness canary for the whole harness.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "expected counts composed from the fixed, tiny scenario sizes cannot overflow"
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use cbh_detect::AnalysisConfig;

/// Benchmark cases the scenario seeds per discriminant set: one per timeline-shape
/// family, so every seeded shape — including the stable one, which must raise
/// nothing — is represented exactly once in every set.
const BENCHMARKS: usize = 5;

/// Points each side of a seeded step must hold for the detectors to trust it: the
/// production `min_regime` gate.
const REGIME_POINTS: usize = 5;

/// First-parent `main` commits the scenario seeds.
///
/// Roughly half of them carry a stored run, so this has to be at least twice the
/// detectors' minimum series length before anything is judged at all, and
/// comfortably more than that before every seeded step keeps a whole regime behind
/// it. Eight regimes' worth of history clears both with margin.
const COMMITS: usize = 8 * REGIME_POINTS;

/// Feature-branch commits the scenario seeds. Branch mode judges only the tip
/// commit's state, so the branch itself stays short.
const BRANCH_COMMITS: usize = 2;

/// Dirty (uncommitted-tree) snapshots the scenario seeds on the feature tip.
const DIRTY_RUNS: usize = 1;

/// Discriminant sets the scenario seeds: every supported engine crossed with the
/// target triples it can run on.
const DISCRIMINANT_SETS: usize = 20;

/// Discriminant sets whose blessing re-baselines the blessable family, so that
/// family's seeded step raises no finding there.
const BLESSED_SETS: usize = 3;

/// Series every mode compares: one per benchmark per discriminant set.
const SERIES: usize = BENCHMARKS * DISCRIMINANT_SETS;

/// History-mode regressions the seeded shapes produce: the drifting family and the
/// mid-history step family in every set, plus the blessable step family in every
/// set no blessing re-baselined.
const HISTORY_REGRESSIONS: usize = 2 * DISCRIMINANT_SETS + (DISCRIMINANT_SETS - BLESSED_SETS);

/// History-mode improvements the seeded shapes produce: the step-down family, in
/// every set.
const HISTORY_IMPROVEMENTS: usize = DISCRIMINANT_SETS;

/// Branch-mode regressions the seeded shapes produce: the two seeded benchmarks the
/// feature branch elevates, in every set.
const BRANCH_REGRESSIONS: usize = 2 * DISCRIMINANT_SETS;

/// Branch-mode improvements: the feature branch only ever raises values, so there
/// are none.
const BRANCH_IMPROVEMENTS: usize = 0;

/// The mode keywords the report can carry, in the order a [`BTreeMap`] holds them.
const MODES: [&str; 2] = ["branch", "history"];

/// The deterministic columns of one mode's row in the report table.
///
/// The timing columns (duration, cpu, cpu%) are deliberately absent: they measure
/// real elapsed time and so reproduce nowhere, while these counts are a pure
/// function of the seed and the scenario sizes.
#[derive(Debug, Eq, PartialEq)]
struct ModeRow {
    /// Stored objects the mode loaded.
    objects: usize,
    /// Distinct series the mode compared.
    series: usize,
    /// Regressions the mode flagged.
    regressions: usize,
    /// Improvements the mode flagged.
    improvements: usize,
    /// Whether any finding survived.
    notable: bool,
}

/// Runs the stress binary with the given arguments plus the default scenario,
/// returning the captured output.
fn run_stress(extra: &[&str]) -> Output {
    let benchmarks = BENCHMARKS.to_string();
    let commits = COMMITS.to_string();
    let branch_commits = BRANCH_COMMITS.to_string();
    let dirty_runs = DIRTY_RUNS.to_string();

    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-bench-history-stress"));
    command.args([
        "--storage",
        "local",
        "--benchmarks",
        benchmarks.as_str(),
        "--commits",
        commits.as_str(),
        "--branch-commits",
        branch_commits.as_str(),
        "--dirty-runs",
        dirty_runs.as_str(),
    ]);
    command.args(extra);
    command
        .output()
        .expect("the stress binary should be runnable")
}

/// Runs the stress binary and returns its stdout, failing the test if it did not
/// exit cleanly.
fn successful_stress(extra: &[&str]) -> String {
    let output = run_stress(extra);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Extracts the per-mode report rows, keyed by mode.
fn mode_rows(stdout: &str) -> BTreeMap<String, ModeRow> {
    stdout.lines().filter_map(parse_mode_row).collect()
}

/// Parses one row of the per-mode report table, or `None` when `line` is anything
/// else (a summary line, a table header, or a rule).
fn parse_mode_row(line: &str) -> Option<(String, ModeRow)> {
    let columns: Vec<&str> = line.split_whitespace().collect();
    let [
        mode,
        _duration,
        _cpu,
        _cpu_percent,
        objects,
        series,
        regressions,
        improvements,
        notable,
    ] = columns.as_slice()
    else {
        return None;
    };
    if !MODES.contains(mode) {
        return None;
    }
    let notable = match *notable {
        "yes" => true,
        "no" => false,
        _ => return None,
    };
    Some((
        (*mode).to_owned(),
        ModeRow {
            objects: objects.parse().ok()?,
            series: series.parse().ok()?,
            regressions: regressions.parse().ok()?,
            improvements: improvements.parse().ok()?,
            notable,
        },
    ))
}

/// A count from the report's summary block, named by the `label` that introduces it
/// (including the trailing colon).
fn summary_count(stdout: &str, label: &str) -> usize {
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix(label))
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or_else(|| panic!("the summary states `{label}`: {stdout}"))
}

/// The number of files anywhere beneath `dir`.
fn stored_files(dir: &Path) -> usize {
    fs::read_dir(dir)
        .expect("the seeded partition is a readable directory")
        .map(|entry| {
            let path = entry.expect("a seeded directory entry is readable").path();
            if path.is_dir() {
                stored_files(&path)
            } else {
                1
            }
        })
        .sum()
}

/// The row the seeded dataset must produce for `mode`, given the `main` commits
/// that carry a run.
///
/// Every mode loads one object per run-carrying `main` commit in every discriminant
/// set; branch mode additionally loads the feature branch's commits and the dirty
/// snapshots on its tip.
fn expected_row(mode: &str, with_runs: usize) -> ModeRow {
    match mode {
        "history" => ModeRow {
            objects: with_runs * DISCRIMINANT_SETS,
            series: SERIES,
            regressions: HISTORY_REGRESSIONS,
            improvements: HISTORY_IMPROVEMENTS,
            notable: true,
        },
        "branch" => ModeRow {
            objects: (with_runs + BRANCH_COMMITS + DIRTY_RUNS) * DISCRIMINANT_SETS,
            series: SERIES,
            regressions: BRANCH_REGRESSIONS,
            improvements: BRANCH_IMPROVEMENTS,
            notable: true,
        },
        other => panic!("the report only ever carries the seeded modes, not {other}"),
    }
}

/// Asserts that `stdout` reports exactly `modes`, each carrying exactly the
/// findings the seeded dataset is built to produce, over a history that clears the
/// detectors' evidence gates.
fn assert_seeded_ground_truth(stdout: &str, modes: &[&str]) {
    // Read the harness's own account of how much evidence it seeded, rather than
    // restating the seeding rule here, so the gate assertions are made against what
    // actually landed in storage.
    let with_runs = summary_count(stdout, "with a run:");

    // The sizing rests on roughly half the commits carrying a run; if the seeding
    // rule ever populated a smaller share, the derivation behind `COMMITS` would no
    // longer hold even though the count below might still clear the floor.
    assert!(
        with_runs * 2 >= COMMITS,
        "roughly half the seeded commits must carry a run: {stdout}"
    );

    // Everything else here is downstream of the history being long enough to judge
    // at all: below this floor every detector abstains and every count is a
    // vacuous zero.
    let config = AnalysisConfig::default();
    assert!(
        with_runs >= config.min_series_points,
        "the seeded history must clear the detectors' evidence floor: {stdout}"
    );

    let rows = mode_rows(stdout);
    assert_eq!(
        rows.keys().map(String::as_str).collect::<Vec<_>>(),
        modes,
        "{stdout}"
    );
    for mode in modes {
        let row = rows
            .get(*mode)
            .unwrap_or_else(|| panic!("the {mode} row was just matched: {stdout}"));
        assert_eq!(*row, expected_row(mode, with_runs), "mode {mode}: {stdout}");
    }
}

#[test]
fn fixture_sizes_match_the_analysis_gates() {
    // The scenario sizes above are derived from the gates rather than picked for
    // speed. Bind them to the gates here, so moving a gate fails loudly instead of
    // silently leaving this suite judging a history the detectors abstain on.
    let config = AnalysisConfig::default();
    assert_eq!(
        REGIME_POINTS, config.min_regime,
        "each side of a seeded step must hold a full regime"
    );
    assert!(
        COMMITS >= 4 * config.min_series_points,
        "half the seeded commits carry a run, and the fixture keeps twice the points a \
         judged series needs"
    );
    assert!(
        COMMITS >= 4 * config.drift_min_points,
        "the fixture keeps twice the run-carrying commits the seeded drift needs to be seen"
    );
    assert!(
        config.compare_window >= config.min_series_points,
        "branch mode's base window must be able to hold the levels its test demands"
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn runs_all_modes_and_reports_a_table() {
    let stdout = successful_stress(&[]);
    assert!(
        stdout.contains("cargo-bench-history stress results"),
        "{stdout}"
    );

    // Every mode loaded the objects the replicated key layout put on disk, compared
    // every seeded series, and reached exactly the findings the dataset encodes.
    assert_seeded_ground_truth(&stdout, &MODES);
}

#[test]
#[cfg_attr(miri, ignore)]
fn measures_only_the_requested_modes() {
    let stdout = successful_stress(&["--modes", "history"]);
    assert_seeded_ground_truth(&stdout, &["history"]);
}

#[test]
#[cfg_attr(miri, ignore)]
fn finds_the_seeded_branch_regressions() {
    // The synthetic dataset elevates a subset of benchmarks on the feature branch
    // and nothing else, so branch mode must surface exactly those as regressions
    // and no improvements at all.
    let stdout = successful_stress(&["--modes", "branch"]);
    assert_seeded_ground_truth(&stdout, &["branch"]);
}

#[test]
#[cfg_attr(miri, ignore)]
fn is_deterministic_across_runs() {
    let first = successful_stress(&["--repeat", "2", "--seed", "424242"]);
    let second = successful_stress(&["--repeat", "2", "--seed", "424242"]);

    assert_eq!(
        mode_rows(&first),
        mode_rows(&second),
        "identical seed and sizing must reproduce identical findings"
    );

    // The seeded shapes are relative to each series' own base value, so a different
    // seed moves every value but changes no finding: the same ground truth must
    // hold here as under the default seed. Without this the comparison above would
    // be satisfied by two equally empty reports.
    assert_seeded_ground_truth(&first, &MODES);
}

#[test]
#[cfg_attr(miri, ignore)]
fn keeps_seeded_data_under_the_versioned_prefix_when_asked() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().to_str().expect("temp path is valid UTF-8");

    let stdout = successful_stress(&["--keep", "--dir", path, "--modes", "history"]);

    // Local storage writes the same versioned key layout the backends use, so every
    // object the run reports having seeded must be a file under the project
    // partition — a partition that merely exists would leave the measured analysis
    // reading nothing.
    let project_dir = dir.path().join("v1").join("stress");
    assert_eq!(
        stored_files(&project_dir),
        summary_count(&stdout, "objects seeded:"),
        "expected every seeded object under {}",
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
fn rejects_a_cache_against_local_storage() {
    // The read-through cache only applies to the cloud backend, so pairing it with
    // the default `--storage local` is a usage error rather than a silent no-op.
    let output = run_stress(&["--cache", "cache-dir", "--modes", "history"]);
    assert!(!output.status.success(), "a cache against local must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--cache only applies to --storage azure"),
        "{stderr}"
    );
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
