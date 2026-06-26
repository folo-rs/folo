//! The series engine: reconstruct per-benchmark, per-metric time series from the
//! stored runs so the finding algorithms can reason over them.
//!
//! A series is identified by the comparable [`DiscriminantSet`] its runs share
//! (parsed from the storage key) together with the [`BenchmarkId`] and metric
//! kind. Its points are ordered by *git topology* — the first-parent position of
//! their commit, supplied as a `commit -> index` map — so the timeline reflects
//! the history the runs were measured against rather than when they were ingested.
//! Within a single commit, clean runs precede dirty snapshots, and any remaining
//! tie breaks by the object's storage-key order (its ordinal) for determinism.
//!
//! Points are kept deliberately small: tens of millions can be resident at once on
//! a large history, so the storage key is reduced to a 4-byte ordinal and the
//! per-point commit string is interned to a shared `Arc<str>` rather than cloned.

use std::collections::{BTreeMap, HashMap};
use std::hash::BuildHasher;
use std::sync::Arc;

use jiff::Timestamp;

use crate::analyze::StorageKey;
use crate::model::BlessingRecord;
use crate::model::DiscriminantSet;
use crate::model::{BenchmarkId, BenchmarkIdPrefix, MetricKind, Run};

/// A single observation in a series.
///
/// Kept compact because a large history materializes tens of millions of these at
/// once: the provenance storage key is reduced to [`object_ordinal`] (the key's
/// rank in sorted storage-key order, the final tie-break) and the abbreviated
/// commit is an interned [`Arc<str>`] shared across every point measured against
/// the same commit.
///
/// [`object_ordinal`]: SeriesPoint::object_ordinal
#[derive(Clone, Debug)]
pub struct SeriesPoint {
    /// First-parent topological position of the run's commit (oldest = 0).
    pub topo_index: usize,
    /// Whether the observation came from a dirty (uncommitted-tree) snapshot.
    pub dirty: bool,
    /// The object's rank in sorted storage-key order (the final tie-break),
    /// standing in for the full storage key to keep the point small.
    pub object_ordinal: u32,
    /// Abbreviated commit the run was measured against, if known. Interned so all
    /// points on one commit share a single allocation.
    pub commit: Option<Arc<str>>,
    /// The measured value.
    pub value: f64,
    /// Lower confidence-interval bound, when the engine reports one (Criterion).
    pub interval_low: Option<f64>,
    /// Upper confidence-interval bound, when the engine reports one (Criterion).
    pub interval_high: Option<f64>,
}

/// A blessing that applies to a series: the commit it was issued at, that commit's
/// topological position, and the accepted level's provenance.
///
/// In history analysis a series with a matching blessing is *re-baselined* to the
/// blessed commit — the detector treats the blessed level as the new baseline and
/// only sees points from that commit onward, while the pre-blessing points are
/// retained for charting (drawn greyed). See the *Re-baselining* analysis
/// section of `DESIGN.md`.
#[derive(Clone, Debug)]
pub struct Blessing {
    /// Full commit SHA the blessing was issued at (the report anchor).
    pub commit: String,
    /// Committer date of the blessed commit (resolved from git topology), for the
    /// report anchor. `None` when topology did not report a date for the commit.
    pub commit_time: Option<Timestamp>,
}

/// A blessing positioned in history, collected per discriminant set and consumed
/// by [`apply_blessings`].
///
/// The tuple pairs the topological index of the commit the blessing was issued at,
/// that commit's committer date (resolved from git topology, `None` when topology
/// did not report one), and the blessing record itself.
pub type BlessingPlacement = (usize, Option<Timestamp>, BlessingRecord);

/// A per-`(set, benchmark, metric kind)` time series ordered by git topology.
#[derive(Clone, Debug)]
pub struct Series {
    /// The comparable discriminant set all points share.
    pub set: DiscriminantSet,
    /// The benchmark identity all points share.
    pub id: BenchmarkId,
    /// The metric kind all points share (governs unit and comparison semantics).
    pub kind: MetricKind,
    /// Observations ordered by `(topo_index, dirty, object_ordinal)`.
    pub points: Vec<SeriesPoint>,
    /// Index into `points` where the active (post-blessing) window begins; `0`
    /// when the series is unblessed (every point is active). History-mode
    /// detection considers only `points[active_start..]`, while charts draw the
    /// whole series with the pre-`active_start` prefix greyed.
    pub active_start: usize,
    /// The blessing that re-baselined this series, if any (the report anchor).
    pub blessing: Option<Blessing>,
}

/// One stored object selected for analysis, ready to be folded into series.
#[derive(Clone, Debug)]
pub struct LoadedObject {
    /// The parsed storage key (discriminant set, commit, and cleanliness).
    pub key: StorageKey,
    /// The full storage object key (provenance and final tie-break).
    pub object_key: String,
    /// The decoded run.
    pub result: Run,
}

/// Filters applied while building series from stored runs.
#[derive(Clone, Copy, Debug, Default)]
pub struct SeriesFilter<'a> {
    /// Keep only series whose benchmark identity's qualified id starts with one of
    /// these prefixes. Empty keeps every series.
    pub prefixes: &'a [BenchmarkIdPrefix],
}

/// Whether `prefixes` accepts `id` (an empty prefix list accepts every id).
///
/// The match is a raw `starts_with` against the benchmark's qualified identity,
/// mirroring blessing-prefix matching so the same prefix selects the same family
/// of benchmarks in `bless` and `analyze`.
fn prefixes_accept(prefixes: &[BenchmarkIdPrefix], id: &BenchmarkId) -> bool {
    if prefixes.is_empty() {
        return true;
    }
    let qualified = id.qualified();
    prefixes
        .iter()
        .any(|prefix| qualified.starts_with(prefix.as_str()))
}

/// Reconstructs every series from the selected `objects`.
///
/// Each metric of each result becomes one point in the series for its
/// `(discriminant set, benchmark id, metric kind)`. A result carries at most one
/// metric of each kind, so the kind alone keys the series unambiguously. The
/// `order` map gives each commit its first-parent topological index; an object
/// whose commit is not in `order` is outside the analyzed selection and is
/// skipped. Points are sorted by `(topo_index, dirty, object_ordinal)` so a clean
/// run precedes a dirty snapshot on the same commit and the timeline follows git
/// history rather than storage-listing order; the ordinal is each object's rank in
/// sorted storage-key order, so the final tie-break matches a sort by storage key.
///
/// This is a convenience wrapper over [`SeriesBuilder`]: it assigns each object its
/// storage-key ordinal up front, then folds every object in. Callers that fetch
/// objects incrementally (and want to drop each parsed run immediately, never
/// holding the whole data set in memory) should drive a [`SeriesBuilder`] directly.
#[must_use]
pub fn build_series<S: BuildHasher>(
    objects: &[LoadedObject],
    order: &HashMap<String, usize, S>,
    filter: &SeriesFilter<'_>,
) -> Vec<Series> {
    // Assign each in-selection object an ordinal equal to its rank in sorted
    // storage-key order, so ordering points by `object_ordinal` is identical to
    // ordering them by the full storage key.
    let mut keys: Vec<&str> = objects
        .iter()
        .filter(|object| order.contains_key(&object.key.commit))
        .map(|object| object.object_key.as_str())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    let ordinals: HashMap<&str, u32> = keys
        .iter()
        .enumerate()
        .map(|(index, key)| (*key, ordinal_of(index)))
        .collect();

    let mut builder = SeriesBuilder::new(*filter);
    for object in objects {
        let Some(&topo_index) = order.get(&object.key.commit) else {
            continue;
        };
        let ordinal = ordinals
            .get(object.object_key.as_str())
            .copied()
            .unwrap_or(u32::MAX);
        builder.push(
            &object.key.set,
            topo_index,
            object.key.is_dirty(),
            ordinal,
            &object.result,
        );
    }
    builder.finish()
}

/// Narrows a storage-key rank to the [`SeriesPoint::object_ordinal`] width.
///
/// Ordinals are a pure tie-break, so the (practically impossible) overflow past
/// `u32::MAX` distinct objects merely lets the last ordinals collide — points then
/// keep their stable insertion order, never a panic.
#[expect(
    clippy::cast_possible_truncation,
    reason = "saturating: ordinals only tie-break, and >4 billion objects never occur"
)]
fn ordinal_of(rank: usize) -> u32 {
    rank.min(u32::MAX as usize) as u32
}

/// Folds stored runs into per-`(set, benchmark, metric kind)` series.
///
/// Each pushed run contributes one point per metric to the series for its
/// identity; [`finish`](SeriesBuilder::finish) sorts every series by
/// `(topo_index, dirty, object_ordinal)`. Unlike [`build_series`], the builder
/// retains nothing of a run beyond the extracted points, so a streaming loader can
/// fold each fetched run and drop it immediately, keeping only the (compact) points
/// resident rather than the whole parsed data set.
///
/// The abbreviated commit of each run is interned, so all points measured against
/// one commit share a single [`Arc<str>`] instead of cloning the string per point.
#[derive(Debug)]
pub struct SeriesBuilder<'a> {
    filter: SeriesFilter<'a>,
    groups: BTreeMap<(DiscriminantSet, BenchmarkId, MetricKind), Vec<SeriesPoint>>,
    commits: HashMap<Box<str>, Arc<str>>,
}

impl<'a> SeriesBuilder<'a> {
    /// Starts an empty builder that keeps only series matching `filter`.
    #[must_use]
    pub fn new(filter: SeriesFilter<'a>) -> Self {
        Self {
            filter,
            groups: BTreeMap::new(),
            commits: HashMap::new(),
        }
    }

    /// Folds one stored run into the accumulating series.
    ///
    /// `topo_index` is the run commit's first-parent position and `object_ordinal`
    /// its rank in sorted storage-key order (the final point tie-break); the caller
    /// supplies both because they come from the storage key and git topology, not
    /// the run payload. Only the matching metrics are retained — the run itself can
    /// be dropped as soon as this returns.
    pub fn push(
        &mut self,
        set: &DiscriminantSet,
        topo_index: usize,
        dirty: bool,
        object_ordinal: u32,
        run: &Run,
    ) {
        let commit = run
            .context
            .git
            .short_commit
            .as_deref()
            .map(|commit| self.intern(commit));

        for record in &run.results {
            if !prefixes_accept(self.filter.prefixes, &record.id) {
                continue;
            }
            for metric in &record.metrics {
                let point = SeriesPoint {
                    topo_index,
                    dirty,
                    object_ordinal,
                    commit: commit.clone(),
                    value: metric.value,
                    interval_low: metric.interval_low,
                    interval_high: metric.interval_high,
                };
                self.groups
                    .entry((set.clone(), record.id.clone(), metric.kind))
                    .or_default()
                    .push(point);
            }
        }
    }

    /// Returns the interned `Arc<str>` for `commit`, allocating once per distinct
    /// commit so the millions of points on a commit share one allocation.
    fn intern(&mut self, commit: &str) -> Arc<str> {
        if let Some(existing) = self.commits.get(commit) {
            return Arc::clone(existing);
        }
        let interned: Arc<str> = Arc::from(commit);
        self.commits
            .insert(Box::from(commit), Arc::clone(&interned));
        interned
    }

    /// Finalizes every accumulated series, each sorted into topological order.
    #[must_use]
    pub fn finish(self) -> Vec<Series> {
        self.groups
            .into_iter()
            .map(|((set, id, kind), mut points)| {
                points.sort_by(|left, right| {
                    left.topo_index
                        .cmp(&right.topo_index)
                        .then_with(|| left.dirty.cmp(&right.dirty))
                        .then_with(|| left.object_ordinal.cmp(&right.object_ordinal))
                });
                Series {
                    set,
                    id,
                    kind,
                    points,
                    active_start: 0,
                    blessing: None,
                }
            })
            .collect()
    }
}

/// Re-baselines each series to its latest matching blessing (history mode).
///
/// For every series, the most recent blessing (by topological position) whose
/// prefixes accept the series' benchmark id selects the re-baseline commit. The
/// series' `active_start` is set to the first point at or after that commit, so the
/// detector only sees the post-blessing window while the full series is retained
/// for charting. A series with no matching blessing is left untouched
/// (`active_start = 0`). Branch and tip modes pass an empty map and so are
/// unaffected.
///
/// Each blessing carries the committer date of its commit (resolved from git
/// topology, `None` when topology did not report one), which becomes the report
/// anchor for the re-baselined series.
pub fn apply_blessings<S: BuildHasher>(
    series: &mut [Series],
    blessings: &HashMap<DiscriminantSet, Vec<BlessingPlacement>, S>,
) {
    for one in series.iter_mut() {
        let Some(set_blessings) = blessings.get(&one.set) else {
            continue;
        };
        let latest = set_blessings
            .iter()
            .filter(|(_, _, record)| record.matches(&one.id))
            .max_by_key(|(topo_index, _, _)| *topo_index);
        let Some((topo_index, commit_time, record)) = latest else {
            continue;
        };
        // The active window starts at the first point on or after the blessed
        // commit. `points` is already sorted by `topo_index`, so a partition point
        // locates the boundary.
        one.active_start = one
            .points
            .partition_point(|point| point.topo_index < *topo_index);
        one.blessing = Some(Blessing {
            commit: record.commit.clone(),
            commit_time: *commit_time,
        });
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "metric values are exact integer-derived counts"
    )]
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use crate::analyze::parse_key;
    use crate::model::{BenchmarkResult, Metric};
    use crate::model::{EnvironmentInfo, GitInfo, RunContext, ToolchainInfo};

    use nonempty::NonEmpty;

    use super::*;

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).unwrap()
    }

    /// Builds a stored run with one result carrying one instruction-count metric.
    fn run(observation: Timestamp, commit: &str, value: f64) -> Run {
        run_for_package(observation, commit, value, None)
    }

    /// Builds a stored run whose single result is scoped to `package`, keeping
    /// every other identity segment fixed.
    fn run_for_package(
        observation: Timestamp,
        commit: &str,
        value: f64,
        package: Option<&str>,
    ) -> Run {
        let context = RunContext::new(
            observation,
            GitInfo {
                commit: Some(format!("{commit}full")),
                short_commit: Some(commit.to_owned()),
                branch: Some("main".to_owned()),
                dirty: false,
            },
            EnvironmentInfo::default(),
            ToolchainInfo::default(),
            "0.0.1".to_owned(),
        );
        let mut segments = Vec::new();
        if let Some(package) = package {
            segments.push(package.to_owned());
        }
        segments.push("group".to_owned());
        segments.push("case".to_owned());
        let record = BenchmarkResult::new(
            BenchmarkId::new(NonEmpty::from_vec(segments).unwrap()),
            vec![Metric::new(MetricKind::InstructionCount, value)],
        );
        Run::new(context, vec![record])
    }

    /// A clean object at `commit` carrying the given instruction-count value.
    fn clean_object(commit: &str, observation: i64, value: f64) -> LoadedObject {
        let object_key =
            format!("v1/proj/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/clean.json");
        LoadedObject {
            key: parse_key(&object_key).unwrap(),
            object_key,
            result: run(ts(observation), commit, value),
        }
    }

    /// A dirty snapshot at `commit` taken at `unix`, carrying the given value.
    fn dirty_object(commit: &str, unix: i64, value: f64) -> LoadedObject {
        let object_key = format!(
            "v1/proj/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/dirty-{unix}.json"
        );
        LoadedObject {
            key: parse_key(&object_key).unwrap(),
            object_key,
            result: run(ts(unix), commit, value),
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
    fn build_series_orders_points_by_topology() {
        // The objects are supplied out of topological order; build_series must sort
        // them by their commit's first-parent index so the values come out in
        // commit order regardless of insertion order.
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
        // then the dirty snapshots ordered by their storage key (`dirty-<unix>`).
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
            "v1/proj/callgrind/aarch64-unknown-linux-gnu/synthetic/c0/clean.json".to_owned();
        let other = LoadedObject {
            key: parse_key(&other_key).unwrap(),
            object_key: other_key,
            result: run(ts(200), "c0", 20.0),
        };
        let objects = vec![clean_object("c0", 100, 10.0), other];
        let series = build_series(&objects, &order(&["c0"]), &SeriesFilter::default());
        assert_eq!(series.len(), 2, "different triples are different series");
    }

    #[test]
    fn build_series_separates_packages() {
        // Two results share group/case/kind and set but belong to different
        // packages, so they must form two series rather than silently merging.
        let foo_key =
            "v1/proj/callgrind/x86_64-unknown-linux-gnu/synthetic/c0/clean.json".to_owned();
        let bar_key =
            "v1/proj/callgrind/x86_64-unknown-linux-gnu/synthetic/c1/clean.json".to_owned();
        let objects = vec![
            LoadedObject {
                key: parse_key(&foo_key).unwrap(),
                object_key: foo_key,
                result: run_for_package(ts(100), "c0", 10.0, Some("foo")),
            },
            LoadedObject {
                key: parse_key(&bar_key).unwrap(),
                object_key: bar_key,
                result: run_for_package(ts(200), "c1", 20.0, Some("bar")),
            },
        ];
        let series = build_series(&objects, &order(&["c0", "c1"]), &SeriesFilter::default());
        assert_eq!(series.len(), 2, "different packages are different series");
    }

    #[test]
    fn build_series_applies_prefix_filter() {
        // Two benchmarks in different packages; a prefix selects only one family.
        let foo_key =
            "v1/proj/callgrind/x86_64-unknown-linux-gnu/synthetic/c0/clean.json".to_owned();
        let bar_key =
            "v1/proj/callgrind/x86_64-unknown-linux-gnu/synthetic/c1/clean.json".to_owned();
        let objects = vec![
            LoadedObject {
                key: parse_key(&foo_key).unwrap(),
                object_key: foo_key,
                result: run_for_package(ts(100), "c0", 10.0, Some("foo")),
            },
            LoadedObject {
                key: parse_key(&bar_key).unwrap(),
                object_key: bar_key,
                result: run_for_package(ts(200), "c1", 20.0, Some("bar")),
            },
        ];
        let prefixes = vec![BenchmarkIdPrefix::new("foo/").unwrap()];
        let filter = SeriesFilter {
            prefixes: &prefixes,
        };
        let series = build_series(&objects, &order(&["c0", "c1"]), &filter);
        assert_eq!(series.len(), 1, "only the foo-prefixed benchmark is kept");
        assert_eq!(series[0].id.qualified(), "foo/group/case");
    }

    fn blessing(prefixes: &[&str], commit: &str, issued: i64) -> BlessingRecord {
        BlessingRecord::new(
            commit.to_owned(),
            ts(issued),
            prefixes
                .iter()
                .map(|prefix| BenchmarkIdPrefix::new(*prefix).unwrap())
                .collect(),
            "0.0.1".to_owned(),
        )
    }

    /// A four-commit `c0..c3` series, ready for blessing tests.
    fn four_commit_series() -> Vec<Series> {
        let objects = vec![
            clean_object("c0", 100, 10.0),
            clean_object("c1", 200, 20.0),
            clean_object("c2", 300, 30.0),
            clean_object("c3", 400, 40.0),
        ];
        build_series(
            &objects,
            &order(&["c0", "c1", "c2", "c3"]),
            &SeriesFilter::default(),
        )
    }

    #[test]
    fn apply_blessings_rebaselines_to_the_matching_blessing() {
        let mut series = four_commit_series();
        let set = series[0].set.clone();
        let mut map = HashMap::new();
        // Blessed at the c2 commit (topological index 2), committer date ts(300).
        map.insert(
            set,
            vec![(2_usize, Some(ts(300)), blessing(&["group"], "c2full", 301))],
        );

        apply_blessings(&mut series, &map);

        // The active window begins at the first point on or after c2, so the c0/c1
        // points are excluded from detection while retained for charts.
        assert_eq!(series[0].active_start, 2);
        let recorded = series[0].blessing.as_ref().unwrap();
        assert_eq!(recorded.commit, "c2full");
        assert_eq!(recorded.commit_time, Some(ts(300)));
    }

    #[test]
    fn apply_blessings_ignores_a_non_matching_blessing() {
        let mut series = four_commit_series();
        let set = series[0].set.clone();
        let mut map = HashMap::new();
        // The series' benchmark id is `group/case`; this prefix matches nothing.
        map.insert(
            set,
            vec![(2_usize, Some(ts(300)), blessing(&["other"], "c2full", 301))],
        );

        apply_blessings(&mut series, &map);

        assert_eq!(series[0].active_start, 0, "no re-baseline");
        assert!(series[0].blessing.is_none(), "no blessing recorded");
    }

    #[test]
    fn apply_blessings_picks_the_latest_matching_blessing() {
        let mut series = four_commit_series();
        let set = series[0].set.clone();
        let mut map = HashMap::new();
        // Two matching blessings; the later one (c3, index 3) wins.
        map.insert(
            set,
            vec![
                (1_usize, Some(ts(200)), blessing(&["group"], "c1full", 201)),
                (3_usize, Some(ts(400)), blessing(&["group"], "c3full", 401)),
            ],
        );

        apply_blessings(&mut series, &map);

        assert_eq!(series[0].active_start, 3);
        assert_eq!(series[0].blessing.as_ref().unwrap().commit, "c3full");
    }
}
