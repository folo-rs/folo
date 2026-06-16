//! The series engine: reconstruct per-benchmark, per-metric time series from the
//! stored result sets so the finding algorithms can reason over them.
//!
//! A series is identified by the comparable [`DiscriminantSet`] its runs share
//! (parsed from the storage key) together with the [`BenchmarkId`] and metric
//! name. Its points are ordered by *git topology* — the first-parent position of
//! their commit, supplied as a `commit -> index` map — so the timeline reflects
//! the history the runs were measured against rather than when they were ingested.
//! Within a single commit, clean runs precede dirty snapshots, and ties break by
//! effective time and then the storage key for determinism.

use std::collections::{BTreeMap, HashMap};

use jiff::Timestamp;

use crate::analyze::discriminant::{DiscriminantSet, ParsedKey};
use crate::model::{BenchmarkId, MetricKind, ResultSet};

/// A single observation in a series.
#[derive(Clone, Debug)]
pub(crate) struct SeriesPoint {
    /// First-parent topological position of the run's commit (oldest = 0).
    pub(crate) topo_index: usize,
    /// Whether the observation came from a dirty (uncommitted-tree) snapshot.
    pub(crate) dirty: bool,
    /// Effective time of the run (a within-commit, within-cleanliness tie-break).
    pub(crate) effective: Timestamp,
    /// Storage key the observation came from (final tie-break and provenance).
    pub(crate) object_key: String,
    /// Abbreviated commit the run was measured against, if known.
    pub(crate) commit: Option<String>,
    /// The measured value.
    pub(crate) value: f64,
}

/// A per-`(set, benchmark, metric)` time series ordered by git topology.
#[derive(Clone, Debug)]
pub(crate) struct Series {
    /// The comparable discriminant set all points share.
    pub(crate) set: DiscriminantSet,
    /// The benchmark identity all points share.
    pub(crate) id: BenchmarkId,
    /// The metric name all points share.
    pub(crate) metric: String,
    /// The metric category (governs comparison semantics).
    pub(crate) kind: MetricKind,
    /// Observations ordered by `(topo_index, dirty, effective, object_key)`.
    pub(crate) points: Vec<SeriesPoint>,
}

/// One stored object selected for analysis, ready to be folded into series.
#[derive(Clone, Debug)]
pub(crate) struct LoadedObject {
    /// The parsed storage key (discriminant set, commit, and cleanliness).
    pub(crate) key: ParsedKey,
    /// The full storage object key (provenance and final tie-break).
    pub(crate) object_key: String,
    /// The decoded result set.
    pub(crate) result: ResultSet,
}

/// Filters applied while building series from stored runs.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SeriesFilter<'a> {
    /// Keep only metrics with this exact name, if set.
    pub(crate) metric: Option<&'a str>,
}

/// Reconstructs every series from the selected `objects`.
///
/// Each metric of each record becomes one point in the series for its
/// `(discriminant set, benchmark id, metric name)`. The `order` map gives each
/// commit its first-parent topological index; an object whose commit is not in
/// `order` is outside the analyzed selection and is skipped. Points are sorted by
/// `(topo_index, dirty, effective, object_key)` so a clean run precedes a dirty
/// snapshot on the same commit and the timeline follows git history rather than
/// storage-listing order.
pub(crate) fn build_series(
    objects: &[LoadedObject],
    order: &HashMap<String, usize>,
    filter: &SeriesFilter<'_>,
) -> Vec<Series> {
    let mut groups: BTreeMap<
        (DiscriminantSet, BenchmarkId, String),
        (MetricKind, Vec<SeriesPoint>),
    > = BTreeMap::new();

    for object in objects {
        let Some(&topo_index) = order.get(&object.key.commit) else {
            continue;
        };
        let dirty = object.key.is_dirty();
        let effective = object.result.context.timestamps.effective;
        let commit = object.result.context.git.short_commit.clone();

        for record in &object.result.results {
            for metric in &record.metrics {
                if filter.metric.is_some_and(|want| metric.name != want) {
                    continue;
                }
                let point = SeriesPoint {
                    topo_index,
                    dirty,
                    effective,
                    object_key: object.object_key.clone(),
                    commit: commit.clone(),
                    value: metric.value,
                };
                groups
                    .entry((
                        object.key.set.clone(),
                        record.id.clone(),
                        metric.name.clone(),
                    ))
                    .or_insert_with(|| (metric.kind, Vec::new()))
                    .1
                    .push(point);
            }
        }
    }

    groups
        .into_iter()
        .map(|((set, id, metric), (kind, mut points))| {
            points.sort_by(|left, right| {
                left.topo_index
                    .cmp(&right.topo_index)
                    .then_with(|| left.dirty.cmp(&right.dirty))
                    .then_with(|| left.effective.cmp(&right.effective))
                    .then_with(|| left.object_key.cmp(&right.object_key))
            });
            Series {
                set,
                id,
                metric,
                kind,
                points,
            }
        })
        .collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "metric values are exact integer-derived counts"
    )]
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use crate::analyze::discriminant::parse_key;
    use crate::context::{CiInfo, GitInfo, RunContext, Timestamps, ToolchainInfo};
    use crate::model::{Metric, ResultRecord};

    use super::*;

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).expect("seconds within range")
    }

    /// Builds a stored result set with one record carrying one `Ir` metric.
    fn result_set(effective: Timestamp, commit: &str, value: f64) -> ResultSet {
        result_set_for_package(effective, commit, value, None)
    }

    /// Builds a stored result set whose single record is scoped to `package`,
    /// keeping every other identity component fixed.
    fn result_set_for_package(
        effective: Timestamp,
        commit: &str,
        value: f64,
        package: Option<&str>,
    ) -> ResultSet {
        let context = RunContext::new(
            Timestamps::new(effective, effective, effective),
            GitInfo {
                commit: Some(format!("{commit}full")),
                short_commit: Some(commit.to_owned()),
                branch: Some("main".to_owned()),
                dirty: false,
            },
            CiInfo::default(),
            ToolchainInfo::default(),
            "0.0.1".to_owned(),
        );
        let record = ResultRecord::new(
            BenchmarkId::new(
                package.map(ToOwned::to_owned),
                "group".to_owned(),
                Some("case".to_owned()),
                None,
            ),
            vec![Metric::new(
                "Ir".to_owned(),
                MetricKind::InstructionCount,
                value,
                Some("count".to_owned()),
            )],
        );
        ResultSet::new(context, vec![record])
    }

    /// A clean object at `commit` carrying the given `Ir` value.
    fn clean_object(commit: &str, effective: i64, value: f64) -> LoadedObject {
        let object_key =
            format!("v2/proj/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/clean.json");
        LoadedObject {
            key: parse_key(&object_key).expect("clean key parses"),
            object_key,
            result: result_set(ts(effective), commit, value),
        }
    }

    /// A dirty snapshot at `commit` taken at `unix`, carrying the given value.
    fn dirty_object(commit: &str, unix: i64, value: f64) -> LoadedObject {
        let object_key = format!(
            "v2/proj/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/dirty-{unix}.json"
        );
        LoadedObject {
            key: parse_key(&object_key).expect("dirty key parses"),
            object_key,
            result: result_set(ts(unix), commit, value),
        }
    }

    fn order(commits: &[&str]) -> HashMap<String, usize> {
        commits
            .iter()
            .enumerate()
            .map(|(index, commit)| ((*commit).to_owned(), index))
            .collect()
    }

    #[test]
    fn build_series_orders_points_by_topology_not_effective_time() {
        // Topology is c0,c1,c2 but the effective times are deliberately reversed;
        // topology must win so the values come out in commit order.
        let objects = vec![
            clean_object("c2", 100, 30.0),
            clean_object("c0", 300, 10.0),
            clean_object("c1", 200, 20.0),
        ];
        let series = build_series(
            &objects,
            &order(&["c0", "c1", "c2"]),
            &SeriesFilter::default(),
        );
        assert_eq!(series.len(), 1);
        let values: Vec<f64> = series[0].points.iter().map(|point| point.value).collect();
        assert_eq!(values, vec![10.0, 20.0, 30.0]);
    }

    #[test]
    fn build_series_orders_clean_before_dirty_within_a_commit() {
        // One commit with a clean run plus two dirty snapshots; clean comes first,
        // then the dirty snapshots ordered by effective time.
        let objects = vec![
            dirty_object("c0", 300, 33.0),
            dirty_object("c0", 200, 22.0),
            clean_object("c0", 100, 11.0),
        ];
        let series = build_series(&objects, &order(&["c0"]), &SeriesFilter::default());
        let values: Vec<f64> = series[0].points.iter().map(|point| point.value).collect();
        assert_eq!(values, vec![11.0, 22.0, 33.0]);
        let dirty: Vec<bool> = series[0].points.iter().map(|point| point.dirty).collect();
        assert_eq!(dirty, vec![false, true, true]);
    }

    #[test]
    fn build_series_skips_commits_outside_the_selection() {
        // `c9` is not in the order map (outside the analyzed selection), so its
        // object contributes nothing.
        let objects = vec![clean_object("c0", 100, 10.0), clean_object("c9", 200, 99.0)];
        let series = build_series(&objects, &order(&["c0"]), &SeriesFilter::default());
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].points.len(), 1);
        assert_eq!(series[0].points[0].value, 10.0);
    }

    #[test]
    fn build_series_separates_sets() {
        // A second run in a different triple is a different (incomparable) series.
        let other_key =
            "v2/proj/callgrind/aarch64-unknown-linux-gnu/synthetic/c0/clean.json".to_owned();
        let other = LoadedObject {
            key: parse_key(&other_key).expect("key parses"),
            object_key: other_key,
            result: result_set(ts(200), "c0", 20.0),
        };
        let objects = vec![clean_object("c0", 100, 10.0), other];
        let series = build_series(&objects, &order(&["c0"]), &SeriesFilter::default());
        assert_eq!(series.len(), 2, "different triples are different series");
    }

    #[test]
    fn build_series_separates_packages() {
        // Two records share group/case/metric and set but belong to different
        // packages, so they must form two series rather than silently merging.
        let foo_key =
            "v2/proj/callgrind/x86_64-unknown-linux-gnu/synthetic/c0/clean.json".to_owned();
        let bar_key =
            "v2/proj/callgrind/x86_64-unknown-linux-gnu/synthetic/c1/clean.json".to_owned();
        let objects = vec![
            LoadedObject {
                key: parse_key(&foo_key).expect("key parses"),
                object_key: foo_key,
                result: result_set_for_package(ts(100), "c0", 10.0, Some("foo")),
            },
            LoadedObject {
                key: parse_key(&bar_key).expect("key parses"),
                object_key: bar_key,
                result: result_set_for_package(ts(200), "c1", 20.0, Some("bar")),
            },
        ];
        let series = build_series(&objects, &order(&["c0", "c1"]), &SeriesFilter::default());
        assert_eq!(series.len(), 2, "different packages are different series");
    }

    #[test]
    fn build_series_applies_metric_filter() {
        let mut object = clean_object("c0", 100, 10.0);
        object.result.results[0].metrics.push(Metric::new(
            "EstimatedCycles".to_owned(),
            MetricKind::EstimatedCycles,
            99.0,
            Some("count".to_owned()),
        ));
        let filter = SeriesFilter { metric: Some("Ir") };
        let series = build_series(&[object], &order(&["c0"]), &filter);
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].metric, "Ir");
    }
}
