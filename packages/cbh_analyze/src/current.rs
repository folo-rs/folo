//! Exact current observations and the historical roster they authorize.

use std::collections::BTreeSet;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::{Path, PathBuf};

use cbh_detect::{RunPoints, Series};
use cbh_model::{
    BenchmarkId, CollectionSnapshot, DiscriminantSet, MetricKind, Run, StorageKey, sanitize_segment,
};

use crate::AnalyzeError;
use crate::load::{WorkerFold, ordinal_of};

/// Selected executions supply current values; their identities bound historical series.
pub(crate) struct CurrentCollections {
    commit: String,
    sets: BTreeSet<DiscriminantSet>,
    roster: BTreeSet<(DiscriminantSet, BenchmarkId, MetricKind)>,
    runs: Vec<(DiscriminantSet, Run)>,
}

impl CurrentCollections {
    pub(crate) fn new(
        project: &str,
        snapshots: &[CollectionSnapshot],
    ) -> Result<Self, AnalyzeError> {
        let first = snapshots
            .first()
            .ok_or_else(|| InvalidCurrentCollection::new("no collection snapshots supplied"))?;
        let mut selected = Self {
            commit: first.commit().to_owned(),
            sets: BTreeSet::new(),
            roster: BTreeSet::new(),
            runs: Vec::new(),
        };
        for snapshot in snapshots {
            if snapshot.project() != sanitize_segment(project)
                || snapshot.commit() != selected.commit
            {
                return Err(InvalidCurrentCollection::new("project or commit mismatch").into());
            }
            for (set, run) in snapshot.runs() {
                selected.sets.insert(set.clone());
                for result in &run.results {
                    for metric in &result.metrics {
                        if !selected
                            .roster
                            .insert((set.clone(), result.id.clone(), metric.kind))
                        {
                            return Err(InvalidCurrentCollection::new(
                                "selected executions contain duplicate current measurement identities",
                            ).into());
                        }
                    }
                }
                selected.runs.push((set, run));
            }
        }
        if selected.roster.is_empty() {
            return Err(InvalidCurrentCollection::new(
                "selected successful collections contain no measurements to analyze",
            )
            .into());
        }
        Ok(selected)
    }

    pub(crate) fn validate_context(&self, commit: &str, dirty: bool) -> Result<(), AnalyzeError> {
        if dirty || self.commit != commit {
            return Err(InvalidCurrentCollection::new(
                "snapshots must match the clean analysis context",
            )
            .into());
        }
        Ok(())
    }

    /// Shared storage may supply older observations, never this context's measurements.
    fn admits_history(&self, key: &StorageKey) -> bool {
        self.sets.contains(&key.set) && (key.is_bless() || key.commit != self.commit)
    }

    /// Other machines remain eligible only as evidence explaining baseline availability.
    fn admits_sibling(&self, key: &StorageKey) -> bool {
        key.commit != self.commit
            && !self.sets.contains(&key.set)
            && self.sets.iter().any(|set| {
                set.engine == key.set.engine
                    && set.target_triple == key.set.target_triple
                    && set.machine_key != key.set.machine_key
            })
    }

    pub(crate) fn retain_history(
        &self,
        candidates: &mut Vec<(String, StorageKey)>,
        blessings: &mut Vec<(String, StorageKey)>,
        siblings: &mut Vec<(String, StorageKey)>,
    ) {
        siblings.extend(
            candidates
                .iter()
                .filter(|(_, key)| self.admits_sibling(key))
                .cloned(),
        );
        candidates.retain(|(_, key)| self.admits_history(key));
        blessings.retain(|(_, key)| self.admits_history(key));
    }

    pub(crate) fn contains(&self, series: &Series) -> bool {
        self.roster
            .contains(&(series.set.clone(), series.id.clone(), series.kind))
    }

    /// Folds the captured values without consulting the mutable shared-store current keys.
    pub(crate) fn fold_into(&self, fold: &mut WorkerFold, tip_index: usize) {
        let first_ordinal = fold.admitted.len();
        for (index, (set, run)) in self.runs.iter().enumerate() {
            fold.run_index.record(set, tip_index, &self.commit, false);
            fold.builder.push(
                set,
                tip_index,
                false,
                ordinal_of(first_ordinal.saturating_add(index)),
                &self.commit,
                &RunPoints::from(run),
            );
        }
    }
}

/// Native file acquisition is separate from snapshot decoding and selection.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn read_current_collections(
    base: &Path,
    paths: &[PathBuf],
    project: &str,
) -> Result<Option<CurrentCollections>, AnalyzeError> {
    if paths.is_empty() {
        return Ok(None);
    }
    let snapshots = paths
        .iter()
        .map(|path| {
            let bytes = std::fs::read(base.join(path))
                .map_err(|error| InvalidCurrentCollection::caused_by("reading snapshot", error))?;
            CollectionSnapshot::from_slice(&bytes).map_err(|error| {
                InvalidCurrentCollection::caused_by("decoding snapshot", error).into()
            })
        })
        .collect::<Result<Vec<_>, AnalyzeError>>()?;
    CurrentCollections::new(project, &snapshots).map(Some)
}

/// Current evidence must be complete, unambiguous, and attributable to the analysis context.
#[ohno::error]
#[display("Invalid current collection: {reason}")]
pub(crate) struct InvalidCurrentCollection {
    reason: String,
}

// Immutable error context cannot expose a partially updated invariant during unwinding.
impl UnwindSafe for InvalidCurrentCollection {}
impl RefUnwindSafe for InvalidCurrentCollection {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::indexing_slicing,
        clippy::float_cmp,
        reason = "exact fixture identities and values"
    )]

    use std::num::NonZero;

    use cbh_command::AnalyzeOptions;
    use cbh_config::Config;
    use cbh_detect::testing::synchronous_spawner;
    use cbh_detect::{AnalysisMode, SeriesFilter};
    use cbh_diag::RecordingReporter;
    use cbh_git::FakeGitHistory;
    use cbh_model::{
        BenchmarkResult, Engine, EnvironmentInfo, GitInfo, MachineInfo, Metric, RunContext,
        ToolchainInfo,
    };
    use cbh_storage::{MemoryStorage, Storage};
    use futures::executor::block_on;
    use nonempty::nonempty;
    use ohno::ErrorExt as _;

    use super::*;
    use crate::dataset::{SelectedDataSet, select_dataset};
    use crate::discriminants::AutoDiscriminants;
    use crate::pipeline::analyze_with_current;
    use crate::selection::Selection;

    const KEY: &str = "0123456789abcdef";
    const OTHER_KEY: &str = "fedcba9876543210";
    const LINUX: &str = "x86_64-unknown-linux-gnu";
    const WINDOWS: &str = "x86_64-pc-windows-msvc";

    fn measured(commit: &str, triple: &str, key: &str, value: f64) -> (DiscriminantSet, Run) {
        let set = DiscriminantSet::new(Engine::Callgrind, &triple.into(), &key.into());
        let mut context = RunContext::new(
            "2026-01-01T00:00:00Z".parse().unwrap(),
            GitInfo {
                commit: Some(commit.to_owned()),
                ..GitInfo::default()
            },
            EnvironmentInfo::default(),
            ToolchainInfo {
                target_triple: triple.into(),
                ..ToolchainInfo::default()
            },
            "test".to_owned(),
        );
        context.best_of = Some(NonZero::new(2).unwrap());
        context.machine = Some(MachineInfo {
            processors: 1,
            memory_regions: 1,
            processor_models: Vec::new(),
            processor_speeds: Vec::new(),
            fingerprint: key.to_owned(),
        });
        (
            set,
            Run::new(
                context,
                vec![BenchmarkResult::new(
                    BenchmarkId::new(nonempty!["measured".to_owned()]),
                    vec![
                        Metric::new(MetricKind::InstructionCount, value).with_dispersion(
                            None,
                            Some(value - 1.0),
                            Some(value + 1.0),
                        ),
                    ],
                )],
            ),
        )
    }

    fn snapshot(commit: &str, triple: &str, key: &str, value: f64) -> CollectionSnapshot {
        let (_, run) = measured(commit, triple, key, value);
        CollectionSnapshot::new(
            "project",
            commit,
            triple.into(),
            key.into(),
            vec![(Engine::Callgrind, run)],
        )
        .unwrap()
    }

    fn store(storage: &MemoryStorage, commit: &str, triple: &str, key: &str, value: f64) {
        let (set, mut run) = measured(commit, triple, key, value);
        // Shared history includes another benchmark and a sparse metric not measured now.
        let mut unrelated = run.results[0].clone();
        unrelated.id = BenchmarkId::new(nonempty!["unrelated".to_owned()]);
        run.results.push(unrelated);
        run.results[0]
            .metrics
            .push(Metric::new(MetricKind::ConditionalBranches, 999.0));
        block_on(storage.put_overwrite(
            &set.clean_key("project", commit),
            run.to_json().unwrap().as_bytes(),
        ))
        .unwrap();
    }

    fn dataset(
        git: &FakeGitHistory,
        storage: &MemoryStorage,
        current: &CurrentCollections,
    ) -> SelectedDataSet {
        let options = AnalyzeOptions {
            since: Some("2020-01-01".to_owned()),
            ..AnalyzeOptions::default()
        };
        let mut selection = Selection::from_analyze(&options);
        selection.current = Some(current);
        block_on(select_dataset(
            git,
            storage,
            "project",
            &Config::default(),
            &selection,
            SeriesFilter::default(),
            true,
            &AutoDiscriminants {
                triple: "analyzer".to_owned(),
                machine_key: "analyzer".to_owned(),
            },
            "2026-01-02T00:00:00Z".parse().unwrap(),
            &RecordingReporter::new(),
            &synchronous_spawner(),
            NonZero::new(2).unwrap(),
        ))
        .unwrap()
    }

    #[test]
    fn exact_values_and_roster_survive_shared_tuple_overwrites_and_cross_target_key_reuse() {
        let first = "a".repeat(40);
        let tip = "b".repeat(40);
        let git = crate::testing::two_commit_history(&first, &tip);
        let storage = MemoryStorage::new();
        for (triple, key) in [(LINUX, KEY), (WINDOWS, OTHER_KEY), (WINDOWS, KEY)] {
            store(&storage, &first, triple, key, 10.0);
            store(&storage, &tip, triple, key, 900.0);
        }
        let current = CurrentCollections::new(
            "project",
            &[
                snapshot(&tip, LINUX, KEY, 20.0),
                snapshot(&tip, WINDOWS, OTHER_KEY, 30.0),
            ],
        )
        .unwrap();
        // A later unrelated writer can even corrupt the exact current objects: no reread is valid.
        for (triple, key) in [(LINUX, KEY), (WINDOWS, OTHER_KEY)] {
            let (set, _) = measured(&tip, triple, key, 1.0);
            block_on(storage.put_overwrite(&set.clean_key("project", &tip), b"not JSON")).unwrap();
        }
        let selected = dataset(&git, &storage, &current);
        assert_eq!(selected.series.len(), 2);
        for one in &selected.series {
            assert_eq!(one.id.qualified(), "measured");
            assert_eq!(one.kind, MetricKind::InstructionCount);
            let expected = if one.set.target_triple.as_str() == LINUX {
                20.0
            } else {
                30.0
            };
            assert_eq!(
                one.points
                    .iter()
                    .map(|point| point.value)
                    .collect::<Vec<_>>(),
                [10.0, expected]
            );
            assert_eq!(one.points[1].interval_low, Some(expected - 1.0));
            assert_eq!(one.points[1].interval_high, Some(expected + 1.0));
        }
        assert!(
            selected
                .series
                .iter()
                .all(|one| !(one.set.target_triple.as_str() == WINDOWS
                    && one.set.machine_key.as_str() == KEY))
        );
    }

    #[test]
    fn shared_machine_key_on_distinct_targets_keeps_each_execution_value() {
        let tip = "b".repeat(40);
        let git = crate::testing::two_commit_history(&"a".repeat(40), &tip);
        let current = CurrentCollections::new(
            "project",
            &[
                snapshot(&tip, LINUX, KEY, 20.0),
                snapshot(&tip, WINDOWS, KEY, 30.0),
            ],
        )
        .unwrap();
        let selected = dataset(&git, &MemoryStorage::new(), &current);
        assert_eq!(selected.series.len(), 2);
        let mut values = selected
            .series
            .iter()
            .map(|one| one.points[0].value)
            .collect::<Vec<_>>();
        values.sort_by(f64::total_cmp);
        assert_eq!(values, [20.0, 30.0]);
    }

    #[test]
    fn selected_hardware_partitions_are_not_duplicated_as_diagnostic_siblings() {
        let tip = "b".repeat(40);
        let current = CurrentCollections::new(
            "project",
            &[
                snapshot(&tip, LINUX, KEY, 20.0),
                snapshot(&tip, LINUX, OTHER_KEY, 30.0),
            ],
        )
        .unwrap();
        let key = cbh_model::parse_key(
            &measured(&tip, LINUX, KEY, 1.0)
                .0
                .clean_key("project", &"a".repeat(40)),
        )
        .unwrap();
        assert!(current.admits_history(&key));
        assert!(!current.admits_sibling(&key));
        let unselected = cbh_model::parse_key(
            &measured(&tip, LINUX, "1111111111111111", 1.0)
                .0
                .clean_key("project", &"a".repeat(40)),
        )
        .unwrap();
        assert!(!current.admits_history(&unselected));
        assert!(current.admits_sibling(&unselected));
    }

    #[test]
    fn branch_side_uses_only_selected_executions_while_retaining_base_history() {
        let base = "a".repeat(40);
        let earlier = "b".repeat(40);
        let tip = "c".repeat(40);
        let mut git = FakeGitHistory::new();
        git.commit(&base, None)
            .commit(&earlier, Some(&base))
            .commit(&tip, Some(&earlier))
            .branch("master", &base)
            .branch("feature", &tip)
            .head("feature")
            .mark_default("master");
        let storage = MemoryStorage::new();
        store(&storage, &base, LINUX, KEY, 10.0);
        store(&storage, &earlier, LINUX, KEY, 999.0);
        store(&storage, &tip, LINUX, KEY, 888.0);
        let current =
            CurrentCollections::new("project", &[snapshot(&tip, LINUX, KEY, 25.0)]).unwrap();
        let selected = dataset(&git, &storage, &current);
        assert_eq!(selected.mode, AnalysisMode::Branch);
        assert_eq!(selected.series.len(), 1);
        assert_eq!(
            selected.series[0]
                .points
                .iter()
                .map(|p| p.value)
                .collect::<Vec<_>>(),
            [25.0]
        );
        assert_eq!(
            selected.series[0]
                .base_window
                .iter()
                .map(|p| p.value)
                .collect::<Vec<_>>(),
            [10.0]
        );
    }

    #[test]
    fn rejects_empty_ambiguous_or_misattributed_current_evidence() {
        let tip = "b".repeat(40);
        let one = snapshot(&tip, LINUX, KEY, 20.0);
        for snapshots in [
            Vec::new(),
            vec![one.clone(), one.clone()],
            vec![one.clone(), snapshot(&"c".repeat(40), WINDOWS, KEY, 30.0)],
            vec![
                CollectionSnapshot::new("project", &tip, LINUX.into(), KEY.into(), Vec::new())
                    .unwrap(),
            ],
        ] {
            let error = CurrentCollections::new("project", &snapshots)
                .err()
                .unwrap();
            assert!(error.find_source::<InvalidCurrentCollection>().is_some());
        }
        assert!(CurrentCollections::new("other", std::slice::from_ref(&one)).is_err());
        let current = CurrentCollections::new("project", &[one]).unwrap();
        assert!(current.validate_context(&tip, true).is_err());
        assert!(current.validate_context(&"c".repeat(40), false).is_err());
    }

    #[test]
    fn scoped_report_census_has_only_measured_series_and_preserves_insufficient_baseline() {
        let tip = "b".repeat(40);
        let git = crate::testing::two_commit_history(&"a".repeat(40), &tip);
        let current =
            CurrentCollections::new("project", &[snapshot(&tip, LINUX, KEY, 20.0)]).unwrap();
        let options = AnalyzeOptions {
            json: Some("report.json".into()),
            ..AnalyzeOptions::default()
        };
        let (reports, _) = block_on(analyze_with_current(
            &git,
            &MemoryStorage::new(),
            "project",
            &Config::default(),
            &options,
            &AutoDiscriminants {
                triple: "other".to_owned(),
                machine_key: "other".to_owned(),
            },
            "2026-01-02T00:00:00Z".parse().unwrap(),
            &RecordingReporter::new(),
            false,
            &synchronous_spawner(),
            NonZero::<usize>::MIN,
            Some(&current),
        ))
        .unwrap();
        let report: serde_json::Value =
            serde_json::from_str(reports.json.as_ref().unwrap()).unwrap();
        assert_eq!(report["census"]["in_scope"], 1);
        assert_eq!(report["census"]["judged"], 0);
        assert_eq!(report["outcome"], "insufficient_baseline");
    }
}
