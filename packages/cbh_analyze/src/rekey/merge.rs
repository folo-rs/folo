//! Deciding whether merging two machine-key partitions is safe.
//!
//! Rekeying splices two sets of measurements into a single series. When the two
//! partitions really are the same machine, their measurement levels agree and the
//! splice is invisible. When they systematically differ, the merge *manufactures* a
//! step change at the splice point, and the next analysis reports it as a regression —
//! precisely the false positive the migration exists to remove. This module quantifies
//! that risk before anything is written.
//!
//! The **level offset** decides it. That is how far apart the two groups sit on a given
//! benchmark and metric, compared as medians so a single outlying run cannot dominate.
//! An offset within tolerance on every shared benchmark clears the merge; any offset
//! beyond it blocks the merge outright.
//!
//! The **interleaving pattern** — how the two groups sit relative to each other in
//! commit order — is reported alongside but decides nothing. Groups that alternate are
//! one machine whose key wobbled back and forth, and a merge simply reunites them,
//! while groups occupying disjoint stretches of history are indistinguishable from a
//! real change at the boundary. That distinction sharpens an operator's reading of an
//! offset, so it reaches the report rather than the refusal.

use std::collections::{BTreeMap, BTreeSet};

use cbh_detect::AnalysisConfig;
use cbh_model::{DiscriminantSet, MetricKind};
use cbh_stats::median;

/// The fraction of the detector's practical-significance floor a merged partition's
/// level offset may reach before `rekey` refuses to merge.
///
/// The detector only reports a move that clears *both* its relative floor and the
/// absolute floor for the metric's kind, so an offset below those floors cannot
/// produce a finding no matter where the splice lands. Half of them is therefore a
/// deliberately conservative margin: an offset that passes leaves a full factor of two
/// of headroom before the merged series could raise anything, which covers the
/// detector composing the splice with the series' own movement rather than measuring
/// it in isolation.
const MERGE_TOLERANCE_FRACTION: f64 = 0.5;

/// One benchmark measurement read out of a stored run, placed in commit order.
#[derive(Clone, Debug)]
pub(crate) struct MeasuredPoint {
    /// The qualified benchmark identity the measurement belongs to.
    pub(crate) benchmark: String,
    /// Which metric was measured.
    pub(crate) metric: MetricKind,
    /// The measured value, in the metric's own units.
    pub(crate) value: f64,
    /// First-parent position of the run's commit, or `None` when the commit is not on
    /// the target ref's first-parent line (so its place in history is unknown).
    pub(crate) position: Option<usize>,
}

/// How two groups of measurements sit relative to each other in commit order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Interleaving {
    /// The groups' stretches of history overlap. One machine's key wobbled back and
    /// forth, so a merge reunites points that already share a timeline: no single
    /// boundary exists for a manufactured step to sit on.
    Interleaved,
    /// Each group occupies its own contiguous stretch of history, disjoint from the
    /// other's. The merge creates a single boundary, which any level offset turns
    /// into a step change that reads exactly like a real one.
    TimeBlocked,
    /// At least one group has no commit on the target ref's first-parent line, so the
    /// groups cannot be ordered against each other.
    Unknown,
}

impl Interleaving {
    /// The label used in rendered reports and machine-readable output.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Interleaved => "interleaved",
            Self::TimeBlocked => "time-blocked",
            Self::Unknown => "unknown",
        }
    }
}

/// How far apart two groups sit on one benchmark and metric.
#[derive(Clone, Debug)]
pub(crate) struct MetricOffset {
    /// The qualified benchmark identity compared.
    pub(crate) benchmark: String,
    /// The metric compared.
    pub(crate) metric: MetricKind,
    /// Median of the baseline group's measurements.
    pub(crate) baseline_level: f64,
    /// Median of the incoming group's measurements.
    pub(crate) incoming_level: f64,
    /// `incoming_level - baseline_level`, in the metric's own units.
    pub(crate) absolute: f64,
    /// The offset as a fraction of the baseline level. A baseline at zero yields the
    /// offset's sign, so a move away from zero still reads as a full-scale change
    /// rather than a division by zero.
    pub(crate) relative: f64,
    /// Whether this offset reaches the merge tolerance and so blocks the merge.
    pub(crate) exceeds_tolerance: bool,
}

/// Two machine-key partitions a merge would splice into one series.
#[derive(Clone, Debug)]
pub(crate) struct GroupPair {
    /// Machine key of the group that starts earlier in history — the level the merged
    /// series would be read as moving *from*.
    pub(crate) baseline_key: String,
    /// Machine key of the group that starts later — the level it would move *to*.
    pub(crate) incoming_key: String,
    /// How the two groups sit relative to each other in commit order.
    pub(crate) interleaving: Interleaving,
    /// Maximal same-group stretches in commit order: two means the groups are fully
    /// separated, and the more there are the more finely they alternate. Zero when
    /// the order is unknown.
    pub(crate) blocks: usize,
    /// The per-benchmark, per-metric level offsets, ordered by benchmark then metric.
    /// Only pairs both groups measured appear.
    pub(crate) offsets: Vec<MetricOffset>,
}

impl GroupPair {
    /// The offsets that reach the merge tolerance, in report order.
    pub(crate) fn blocking_offsets(&self) -> impl Iterator<Item = &MetricOffset> {
        self.offsets
            .iter()
            .filter(|offset| offset.exceeds_tolerance)
    }
}

/// One destination partition several source partitions would merge into.
#[derive(Clone, Debug)]
pub(crate) struct PartitionMerge {
    /// The destination discriminant set the sources merge into.
    pub(crate) set: DiscriminantSet,
    /// The source machine keys contributing to it, ascending.
    pub(crate) source_keys: Vec<String>,
    /// Every pair of source groups, in ascending key order.
    pub(crate) pairs: Vec<GroupPair>,
}

/// The merge risk assessment of a whole rekey pass.
#[derive(Clone, Debug, Default)]
pub(crate) struct MergeAnalysis {
    /// The destination partitions that more than one source partition merges into.
    pub(crate) merges: Vec<PartitionMerge>,
}

impl MergeAnalysis {
    /// Every offset that reaches the merge tolerance, paired with the merge and group
    /// pair it was measured on.
    pub(crate) fn blocking(&self) -> Vec<(&PartitionMerge, &GroupPair, &MetricOffset)> {
        self.merges
            .iter()
            .flat_map(|merge| {
                merge.pairs.iter().flat_map(move |pair| {
                    pair.blocking_offsets()
                        .map(move |offset| (merge, pair, offset))
                })
            })
            .collect()
    }
}

/// The offset magnitudes a merge may reach on each metric family.
///
/// Derived from the detector's own practical-significance floors so the two never
/// drift apart: a change to what the detector considers practically significant moves
/// this gate with it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MergeTolerance {
    /// Smallest relative offset that blocks a merge.
    pub(crate) relative: f64,
    /// Smallest absolute offset that blocks a merge on an instruction or branch count.
    pub(crate) absolute_count: f64,
    /// Smallest absolute offset that blocks a merge on a timing metric, in nanoseconds.
    pub(crate) absolute_time: f64,
    /// Smallest absolute offset that blocks a merge on an allocation metric.
    pub(crate) absolute_alloc: f64,
}

impl MergeTolerance {
    /// The absolute floor that applies to `metric`.
    pub(crate) fn absolute_for(self, metric: MetricKind) -> f64 {
        match metric {
            MetricKind::WallTime | MetricKind::ProcessorTime => self.absolute_time,
            MetricKind::InstructionCount
            | MetricKind::ConditionalBranches
            | MetricKind::IndirectBranches => self.absolute_count,
            MetricKind::AllocatedBytes | MetricKind::AllocationCount => self.absolute_alloc,
        }
    }

    /// Whether an offset of `absolute` (relative magnitude `relative`) on `metric`
    /// blocks the merge.
    ///
    /// The two floors compose by conjunction, exactly as the detector composes them:
    /// a move must be both a meaningful fraction of the level *and* large enough in
    /// the metric's own units to carry information.
    pub(crate) fn is_exceeded(self, metric: MetricKind, absolute: f64, relative: f64) -> bool {
        relative.abs() >= self.relative && absolute.abs() >= self.absolute_for(metric)
    }
}

/// The merge tolerance, derived from the detector's default gating policy.
pub(crate) fn merge_offset_tolerance() -> MergeTolerance {
    let config = AnalysisConfig::default();
    MergeTolerance {
        relative: config.practical_relative * MERGE_TOLERANCE_FRACTION,
        absolute_count: config.practical_absolute_count * MERGE_TOLERANCE_FRACTION,
        absolute_time: config.practical_absolute_time * MERGE_TOLERANCE_FRACTION,
        absolute_alloc: config.practical_absolute_alloc * MERGE_TOLERANCE_FRACTION,
    }
}

/// Assesses every destination partition more than one source partition would merge
/// into.
///
/// `groups` maps each destination set to its source machine keys and the measurements
/// stored under them. A destination with a single source key is not a merge and is
/// dropped: its objects simply move.
pub(crate) fn analyze_merges(
    groups: &BTreeMap<DiscriminantSet, BTreeMap<String, Vec<MeasuredPoint>>>,
    tolerance: MergeTolerance,
) -> MergeAnalysis {
    const SOURCES_THAT_MERGE: usize = 2;

    let merges = groups
        .iter()
        .filter(|(_, sources)| sources.len() >= SOURCES_THAT_MERGE)
        .map(|(set, sources)| {
            let ordered: Vec<(&String, &Vec<MeasuredPoint>)> = sources.iter().collect();
            let mut pairs = Vec::new();
            for (index, (left_key, left_points)) in ordered.iter().enumerate() {
                for (right_key, right_points) in ordered.iter().skip(index.saturating_add(1)) {
                    pairs.push(compare_groups(
                        left_key,
                        left_points,
                        right_key,
                        right_points,
                        tolerance,
                    ));
                }
            }
            PartitionMerge {
                set: set.clone(),
                source_keys: ordered.iter().map(|(key, _)| (*key).clone()).collect(),
                pairs,
            }
        })
        .collect();

    MergeAnalysis { merges }
}

/// Compares two source groups, orienting the baseline to whichever starts earlier.
fn compare_groups(
    left_key: &str,
    left_points: &[MeasuredPoint],
    right_key: &str,
    right_points: &[MeasuredPoint],
    tolerance: MergeTolerance,
) -> GroupPair {
    // The baseline is the group history reaches first: a merged series is read as
    // moving *from* it, so it is what the level offset is expressed against. Groups
    // whose position is unknown keep the ascending key order the caller supplied.
    let left_first = first_position(left_points);
    let right_first = first_position(right_points);
    let swap = match (left_first, right_first) {
        (Some(left), Some(right)) => right < left,
        _ => false,
    };
    let (baseline_key, baseline_points, incoming_key, incoming_points) = if swap {
        (right_key, right_points, left_key, left_points)
    } else {
        (left_key, left_points, right_key, right_points)
    };

    let (interleaving, blocks) = classify_interleaving(baseline_points, incoming_points);

    GroupPair {
        baseline_key: baseline_key.to_owned(),
        incoming_key: incoming_key.to_owned(),
        interleaving,
        blocks,
        offsets: level_offsets(baseline_points, incoming_points, tolerance),
    }
}

/// The earliest first-parent position any of `points` sits at, or `None` when none of
/// them is on the target ref's first-parent line.
fn first_position(points: &[MeasuredPoint]) -> Option<usize> {
    points.iter().filter_map(|point| point.position).min()
}

/// Classifies how the two groups sit relative to each other in commit order, and
/// counts the maximal same-group stretches over that order.
///
/// The classification turns on whether the two groups' stretches of history overlap.
/// Disjoint stretches leave exactly one boundary between them, and any level offset
/// across that boundary reads exactly like a real change at it. Overlapping stretches
/// have no single boundary to carry a step, so the groups share a timeline and the
/// merge reunites points that already interleave — including the strongest case of
/// all, a commit measured under both keys.
///
/// The block count is reported alongside as the texture of that overlap: two means
/// fully separated, and the more blocks the more finely the groups alternate.
fn classify_interleaving(
    baseline_points: &[MeasuredPoint],
    incoming_points: &[MeasuredPoint],
) -> (Interleaving, usize) {
    let baseline: BTreeSet<usize> = baseline_points
        .iter()
        .filter_map(|point| point.position)
        .collect();
    let incoming: BTreeSet<usize> = incoming_points
        .iter()
        .filter_map(|point| point.position)
        .collect();
    let (Some(baseline_first), Some(baseline_last), Some(incoming_first), Some(incoming_last)) = (
        baseline.first(),
        baseline.last(),
        incoming.first(),
        incoming.last(),
    ) else {
        return (Interleaving::Unknown, 0);
    };

    // Walk the union of the positions in order, emitting one label per group present
    // at each position, and count the maximal same-label stretches.
    let mut labels: Vec<bool> = Vec::new();
    for position in baseline.union(&incoming) {
        if baseline.contains(position) {
            labels.push(true);
        }
        if incoming.contains(position) {
            labels.push(false);
        }
    }
    let blocks = labels.chunk_by(|left, right| left == right).count();

    let disjoint = baseline_last < incoming_first || incoming_last < baseline_first;
    let interleaving = if disjoint {
        Interleaving::TimeBlocked
    } else {
        Interleaving::Interleaved
    };
    (interleaving, blocks)
}

/// The per-benchmark, per-metric level offsets between two groups.
///
/// Only pairs both groups measured are comparable, so a benchmark present on one side
/// alone is skipped: it contributes no splice, since the merged series simply gains
/// points it did not have.
fn level_offsets(
    baseline_points: &[MeasuredPoint],
    incoming_points: &[MeasuredPoint],
    tolerance: MergeTolerance,
) -> Vec<MetricOffset> {
    let baseline_levels = levels(baseline_points);
    let incoming_levels = levels(incoming_points);

    baseline_levels
        .into_iter()
        .filter_map(|((benchmark, metric), baseline_level)| {
            let incoming_level = *incoming_levels.get(&(benchmark.clone(), metric))?;
            let absolute = incoming_level - baseline_level;
            let relative = relative_offset(baseline_level, absolute);
            Some(MetricOffset {
                benchmark,
                metric,
                baseline_level,
                incoming_level,
                absolute,
                relative,
                exceeds_tolerance: tolerance.is_exceeded(metric, absolute, relative),
            })
        })
        .collect()
}

/// The median measurement of each `(benchmark, metric)` a group covers.
fn levels(points: &[MeasuredPoint]) -> BTreeMap<(String, MetricKind), f64> {
    let mut by_series: BTreeMap<(String, MetricKind), Vec<f64>> = BTreeMap::new();
    for point in points {
        by_series
            .entry((point.benchmark.clone(), point.metric))
            .or_default()
            .push(point.value);
    }
    by_series
        .into_iter()
        .filter_map(|(series, values)| Some((series, median(&values)?)))
        .collect()
}

/// The offset as a fraction of the baseline level.
///
/// A baseline indistinguishable from zero has no scale to express a fraction of, so
/// the offset's own sign stands in: any move away from zero is a full-scale change,
/// and no move at all is no change.
fn relative_offset(baseline_level: f64, absolute: f64) -> f64 {
    if baseline_level.abs() <= f64::EPSILON {
        absolute.signum() * f64::from(u8::from(absolute != 0.0))
    } else {
        absolute / baseline_level
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use cbh_model::{Engine, MachineKey, TargetTriple};

    use super::*;

    /// Builds a measurement of the `bench` benchmark's instruction count.
    fn count_point(position: usize, value: f64) -> MeasuredPoint {
        MeasuredPoint {
            benchmark: "pkg/bench".to_owned(),
            metric: MetricKind::InstructionCount,
            value,
            position: Some(position),
        }
    }

    /// Builds a measurement whose commit is not on the first-parent line.
    fn unpositioned_point(value: f64) -> MeasuredPoint {
        MeasuredPoint {
            position: None,
            ..count_point(0, value)
        }
    }

    fn destination() -> DiscriminantSet {
        DiscriminantSet::new(
            Engine::Callgrind,
            &TargetTriple::from("x86_64-unknown-linux-gnu"),
            &MachineKey::from("new"),
        )
    }

    /// Assembles the destination-keyed group map `analyze_merges` consumes.
    fn grouped(
        sources: &[(&str, Vec<MeasuredPoint>)],
    ) -> BTreeMap<DiscriminantSet, BTreeMap<String, Vec<MeasuredPoint>>> {
        let mut by_source: BTreeMap<String, Vec<MeasuredPoint>> = BTreeMap::new();
        for (key, points) in sources {
            by_source.insert((*key).to_owned(), points.clone());
        }
        let mut groups = BTreeMap::new();
        groups.insert(destination(), by_source);
        groups
    }

    #[test]
    fn tolerance_is_half_the_detector_practical_floors() {
        let config = AnalysisConfig::default();
        let tolerance = merge_offset_tolerance();
        assert!((tolerance.relative - config.practical_relative / 2.0).abs() < f64::EPSILON);
        assert!(
            (tolerance.absolute_count - config.practical_absolute_count / 2.0).abs() < f64::EPSILON
        );
        assert!(
            (tolerance.absolute_time - config.practical_absolute_time / 2.0).abs() < f64::EPSILON
        );
        assert!(
            (tolerance.absolute_alloc - config.practical_absolute_alloc / 2.0).abs() < f64::EPSILON
        );
    }

    #[test]
    fn tolerance_maps_every_metric_kind_to_its_own_absolute_floor() {
        let tolerance = merge_offset_tolerance();
        for metric in MetricKind::ALL {
            let expected = match metric {
                MetricKind::WallTime | MetricKind::ProcessorTime => tolerance.absolute_time,
                MetricKind::InstructionCount
                | MetricKind::ConditionalBranches
                | MetricKind::IndirectBranches => tolerance.absolute_count,
                MetricKind::AllocatedBytes | MetricKind::AllocationCount => {
                    tolerance.absolute_alloc
                }
            };
            assert!(
                (tolerance.absolute_for(metric) - expected).abs() < f64::EPSILON,
                "{metric:?}"
            );
        }
    }

    #[test]
    fn tolerance_requires_both_floors_to_be_reached() {
        let tolerance = merge_offset_tolerance();
        // A large fraction of a tiny level: below the absolute floor, so it cannot
        // produce a finding however large the percentage reads.
        assert!(!tolerance.is_exceeded(MetricKind::InstructionCount, 1.0, 1.0));
        // A large absolute move that is a negligible fraction of a huge level.
        assert!(!tolerance.is_exceeded(MetricKind::InstructionCount, 100.0, 0.001));
        // Both floors reached.
        assert!(tolerance.is_exceeded(MetricKind::InstructionCount, 100.0, 0.5));
    }

    #[test]
    fn a_single_source_partition_is_not_a_merge() {
        let groups = grouped(&[("only", vec![count_point(0, 100.0)])]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        assert!(analysis.merges.is_empty());
    }

    #[test]
    fn alternating_groups_are_interleaved() {
        let groups = grouped(&[
            (
                "aaa",
                vec![
                    count_point(0, 100.0),
                    count_point(2, 100.0),
                    count_point(4, 100.0),
                ],
            ),
            (
                "bbb",
                vec![
                    count_point(1, 100.0),
                    count_point(3, 100.0),
                    count_point(5, 100.0),
                ],
            ),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        let pair = &analysis.merges[0].pairs[0];
        assert_eq!(pair.interleaving, Interleaving::Interleaved);
        assert_eq!(pair.blocks, 6);
    }

    #[test]
    fn disjoint_stretches_are_time_blocked() {
        let groups = grouped(&[
            (
                "aaa",
                vec![
                    count_point(0, 100.0),
                    count_point(1, 100.0),
                    count_point(2, 100.0),
                ],
            ),
            (
                "bbb",
                vec![
                    count_point(3, 100.0),
                    count_point(4, 100.0),
                    count_point(5, 100.0),
                ],
            ),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        let pair = &analysis.merges[0].pairs[0];
        assert_eq!(pair.interleaving, Interleaving::TimeBlocked);
        assert_eq!(pair.blocks, 2);
    }

    #[test]
    fn one_group_inside_the_other_is_interleaved() {
        let groups = grouped(&[
            (
                "aaa",
                vec![
                    count_point(0, 100.0),
                    count_point(1, 100.0),
                    count_point(3, 100.0),
                    count_point(4, 100.0),
                ],
            ),
            ("bbb", vec![count_point(2, 100.0)]),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        let pair = &analysis.merges[0].pairs[0];
        assert_eq!(pair.interleaving, Interleaving::Interleaved);
        assert_eq!(pair.blocks, 3);
    }

    #[test]
    fn a_shared_commit_counts_toward_both_groups() {
        // Both keys recorded the same commit, which only one machine measuring under
        // two keys can produce, so the groups cannot be time-blocked.
        let groups = grouped(&[
            ("aaa", vec![count_point(0, 100.0), count_point(1, 100.0)]),
            ("bbb", vec![count_point(1, 100.0), count_point(2, 100.0)]),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        let pair = &analysis.merges[0].pairs[0];
        assert_eq!(pair.interleaving, Interleaving::Interleaved);
        assert_eq!(pair.blocks, 2);
    }

    #[test]
    fn touching_stretches_are_time_blocked() {
        // Adjacent but disjoint stretches: the boundary between position 1 and 2 is
        // exactly where a merged series would show a step.
        let groups = grouped(&[
            ("aaa", vec![count_point(0, 100.0), count_point(1, 100.0)]),
            ("bbb", vec![count_point(2, 100.0), count_point(3, 100.0)]),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        let pair = &analysis.merges[0].pairs[0];
        assert_eq!(pair.interleaving, Interleaving::TimeBlocked);
        assert_eq!(pair.blocks, 2);
    }

    #[test]
    fn an_unplaced_group_leaves_the_order_unknown() {
        let groups = grouped(&[
            ("aaa", vec![count_point(0, 100.0)]),
            ("bbb", vec![unpositioned_point(100.0)]),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        let pair = &analysis.merges[0].pairs[0];
        assert_eq!(pair.interleaving, Interleaving::Unknown);
        assert_eq!(pair.blocks, 0);
    }

    #[test]
    fn the_baseline_is_the_group_that_starts_earlier() {
        // `zzz` sorts after `aaa` but reaches history first, so it is the baseline.
        let groups = grouped(&[
            ("aaa", vec![count_point(5, 100.0)]),
            ("zzz", vec![count_point(1, 100.0)]),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        let pair = &analysis.merges[0].pairs[0];
        assert_eq!(pair.baseline_key, "zzz");
        assert_eq!(pair.incoming_key, "aaa");
    }

    #[test]
    fn an_unordered_pair_keeps_the_ascending_key_order() {
        let groups = grouped(&[
            ("aaa", vec![unpositioned_point(100.0)]),
            ("zzz", vec![unpositioned_point(100.0)]),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        let pair = &analysis.merges[0].pairs[0];
        assert_eq!(pair.baseline_key, "aaa");
        assert_eq!(pair.incoming_key, "zzz");
    }

    #[test]
    fn groups_starting_at_the_same_commit_keep_the_ascending_key_order() {
        // Neither group reaches history first, so there is nothing to prefer and the
        // caller's ascending key order stands.
        let groups = grouped(&[
            ("aaa", vec![count_point(0, 100.0), count_point(3, 100.0)]),
            ("zzz", vec![count_point(0, 100.0), count_point(1, 100.0)]),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        let pair = &analysis.merges[0].pairs[0];
        assert_eq!(pair.baseline_key, "aaa");
        assert_eq!(pair.incoming_key, "zzz");
    }

    #[test]
    fn the_classification_does_not_depend_on_which_group_is_named_first() {
        // The classification asks whether the two stretches overlap, which is a
        // property of the pair rather than of the order it is presented in.
        let early = [count_point(0, 100.0), count_point(1, 100.0)];
        let late = [count_point(2, 100.0), count_point(3, 100.0)];
        assert_eq!(
            classify_interleaving(&late, &early),
            (Interleaving::TimeBlocked, 2)
        );
        assert_eq!(
            classify_interleaving(&early, &late),
            (Interleaving::TimeBlocked, 2)
        );
    }

    #[test]
    fn stretches_that_meet_at_one_commit_overlap() {
        // The groups share position 1, so no single boundary separates them however
        // the pair is ordered. The block count is read off the baseline-first walk and
        // so does depend on the order; the classification does not.
        let earlier = [count_point(0, 100.0), count_point(1, 100.0)];
        let later = [count_point(1, 100.0), count_point(2, 100.0)];
        assert_eq!(
            classify_interleaving(&later, &earlier).0,
            Interleaving::Interleaved
        );
        assert_eq!(
            classify_interleaving(&earlier, &later).0,
            Interleaving::Interleaved
        );
    }

    #[test]
    fn matching_levels_produce_no_blocking_offset() {
        let groups = grouped(&[
            ("aaa", vec![count_point(0, 100.0), count_point(2, 102.0)]),
            ("bbb", vec![count_point(1, 101.0), count_point(3, 101.0)]),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        assert!(analysis.blocking().is_empty());
        let offset = &analysis.merges[0].pairs[0].offsets[0];
        assert!((offset.baseline_level - 101.0).abs() < f64::EPSILON);
        assert!((offset.incoming_level - 101.0).abs() < f64::EPSILON);
        assert!(offset.absolute.abs() < f64::EPSILON);
    }

    #[test]
    fn a_systematic_level_offset_blocks_the_merge() {
        let groups = grouped(&[
            ("aaa", vec![count_point(0, 100.0), count_point(1, 100.0)]),
            ("bbb", vec![count_point(2, 200.0), count_point(3, 200.0)]),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        let blocking = analysis.blocking();
        assert_eq!(blocking.len(), 1);
        let (_, _, offset) = blocking[0];
        assert!((offset.absolute - 100.0).abs() < f64::EPSILON);
        assert!((offset.relative - 1.0).abs() < f64::EPSILON);
        assert!(offset.exceeds_tolerance);
    }

    #[test]
    fn medians_absorb_a_single_outlying_run() {
        // One wild point per group must not move the compared level: the medians of
        // both groups stay at 100.
        let groups = grouped(&[
            (
                "aaa",
                vec![
                    count_point(0, 100.0),
                    count_point(1, 100.0),
                    count_point(2, 9_000.0),
                ],
            ),
            (
                "bbb",
                vec![
                    count_point(3, 100.0),
                    count_point(4, 100.0),
                    count_point(5, 9_000.0),
                ],
            ),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        assert!(analysis.blocking().is_empty());
    }

    #[test]
    fn a_benchmark_only_one_group_measured_is_not_compared() {
        let mut only_incoming = count_point(3, 500.0);
        only_incoming.benchmark = "pkg/other".to_owned();
        let groups = grouped(&[
            ("aaa", vec![count_point(0, 100.0)]),
            ("bbb", vec![count_point(1, 100.0), only_incoming]),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        let offsets = &analysis.merges[0].pairs[0].offsets;
        assert_eq!(offsets.len(), 1);
        assert_eq!(offsets[0].benchmark, "pkg/bench");
    }

    #[test]
    fn three_sources_produce_every_pair() {
        let groups = grouped(&[
            ("aaa", vec![count_point(0, 100.0)]),
            ("bbb", vec![count_point(1, 100.0)]),
            ("ccc", vec![count_point(2, 100.0)]),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        let merge = &analysis.merges[0];
        assert_eq!(merge.source_keys, vec!["aaa", "bbb", "ccc"]);
        assert_eq!(merge.pairs.len(), 3);
    }

    #[test]
    fn a_zero_baseline_reads_any_move_as_a_full_scale_change() {
        assert!((relative_offset(0.0, 4.0) - 1.0).abs() < f64::EPSILON);
        assert!((relative_offset(0.0, -4.0) + 1.0).abs() < f64::EPSILON);
        assert!(relative_offset(0.0, 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn interleaving_labels_are_stable() {
        assert_eq!(Interleaving::Interleaved.as_str(), "interleaved");
        assert_eq!(Interleaving::TimeBlocked.as_str(), "time-blocked");
        assert_eq!(Interleaving::Unknown.as_str(), "unknown");
    }
}
