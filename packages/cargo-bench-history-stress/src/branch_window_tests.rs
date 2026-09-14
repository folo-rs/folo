//! Real-adapter coverage of the production branch-window boundary.
//!
//! The CLI smoke suite covers platform-label combinations. This fixture keeps
//! every engine and timeline family without repeating the capped statistical work
//! for equivalent labels, retaining the selected partitions' original seed roles.

use std::fs;

use cbh_detect::MAX_BRANCH_BASE_COMMITS;
use cbh_model::Engine;
use jiff::Timestamp;
use tempfile::TempDir;

use crate::cli::ModeArg;
use crate::logging::Logger;
use crate::measure::{self, StorageInputs};
use crate::scenario::{self, Scenario};
use crate::target::StorageTarget;
use crate::{repo, seed};

#[tokio::test]
#[cfg_attr(miri, ignore = "builds real Git history and reads filesystem storage")]
async fn handles_more_base_evidence_than_the_window_cap() {
    // Cover every timeline family, including the stable negative control.
    const BENCHMARKS: usize = 5;
    // The branch needs a clean history and a dirty tip, not a long feature history.
    const BRANCH_COMMITS: usize = 2;
    const DIRTY_RUNS: usize = 1;
    // Reuse the CLI repeatability seed; findings depend on shape, not absolute values.
    const SEED: u64 = 424242;
    // A fixed history anchor makes the fixture independent of the wall clock.
    const ANCHOR_UNIX: i64 = 1_750_000_000;
    // All engines support this synthetic label, including Linux-only Callgrind.
    // Platform labels only partition storage; repeating them adds identical expensive
    // regime searches, while CLI smoke tests already cover the complete label matrix.
    const TARGET: &str = "x86_64-unknown-linux-gnu";

    let scenario = Scenario {
        benchmarks: BENCHMARKS,
        commits: MAX_BRANCH_BASE_COMMITS * 2,
        branch_commits: BRANCH_COMMITS,
        dirty_runs: DIRTY_RUNS,
        seed: SEED,
    };
    let with_runs = scenario.commits_with_runs();
    assert!(with_runs > MAX_BRANCH_BASE_COMMITS);
    let sets = scenario::discriminant_sets();
    let selected_sets: Vec<_> = sets
        .iter()
        .enumerate()
        .filter_map(|(index, set)| (set.target_triple.as_str() == TARGET).then_some(index))
        .collect();
    assert_eq!(
        selected_sets
            .iter()
            .map(|&index| sets.get(index).unwrap().engine)
            .collect::<Vec<_>>(),
        Engine::ALL
    );

    let repo_dir = TempDir::new().unwrap();
    let workspace_dir = TempDir::new().unwrap();
    let target = StorageTarget::local(None).unwrap();
    let anchor = Timestamp::from_second(ANCHOR_UNIX).unwrap();
    let (main_times, feature_times) =
        scenario::commit_times(anchor, scenario.commits, scenario.branch_commits);
    // Put every seeded clean/dirty observation before the analysis clock.
    let clock = Timestamp::from_second(feature_times.last().unwrap().as_second() + 1).unwrap();
    let logger = Logger::new(true);
    let repo = repo::build_repo(
        repo_dir.path(),
        &workspace_dir.path().join("marks"),
        &main_times,
        &feature_times,
        logger,
    )
    .await
    .unwrap();
    let config_path = measure::config_path(workspace_dir.path());
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(config_path, target.config_toml()).unwrap();
    let seeded =
        seed::seed_selected_sets(target.seed_root(), scenario, &sets, &selected_sets, &repo)
            .unwrap();
    let expected_runs = (with_runs + BRANCH_COMMITS + DIRTY_RUNS) * selected_sets.len();
    let blessings = selected_sets
        .iter()
        .filter(|&&index| scenario::is_blessed_set(index))
        .count();
    assert_eq!(seeded.objects, expected_runs + blessings);
    assert_eq!(seeded.series, BENCHMARKS * selected_sets.len());

    let measured = measure::measure(
        workspace_dir.path(),
        repo_dir.path(),
        ModeArg::Branch,
        StorageInputs {
            local: target.local_path(),
            cache: None,
        },
        clock,
        1,
        logger,
    )
    .await
    .unwrap();

    assert_eq!(measured.runs, expected_runs);
    assert_eq!(measured.series, BENCHMARKS * selected_sets.len());
    // The drifting and blessable families are elevated on the feature branch.
    assert_eq!(measured.regressions, 2 * selected_sets.len());
    assert_eq!(measured.improvements, Some(0));
    assert!(measured.notable);
}
