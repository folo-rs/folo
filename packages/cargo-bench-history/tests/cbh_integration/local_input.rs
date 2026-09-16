use std::collections::BTreeMap;
use std::fs;

use cbh_model::Engine;
use serde_json::Value;

use crate::harness::*;

#[tokio::test]
#[cfg_attr(miri, ignore = "uses real git history and filesystem stores")]
async fn queries_combine_local_input_with_baseline_without_modifying_either() {
    // A flat base at the detector's evidence floor and a doubled tip give a
    // deterministic branch finding without a larger history or noisy measurements.
    const BASELINE: f64 = 100.0;
    const TIP: f64 = 200.0;
    // A distinct stored value makes input precedence observable in examine, even
    // though one stale point would not change the base's median.
    const STALE: f64 = 50.0;

    let baseline = Workspace::repo(&storage_only_config());
    let input = Workspace::new(&storage_only_config());
    let mut commits = Vec::new();
    for (index, date) in sequential_dates("2024-01-01", MIN_SERIES_POINTS)
        .into_iter()
        .enumerate()
    {
        let label = format!("base-{index}");
        commits.push(baseline.commit_dated(&date, &label));
        let value = if index + 1 == MIN_SERIES_POINTS {
            STALE
        } else {
            BASELINE
        };
        baseline.seed_callgrind(&label, value);
    }
    baseline.checkout_new_branch("feature");
    let tip = baseline.commit_dated("2024-02-01", "tip");
    assert_eq!(baseline.head_commit_id(), tip);

    // Seed ordinary stored objects: one corrected base key and a tip that exists
    // only in the input. The baseline-only keys must still be read from the baseline.
    for (commit, value) in [(commits.last().unwrap(), BASELINE), (&tip, TIP)] {
        let key = seed_clean_key(
            Engine::Callgrind,
            HARNESS_AUTO_TRIPLE,
            HARNESS_AUTO_MACHINE_KEY,
            commit,
        );
        input.seed(
            &key,
            &ir_result_set(analysis_now().as_second(), commit, value),
        );
    }
    commits.push(tip.clone());

    let baseline_store = baseline.root().join("store");
    let input_store = input.root().join("store");
    let baseline_before = snapshot(&baseline_store);
    let input_before = snapshot(&input_store);
    let baseline_selection = format!("--local={}", baseline_store.display());
    let repo = baseline.root().to_str().unwrap();
    // Both workspaces have a store directory. Resolving this relative input against
    // --repo would silently select the baseline again instead of the local tip.
    let selection = [
        baseline_selection.as_str(),
        "--local-input",
        "store",
        "--repo",
        repo,
        "--base",
        "master",
    ];

    let args = [&["analyze"][..], &selection].concat();
    let report = input.drive_json(&args).await;
    let report: Value = serde_json::from_str(&report).unwrap();
    assert_eq!(report["mode"], "branch");
    assert_eq!(report["tip_commit"], tip);
    assert_eq!(report["census"]["judged"], 1);
    assert_eq!(report["regressions"], 1);
    let findings = report["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["kind"], "instruction_count");
    assert_eq!(findings[0]["direction"], "regression");
    assert_eq!(findings[0]["baseline"], BASELINE);
    assert_eq!(findings[0]["latest"], TIP);
    assert_eq!(snapshot(&baseline_store), baseline_before);
    assert_eq!(snapshot(&input_store), input_before);

    let args = [&["list", "runs"][..], &selection].concat();
    let report = input.drive_json(&args).await;
    let report: Value = serde_json::from_str(&report).unwrap();
    assert_eq!(report["totals"]["runs"], commits.len());
    assert_eq!(report["totals"]["series"], 1);
    assert_eq!(report["totals"]["discriminant_sets"], 1);
    let listed = report["sets"][0]["commits"].as_array().unwrap();
    let listed_commits: Vec<&str> = listed
        .iter()
        .map(|commit| commit["commit"].as_str().unwrap())
        .collect();
    assert_eq!(listed_commits, commits);
    assert!(listed.iter().all(|commit| commit["runs"] == 1));
    assert_eq!(snapshot(&baseline_store), baseline_before);
    assert_eq!(snapshot(&input_store), input_before);

    let args = [
        &[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
        ][..],
        &selection,
    ]
    .concat();
    let report = input.drive_json(&args).await;
    let report: Value = serde_json::from_str(&report).unwrap();
    let sets = report["sets"].as_array().unwrap();
    assert_eq!(sets.len(), 1);
    let points = sets[0]["points"].as_array().unwrap();
    let examined_commits: Vec<&str> = points
        .iter()
        .map(|point| point["commit"].as_str().unwrap())
        .collect();
    assert_eq!(examined_commits, commits);
    let values: Vec<f64> = points
        .iter()
        .map(|point| point["value"].as_f64().unwrap())
        .collect();
    let expected: Vec<f64> = std::iter::repeat_n(BASELINE, MIN_SERIES_POINTS)
        .chain([TIP])
        .collect();
    assert_eq!(values, expected);
    assert_eq!(snapshot(&baseline_store), baseline_before);
    assert_eq!(snapshot(&input_store), input_before);
}

#[tokio::test]
#[cfg_attr(miri, ignore = "uses real filesystem stores")]
async fn missing_local_input_is_a_hard_error_and_is_not_created() {
    let workspace = Workspace::new(&storage_only_config());
    let baseline_store = workspace.root().join("store");
    fs::create_dir_all(&baseline_store).unwrap();
    let before = snapshot(&baseline_store);
    let missing = workspace.root().join("missing-input");

    workspace
        .drive(&[
            "list",
            "discriminants",
            "--local-input",
            missing.to_str().unwrap(),
        ])
        .await
        .unwrap_err();

    assert!(!missing.exists());
    assert_eq!(snapshot(&baseline_store), before);
}

/// Records directory entries and raw file bytes, including non-object files.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut entries = BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let key = path.strip_prefix(root).unwrap().to_path_buf();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                entries.insert(key, None);
                directories.push(path);
            } else {
                assert!(kind.is_file());
                entries.insert(key, Some(fs::read(path).unwrap()));
            }
        }
    }
    entries
}
