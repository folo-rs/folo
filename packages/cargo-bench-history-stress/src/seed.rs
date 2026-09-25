//! Generates the synthetic blob tree and writes it under a storage root.
//!
//! For every discriminant set and commit the harness builds a [`Run`] (or a
//! blessing sidecar) and writes it to `<root>/<storage-key>` — the exact layout
//! the storage backends use, so the tree is the local store directly and the
//! upload source for Azure. Generation is CPU-bound (millions of small records
//! serialized to JSON), so it is fanned out across the available cores.

use std::num::NonZero;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::{fs, thread};

use cbh_codec as codec;
use cbh_model::{
    BenchmarkIdPrefix, BenchmarkResult, BlessingRecord, DiscriminantSet, EnvironmentInfo, GitInfo,
    Run, RunContext, ToolchainInfo,
};
use jiff::Timestamp;
use many_cpus::SystemHardware;

use crate::error::{Error, fail};
use crate::logging::Logger;
use crate::repo::SeededRepo;
use crate::scenario::{
    self, BRANCH_FEATURE, BRANCH_MAIN, PROJECT, Scenario, TOOL_VERSION, benchmark_id,
};

/// Summary of what a seeding pass wrote, for the final report.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SeedStats {
    /// Number of stored objects written (clean + dirty + blessing).
    pub(crate) objects: usize,
    /// Total bytes written across all objects.
    pub(crate) bytes: u64,
    /// Number of distinct series the dataset defines (benchmarks × sets).
    pub(crate) series: usize,
}

/// One file to generate and write: a storage key plus the run it should hold.
#[derive(Debug)]
enum Task {
    /// A clean run on a `main` commit at first-parent index `index`.
    CleanMain {
        /// Index into [`discriminant_sets`](scenario::discriminant_sets).
        set: usize,
        /// First-parent index of the commit on `main`.
        index: usize,
        /// The commit ID.
        commit_id: String,
        /// The committer date.
        time: Timestamp,
    },
    /// A clean run on a feature-branch commit.
    CleanFeature {
        /// Index into the discriminant-set matrix.
        set: usize,
        /// The commit ID.
        commit_id: String,
        /// The committer date.
        time: Timestamp,
    },
    /// A dirty (uncommitted-tree) snapshot on the feature tip.
    Dirty {
        /// Index into the discriminant-set matrix.
        set: usize,
        /// Which dirty snapshot this is (distinguishes the observation time).
        k: usize,
        /// The feature-tip commit ID the snapshot is based on.
        commit_id: String,
        /// The feature-tip committer date.
        time: Timestamp,
        /// The snapshot's observation second (part of its key).
        observation: i64,
    },
    /// A blessing sidecar re-baselining the blessable family in a blessed set.
    Bless {
        /// Index into the discriminant-set matrix.
        set: usize,
        /// The blessed commit ID.
        commit_id: String,
        /// The blessing's issued second (part of its key).
        issued: i64,
    },
}

/// Seeds the full dataset under `root`, returning what was written.
///
/// # Errors
///
/// Returns an error if generation or any file write fails.
pub(crate) fn seed(
    root: &Path,
    scenario: Scenario,
    sets: &[DiscriminantSet],
    repo: &SeededRepo,
    logger: Logger,
) -> Result<SeedStats, Error> {
    let tasks = plan_tasks(scenario, sets, repo);
    let series = scenario.benchmarks.saturating_mul(sets.len());
    logger.step(&format!(
        "generating {} stored objects across {} series",
        tasks.len(),
        series
    ));
    logger.detail_with(|| {
        "each clean object holds every benchmark's primary metric at one commit; objects fan \
         out across CPU cores because serialization, not I/O, dominates generation"
            .to_owned()
    });

    let workers = NonZero::new(SystemHardware::current().processors().len())
        .expect("a processor set is never empty");
    let bytes = write_tasks(root, scenario, sets, &tasks, workers, &write_file)?;
    let stats = SeedStats {
        objects: tasks.len(),
        bytes,
        series,
    };
    logger.detail_with(|| {
        format!(
            "wrote {} bytes across {} objects",
            stats.bytes, stats.objects
        )
    });
    Ok(stats)
}

/// Enumerates every object the dataset should contain.
fn plan_tasks(scenario: Scenario, sets: &[DiscriminantSet], repo: &SeededRepo) -> Vec<Task> {
    let mut tasks = Vec::new();
    let bless_index = scenario.bless_index();
    let feature_tip = repo.feature.last();

    for set in 0..sets.len() {
        for (index, commit) in repo.main.iter().enumerate() {
            if !scenario.commit_has_run(index) {
                continue;
            }
            tasks.push(Task::CleanMain {
                set,
                index,
                commit_id: commit.commit_id.clone(),
                time: commit.time,
            });
        }
        for commit in &repo.feature {
            tasks.push(Task::CleanFeature {
                set,
                commit_id: commit.commit_id.clone(),
                time: commit.time,
            });
        }
        if let Some(tip) = feature_tip {
            for k in 0..scenario.dirty_runs {
                let observation = tip.time.as_second().saturating_add(i64_from(k));
                tasks.push(Task::Dirty {
                    set,
                    k,
                    commit_id: tip.commit_id.clone(),
                    time: tip.time,
                    observation,
                });
            }
        }
        if scenario::is_blessed_set(set)
            && let Some(commit) = repo.main.get(bless_index)
        {
            tasks.push(Task::Bless {
                set,
                commit_id: commit.commit_id.clone(),
                issued: commit.time.as_second().saturating_add(60),
            });
        }
    }

    tasks
}

/// Writes every planned task using the supplied worker limit and storage operation.
///
/// The process boundary supplies OS parallelism and filesystem writes; unit tests
/// supply in-memory storage. See docs/implementation.md, "Validation boundaries".
fn write_tasks(
    root: &Path,
    scenario: Scenario,
    sets: &[DiscriminantSet],
    tasks: &[Task],
    workers: NonZero<usize>,
    write: &(impl Fn(&Path, &[u8]) -> Result<(), Error> + Sync),
) -> Result<u64, Error> {
    let next = AtomicUsize::new(0);
    let total_bytes = AtomicU64::new(0);
    let worker_count = workers.get().min(tasks.len().max(1));

    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let next = &next;
            let total_bytes = &total_bytes;
            handles.push(scope.spawn(move || -> Result<(), Error> {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(task) = tasks.get(index) else {
                        return Ok(());
                    };
                    let written = write_one(root, scenario, sets, task, write)?;
                    total_bytes.fetch_add(written, Ordering::Relaxed);
                }
            }));
        }
        for handle in handles {
            handle
                .join()
                .map_err(|_panic| fail("a seeding worker panicked"))??;
        }
        Ok::<(), Error>(())
    })?;

    Ok(total_bytes.load(Ordering::Relaxed))
}

/// Builds and writes a single object, returning the byte count written.
fn write_one(
    root: &Path,
    scenario: Scenario,
    sets: &[DiscriminantSet],
    task: &Task,
    write: &impl Fn(&Path, &[u8]) -> Result<(), Error>,
) -> Result<u64, Error> {
    let set = sets
        .get(set_index(task))
        .ok_or_else(|| fail("task referenced an out-of-range discriminant set"))?;
    let (key, body) = match task {
        Task::CleanMain {
            set: s,
            index,
            commit_id,
            time,
        } => {
            let run = clean_run(scenario, set, *time, commit_id, BRANCH_MAIN, false, |b| {
                scenario.main_clean_value(b, *s, *index)
            });
            (set.clean_key(PROJECT, commit_id), run.to_json()?)
        }
        Task::CleanFeature {
            set: s,
            commit_id,
            time,
        } => {
            let run = clean_run(
                scenario,
                set,
                *time,
                commit_id,
                BRANCH_FEATURE,
                false,
                |b| scenario.feature_clean_value(b, *s),
            );
            (set.clean_key(PROJECT, commit_id), run.to_json()?)
        }
        Task::Dirty {
            set: s,
            k,
            commit_id,
            time,
            observation,
        } => {
            let run = clean_run(scenario, set, *time, commit_id, BRANCH_FEATURE, true, |b| {
                scenario.dirty_value(b, *s, *k)
            });
            (
                set.dirty_key(PROJECT, commit_id, *observation),
                run.to_json()?,
            )
        }
        Task::Bless {
            commit_id, issued, ..
        } => {
            let prefix = BenchmarkIdPrefix::new(scenario::blessable_family_prefix())
                .map_err(|error| fail(error.to_string()))?;
            let record = BlessingRecord::new(
                commit_id.clone(),
                Timestamp::from_second(*issued)
                    .map_err(|error| fail(format!("invalid blessing time: {error}")))?,
                vec![prefix],
                TOOL_VERSION.to_owned(),
            );
            (
                set.bless_key(PROJECT, commit_id, *issued),
                record.to_json()?,
            )
        }
    };

    // Mirror the storage layer's encoding by calling the same codec it uses, so
    // the seeded tree is byte-for-byte what a real `put` would have written and
    // the reported volume is the real on-disk/wire size #260 is about.
    let stored = codec::compress(body.as_bytes());
    write(&root.join(&key), &stored)?;
    Ok(stored.len() as u64)
}

/// Persists an encoded object and creates its containing directories.
// Real filesystem access belongs to the binary integration tests. The in-process
// tests exercise generation, dispatch, accounting and propagated storage failures.
// Ref: docs/implementation.md, "Validation boundaries".
#[cfg_attr(test, mutants::skip)]
#[cfg_attr(coverage_nightly, coverage(off))]
fn write_file(path: &Path, stored: &[u8]) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| fail(format!("failed to create {}: {error}", parent.display())))?;
    }
    fs::write(path, stored)
        .map_err(|error| fail(format!("failed to write {}: {error}", path.display())))?;
    Ok(())
}

/// Builds a [`Run`] whose every benchmark's primary metric comes from `value`.
///
/// The metric kind (and whether it carries a noise band) follows the set's engine,
/// so each engine's runs are well-formed for the detection path that engine takes.
fn clean_run(
    scenario: Scenario,
    set: &DiscriminantSet,
    time: Timestamp,
    commit_id: &str,
    branch: &str,
    dirty: bool,
    value: impl Fn(usize) -> f64,
) -> Run {
    let engine = set.engine;
    let results: Vec<BenchmarkResult> = (0..scenario.benchmarks)
        .map(|b| {
            BenchmarkResult::new(
                benchmark_id(b),
                vec![scenario::metric_for(engine, value(b))],
            )
        })
        .collect();
    Run::new(run_context(set, time, commit_id, branch, dirty), results)
}

/// Builds the run context shared by every seeded run.
fn run_context(
    set: &DiscriminantSet,
    time: Timestamp,
    commit_id: &str,
    branch: &str,
    dirty: bool,
) -> RunContext {
    let git = GitInfo {
        commit: Some(commit_id.to_owned()),
        branch: Some(branch.to_owned()),
        dirty,
    };
    let toolchain = ToolchainInfo {
        target_triple: set.target_triple.clone(),
        rustc_version: None,
    };
    RunContext::new(
        time,
        git,
        EnvironmentInfo::default(),
        toolchain,
        TOOL_VERSION.to_owned(),
    )
}

/// The discriminant-set index a task targets.
fn set_index(task: &Task) -> usize {
    match task {
        Task::CleanMain { set, .. }
        | Task::CleanFeature { set, .. }
        | Task::Dirty { set, .. }
        | Task::Bless { set, .. } => *set,
    }
}

/// Converts a small index to `i64` for committer-date arithmetic.
fn i64_from(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod write_tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::slice;
    use std::sync::Mutex;

    use cbh_model::{Engine, MachineKey, TargetTriple};

    use super::*;

    // Native runs cover every family; Miri needs one metric to verify each stored-object format.
    const SCENARIO: Scenario = Scenario {
        benchmarks: if cfg!(miri) { 1 } else { 5 },
        commits: 4,
        branch_commits: 1,
        dirty_runs: 2,
        // Arbitrary fixed seed: contents must be reproducible, not statistically representative.
        seed: 37,
    };

    // Distinct fixed timestamps and IDs expose routing mistakes without a real clock or Git.
    const MAIN_TIME: i64 = 1_000;
    const FEATURE_TIME: i64 = 2_000;
    const OBSERVATION: i64 = 2_001;
    const ISSUED: i64 = 1_060;
    const MAIN_COMMIT: &str = "main-commit";
    const FEATURE_COMMIT: &str = "feature-commit";

    // Exercise shared accounting across workers without querying OS parallelism.
    const WORKERS: NonZero<usize> = NonZero::new(2).unwrap();

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
        // Use the model and scenario, not the writer's clean_run/run_context.
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

    fn expected_objects(sets: &[DiscriminantSet; 2]) -> [(String, String); 4] {
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
                expected_blessing(ISSUED),
            ),
        ]
    }

    fn expected_blessing(issued: i64) -> String {
        BlessingRecord::new(
            MAIN_COMMIT.to_owned(),
            ts(issued),
            vec![BenchmarkIdPrefix::new(scenario::blessable_family_prefix()).unwrap()],
            TOOL_VERSION.to_owned(),
        )
        .to_json()
        .unwrap()
    }

    fn collect_objects(
        objects: &Mutex<BTreeMap<PathBuf, Vec<u8>>>,
    ) -> impl Fn(&Path, &[u8]) -> Result<(), Error> + '_ {
        move |path, stored| {
            let previous = objects
                .lock()
                .unwrap()
                .insert(path.to_owned(), stored.to_vec());
            assert!(previous.is_none());
            Ok(())
        }
    }

    fn assert_object(stored: &[u8], body: &str) -> u64 {
        // Use the current storage codec as the oracle instead of pinning compressed fixture bytes.
        assert_eq!(stored, codec::compress(body.as_bytes()));
        u64::try_from(stored.len()).unwrap()
    }

    fn assert_one(index: usize) {
        let root = Path::new("storage");
        let objects = Mutex::new(BTreeMap::new());
        let sets = sets();
        let tasks = tasks();
        let task = tasks.get(index).unwrap();
        let expected = expected_objects(&sets);
        let (key, body) = expected.get(index).unwrap();
        let bytes = write_one(root, SCENARIO, &sets, task, &collect_objects(&objects)).unwrap();
        let objects = objects.into_inner().unwrap();
        assert_eq!(
            bytes,
            assert_object(objects.get(&root.join(key)).unwrap(), body)
        );
        assert_eq!(objects.len(), 1);
    }

    #[test]
    fn write_one_stores_clean_main_content_and_byte_count() {
        assert_one(0);
    }

    #[test]
    fn write_one_stores_clean_feature_content_and_byte_count() {
        assert_one(1);
    }

    #[test]
    fn write_one_stores_dirty_content_and_byte_count() {
        assert_one(2);
    }

    #[test]
    fn write_one_stores_blessing_content_and_byte_count() {
        assert_one(3);
    }

    #[test]
    fn write_tasks_stores_every_object_and_sums_compressed_bytes() {
        let root = Path::new("storage");
        let objects = Mutex::new(BTreeMap::new());
        let sets = sets();
        let (tasks, expected): (Vec<_>, Vec<_>) = if cfg!(miri) {
            // Compact sidecars exercise shared accounting without repeating the full format
            // matrix's compression workload. Distinct issue times give them distinct keys.
            [ISSUED, ISSUED + 1]
                .map(|issued| {
                    (
                        Task::Bless {
                            set: 0,
                            commit_id: MAIN_COMMIT.to_owned(),
                            issued,
                        },
                        (
                            sets[0].bless_key(PROJECT, MAIN_COMMIT, issued),
                            expected_blessing(issued),
                        ),
                    )
                })
                .into_iter()
                .unzip()
        } else {
            (tasks().into(), expected_objects(&sets).into())
        };
        let bytes = write_tasks(
            root,
            SCENARIO,
            &sets,
            &tasks,
            WORKERS,
            &collect_objects(&objects),
        )
        .unwrap();
        let objects = objects.into_inner().unwrap();
        assert_eq!(objects.len(), tasks.len());
        let expected_bytes: u64 = expected
            .iter()
            .map(|(key, body)| assert_object(objects.get(&root.join(key)).unwrap(), body))
            .sum();
        assert_eq!(bytes, expected_bytes);
    }

    #[test]
    fn write_tasks_without_tasks_writes_nothing() {
        let bytes = write_tasks(
            Path::new("unused"),
            SCENARIO,
            &[],
            &[],
            WORKERS,
            &|_, _| unreachable!(),
        )
        .unwrap();
        assert_eq!(bytes, 0);
    }

    #[test]
    fn write_one_rejects_an_unknown_set() {
        _ = write_one(
            Path::new("unused"),
            SCENARIO,
            &[],
            &tasks()[0],
            &|_, _| unreachable!(),
        )
        .unwrap_err();
    }

    #[test]
    fn write_one_rejects_an_invalid_blessing_timestamp() {
        let task = Task::Bless {
            set: 0,
            commit_id: MAIN_COMMIT.to_owned(),
            issued: i64::MAX,
        };
        _ = write_one(
            Path::new("unused"),
            SCENARIO,
            &sets(),
            &task,
            &|_, _| unreachable!(),
        )
        .unwrap_err();
    }

    #[test]
    fn write_tasks_propagates_a_worker_error() {
        _ = write_tasks(
            Path::new("unused"),
            SCENARIO,
            &[],
            &tasks(),
            WORKERS,
            &|_, _| unreachable!(),
        )
        .unwrap_err();
    }

    #[test]
    fn write_one_propagates_a_storage_error() {
        let calls = AtomicUsize::new(0);
        _ = write_one(
            Path::new("storage"),
            SCENARIO,
            &sets(),
            &tasks()[0],
            &|_, _| {
                calls.fetch_add(1, Ordering::Relaxed);
                Err(fail("injected storage failure"))
            },
        )
        .unwrap_err();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn write_tasks_propagates_a_storage_error() {
        let calls = AtomicUsize::new(0);
        let tasks = tasks();
        // A compact sidecar suffices: this case checks failure propagation, not object formats.
        let tasks = slice::from_ref(tasks.last().unwrap());
        _ = write_tasks(
            Path::new("storage"),
            SCENARIO,
            &sets(),
            tasks,
            WORKERS,
            &|_, _| {
                calls.fetch_add(1, Ordering::Relaxed);
                Err(fail("injected storage failure"))
            },
        )
        .unwrap_err();
        assert_ne!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn write_tasks_returns_an_error_when_a_worker_panics() {
        _ = write_tasks(
            Path::new("storage"),
            SCENARIO,
            &sets(),
            &tasks()[..1],
            NonZero::<usize>::MIN,
            &|_, _| panic!(),
        )
        .unwrap_err();
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use cbh_model::{DiscriminantSet, Engine, MachineKey, TargetTriple};
    use jiff::Timestamp;

    use super::*;
    use crate::repo::Commit;

    fn ts(second: i64) -> Timestamp {
        Timestamp::from_second(second).expect("test second is in range")
    }

    /// A repository with `main` first-parent commits and no feature branch, so the
    /// planned tasks reduce to the `main` commits that store a run.
    fn main_only_repo(commits: usize) -> SeededRepo {
        SeededRepo {
            main: (0..commits)
                .map(|i| Commit {
                    commit_id: format!("main{i:02}"),
                    time: ts(1_000_i64.saturating_add(i64_from(i))),
                })
                .collect(),
            feature: Vec::new(),
        }
    }

    #[test]
    fn plan_tasks_only_seeds_commits_that_store_a_run() {
        // A single discriminant set keeps the per-set fan-out at one, so the
        // CleanMain tasks map one-to-one onto the main commits that have a run.
        let sets = vec![DiscriminantSet::new(
            Engine::Callgrind,
            &TargetTriple::from("x86_64-unknown-linux-gnu"),
            &MachineKey::from("stress-rig"),
        )];
        let repo = main_only_repo(4);
        let scenario = Scenario {
            benchmarks: 4,
            commits: 4,
            branch_commits: 0,
            dirty_runs: 1,
            seed: 1,
        };

        let mut indices: Vec<usize> = plan_tasks(scenario, &sets, &repo)
            .into_iter()
            .filter_map(|task| match task {
                Task::CleanMain { index, .. } => Some(index),
                _ => None,
            })
            .collect();
        indices.sort_unstable();

        // commits == 4 -> bless_index 2; runs land on the even indices {0, 2} plus
        // the forced tip (3) and blessed (2) commits, leaving index 1 a gap.
        assert_eq!(indices, vec![0, 2, 3]);
    }

    #[test]
    fn i64_from_preserves_small_values() {
        assert_eq!(i64_from(0), 0);
        assert_eq!(i64_from(7), 7);
    }

    #[test]
    fn set_index_returns_the_targeted_discriminant_set() {
        // A value distinct from both 0 and 1 detects a constant-return mutation
        // across every task variant.
        assert_eq!(
            set_index(&Task::CleanMain {
                set: 7,
                index: 0,
                commit_id: "a".to_owned(),
                time: ts(1),
            }),
            7
        );
        assert_eq!(
            set_index(&Task::CleanFeature {
                set: 4,
                commit_id: "b".to_owned(),
                time: ts(2),
            }),
            4
        );
        assert_eq!(
            set_index(&Task::Dirty {
                set: 5,
                k: 0,
                commit_id: "c".to_owned(),
                time: ts(3),
                observation: 10,
            }),
            5
        );
        assert_eq!(
            set_index(&Task::Bless {
                set: 9,
                commit_id: "d".to_owned(),
                issued: 11,
            }),
            9
        );
    }
}
