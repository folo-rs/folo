use std::fs;

use cbh_model::{Engine, MachineKey, TargetTriple};
use tempfile::tempdir;

use super::*;

// Small enough to test storage without analysis or Git, while covering every benchmark family.
const SCENARIO: Scenario = Scenario {
    benchmarks: 5,
    commits: 4,
    branch_commits: 1,
    dirty_runs: 2,
    // Arbitrary fixed seed: object contents must be reproducible, not statistically representative.
    seed: 37,
};

// Distinct fixed timestamps and IDs expose routing mistakes without consulting a real clock or Git.
const MAIN_TIME: i64 = 1_000;
const FEATURE_TIME: i64 = 2_000;
const OBSERVATION: i64 = 2_001;
const ISSUED: i64 = 1_060;
const MAIN_COMMIT: &str = "main-commit";
const FEATURE_COMMIT: &str = "feature-commit";

fn sets() -> [DiscriminantSet; 2] {
    // Contrast noisy and exact metrics, as well as the zero and nonzero set indices.
    [Engine::Criterion, Engine::Callgrind].map(|engine| {
        DiscriminantSet::new(
            engine,
            &TargetTriple::from("x86_64-unknown-linux-gnu"),
            &MachineKey::from("write-test-rig"),
        )
    })
}

fn tasks() -> [Task; 4] {
    [
        Task::CleanMain {
            set: 1,
            index: SCENARIO.bless_index(),
            commit_id: MAIN_COMMIT.to_owned(),
            time: ts(MAIN_TIME),
        },
        Task::CleanFeature {
            set: 0,
            commit_id: FEATURE_COMMIT.to_owned(),
            time: ts(FEATURE_TIME),
        },
        Task::Dirty {
            set: 1,
            k: 1,
            commit_id: FEATURE_COMMIT.to_owned(),
            time: ts(FEATURE_TIME),
            observation: OBSERVATION,
        },
        Task::Bless {
            set: 0,
            commit_id: MAIN_COMMIT.to_owned(),
            issued: ISSUED,
        },
    ]
}

fn ts(second: i64) -> Timestamp {
    Timestamp::from_second(second).unwrap()
}

fn expected_run(
    set: &DiscriminantSet,
    time: i64,
    commit: &str,
    branch: &str,
    dirty: bool,
    value: impl Fn(usize) -> f64,
) -> String {
    // Build expectations from the model and scenario, not the writer's clean_run/run_context.
    Run::new(
        RunContext::new(
            ts(time),
            GitInfo {
                commit: Some(commit.to_owned()),
                branch: Some(branch.to_owned()),
                dirty,
            },
            EnvironmentInfo::default(),
            ToolchainInfo {
                target_triple: set.target_triple.clone(),
                rustc_version: None,
            },
            TOOL_VERSION.to_owned(),
        ),
        (0..SCENARIO.benchmarks)
            .map(|b| {
                BenchmarkResult::new(
                    benchmark_id(b),
                    vec![scenario::metric_for(set.engine, value(b))],
                )
            })
            .collect(),
    )
    .to_json()
    .unwrap()
}

fn expected_objects(sets: &[DiscriminantSet]) -> [(String, String); 4] {
    [
        (
            sets[1].clean_key(PROJECT, MAIN_COMMIT),
            expected_run(&sets[1], MAIN_TIME, MAIN_COMMIT, BRANCH_MAIN, false, |b| {
                SCENARIO.main_clean_value(b, 1, SCENARIO.bless_index())
            }),
        ),
        (
            sets[0].clean_key(PROJECT, FEATURE_COMMIT),
            expected_run(
                &sets[0],
                FEATURE_TIME,
                FEATURE_COMMIT,
                BRANCH_FEATURE,
                false,
                |b| SCENARIO.feature_clean_value(b, 0),
            ),
        ),
        (
            sets[1].dirty_key(PROJECT, FEATURE_COMMIT, OBSERVATION),
            expected_run(
                &sets[1],
                FEATURE_TIME,
                FEATURE_COMMIT,
                BRANCH_FEATURE,
                true,
                |b| SCENARIO.dirty_value(b, 1, 1),
            ),
        ),
        (
            sets[0].bless_key(PROJECT, MAIN_COMMIT, ISSUED),
            BlessingRecord::new(
                MAIN_COMMIT.to_owned(),
                ts(ISSUED),
                vec![BenchmarkIdPrefix::new(scenario::blessable_family_prefix()).unwrap()],
                TOOL_VERSION.to_owned(),
            )
            .to_json()
            .unwrap(),
        ),
    ]
}

fn assert_object(root: &Path, key: &str, body: &str) -> u64 {
    let stored = fs::read(root.join(key)).unwrap();
    // Checking the encoded bytes rejects plain JSON too, which the tolerant decoder accepts.
    assert_eq!(stored, codec::compress(body.as_bytes()));
    assert_eq!(codec::decompress(&stored).unwrap(), body.as_bytes());
    u64::try_from(stored.len()).unwrap()
}

#[test]
#[cfg_attr(miri, ignore = "writes compressed objects to the real filesystem")]
fn write_one_stores_keyed_content_and_reports_compressed_bytes() {
    let root = tempdir().unwrap();
    let sets = sets();
    for (task, (key, body)) in tasks().iter().zip(expected_objects(&sets)) {
        let bytes = write_one(root.path(), SCENARIO, &sets, task).unwrap();
        assert_eq!(bytes, assert_object(root.path(), &key, &body));
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "uses OS parallelism discovery and real filesystem writes"
)]
fn write_tasks_stores_every_object_and_sums_compressed_bytes() {
    let root = tempdir().unwrap();
    let sets = sets();
    let bytes = write_tasks(root.path(), SCENARIO, &sets, &tasks()).unwrap();
    let expected_bytes: u64 = expected_objects(&sets)
        .iter()
        .map(|(key, body)| assert_object(root.path(), key, body))
        .sum();
    assert_eq!(bytes, expected_bytes);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "uses OS parallelism discovery and real filesystem writes"
)]
fn write_tasks_without_tasks_writes_nothing() {
    let parent = tempdir().unwrap();
    let root = parent.path().join("not-created");
    assert_eq!(write_tasks(&root, SCENARIO, &[], &[]).unwrap(), 0);
    assert!(!root.exists());
}

#[test]
fn write_one_rejects_an_unknown_set() {
    assert!(write_one(Path::new("unused"), SCENARIO, &[], &tasks()[0]).is_err());
}

#[test]
fn write_one_rejects_an_invalid_blessing_timestamp() {
    let task = Task::Bless {
        set: 0,
        commit_id: MAIN_COMMIT.to_owned(),
        issued: i64::MAX,
    };
    assert!(write_one(Path::new("unused"), SCENARIO, &sets(), &task).is_err());
}

#[test]
#[cfg_attr(miri, ignore = "uses OS parallelism discovery to run seeding workers")]
fn write_tasks_propagates_a_worker_error() {
    assert!(write_tasks(Path::new("unused"), SCENARIO, &[], &tasks()).is_err());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "creates a real filesystem obstruction to directory creation"
)]
fn write_one_propagates_directory_creation_failure() {
    let parent = tempdir().unwrap();
    let root = parent.path().join("file");
    fs::write(&root, b"obstruction").unwrap();
    assert!(write_one(&root, SCENARIO, &sets(), &tasks()[0]).is_err());
    assert_eq!(fs::read(&root).unwrap(), b"obstruction");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "creates a real filesystem obstruction to an object write"
)]
fn write_one_propagates_file_write_failure() {
    let root = tempdir().unwrap();
    let sets = sets();
    let path = root.path().join(sets[1].clean_key(PROJECT, MAIN_COMMIT));
    fs::create_dir_all(&path).unwrap();
    assert!(write_one(root.path(), SCENARIO, &sets, &tasks()[0]).is_err());
    assert!(path.is_dir());
}
