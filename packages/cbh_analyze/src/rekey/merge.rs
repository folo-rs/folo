//! Deciding whether merging two machine-key partitions is safe.
//!
//! Rekeying splices two sets of measurements into a single series. When the two
//! partitions really are the same machine, their measurement levels agree and the
//! splice is invisible. When they systematically differ, the merge *manufactures* a
//! step change at the splice point, and the next analysis reports it as a regression —
//! precisely the false positive the migration exists to remove. This module quantifies
//! that risk before anything is written.
//!
//! The **level offset** measures it. That is how far apart the two groups sit on a
//! given benchmark and metric, compared as medians so a single outlying run cannot
//! dominate. What a splice can turn into a visible step is only the *systematic* part
//! of those offsets — the amount by which the whole family of shared series moves
//! together — so the verdict is read off their median, taken over the offsets large
//! enough in their metric's own units to carry information about a level at all. The
//! scatter around that median is not a step: independent per-benchmark disagreement
//! becomes ordinary within-series noise once the two groups sit in one series, where
//! the detector's own gates are what judge it.
//!
//! Reading each benchmark's own offset as its own verdict cannot work, because such a
//! gate has no multiplicity control. The tolerance is a fraction of what one benchmark
//! must move to be worth reporting, while per-benchmark measurement noise is of the
//! same order, so across a family of hundreds the probability that *some* member
//! wanders past the tolerance approaches one whether or not the two partitions differ
//! at all. A gate that fires on every real input teaches its operator to always pass
//! the override, and the override switches the gate off for the genuinely different
//! machine too, so a per-benchmark gate ends up protecting nothing.
//!
//! A pair sharing exactly one informative offset is the degenerate case: the median
//! *is* that offset, so the gate stays exactly as strict as reading that one series
//! directly. That is the wanted outcome rather than an accident of the formula — with
//! no family to average over there is no evidence that a disagreement is scatter, so
//! nothing licenses the more permissive reading.
//!
//! Offsets that lie beyond the tolerance individually are still reported, as the
//! benchmarks whose own series may gain a visible step at the splice. They inform an
//! operator's reading; they do not decide.
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
/// systematic level offset may reach before `rekey` refuses to merge.
///
/// The detector only reports a move that clears *both* its relative floor and the
/// absolute floor for the metric's kind, so an offset below those floors cannot
/// produce a finding no matter where the splice lands. Half of them is therefore a
/// deliberately conservative margin: an offset that passes leaves a full factor of two
/// of headroom before the merged series could raise anything, which covers the
/// detector composing the splice with the series' own movement rather than measuring
/// it in isolation.
///
/// The margin can afford to be that tight because it is applied to a median over a
/// whole family of benchmarks, where per-benchmark noise cancels rather than
/// accumulates. The same fraction read against each benchmark separately would refuse
/// almost every real merge on scatter alone.
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
    /// Whether this offset lies beyond the merge tolerance on its own.
    ///
    /// Informational: it marks a benchmark whose own series may gain a visible step at
    /// the splice, which is worth an operator's attention. It is not the merge
    /// verdict — that is [`GroupPair::manufactures_step`], read off the whole family.
    pub(crate) beyond_tolerance: bool,
}

/// The systematic component of a pair's level offsets: how far the two groups sit
/// apart as a family, which is the only part of a disagreement a merge can turn into a
/// visible step change.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SystematicOffset {
    /// The median of the contributing relative offsets.
    pub(crate) relative: f64,
    /// How many of the pair's shared `(benchmark, metric)` offsets the median was taken
    /// over — the evidence behind the reading. Always at least one: a pair with nothing
    /// to take a median over reports no systematic offset at all rather than an empty
    /// one.
    pub(crate) offsets: usize,
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
    /// How far apart the two groups sit as a family, or `None` when not one shared
    /// offset is a large enough move in its metric's own units to say anything about a
    /// level. A pair with no systematic offset has nothing that could manufacture a
    /// step, so it never blocks.
    pub(crate) systematic: Option<SystematicOffset>,
    /// Whether the systematic offset reaches the merge tolerance, so splicing these
    /// two partitions would manufacture a step change. This is the merge verdict.
    pub(crate) manufactures_step: bool,
}

impl GroupPair {
    /// The offsets that lie beyond the merge tolerance on their own, in report order.
    ///
    /// These name the benchmarks whose own series may gain a visible step at the
    /// splice, which is worth reporting. They do not decide the merge:
    /// [`Self::manufactures_step`] does, and it reads the family rather than its
    /// members.
    pub(crate) fn outlying_offsets(&self) -> impl Iterator<Item = &MetricOffset> {
        self.offsets.iter().filter(|offset| offset.beyond_tolerance)
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
    /// Every pair of source groups whose systematic offset blocks the merge, paired
    /// with the destination partition it would merge into.
    pub(crate) fn blocking(&self) -> Vec<(&PartitionMerge, &GroupPair)> {
        self.merges
            .iter()
            .flat_map(|merge| {
                merge
                    .pairs
                    .iter()
                    .filter(|pair| pair.manufactures_step)
                    .map(move |pair| (merge, pair))
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
    /// Smallest systematic relative offset that blocks a merge.
    pub(crate) relative: f64,
    /// Smallest absolute offset that carries information on an instruction or branch
    /// count.
    pub(crate) absolute_count: f64,
    /// Smallest absolute offset that carries information on a timing metric, in
    /// nanoseconds.
    pub(crate) absolute_time: f64,
    /// Smallest absolute offset that carries information on an allocation metric.
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

    /// Whether an offset of `absolute` on `metric` is a large enough move in the
    /// metric's own units to say anything about where the two groups sit.
    ///
    /// Below the floor the move is too small to mean anything: it is neither evidence
    /// of a difference nor evidence of agreement, and it stays out of the systematic
    /// reading entirely.
    pub(crate) fn carries_information(self, metric: MetricKind, absolute: f64) -> bool {
        absolute.abs() >= self.absolute_for(metric)
    }

    /// Whether an offset of `absolute` (relative magnitude `relative`) on `metric` lies
    /// beyond the tolerance read on its own.
    ///
    /// The two floors compose by conjunction, exactly as the detector composes them:
    /// a move must be both a meaningful fraction of the level *and* large enough in
    /// the metric's own units to carry information. This is the per-benchmark reading,
    /// which is reported but does not decide a merge.
    pub(crate) fn is_beyond(self, metric: MetricKind, absolute: f64, relative: f64) -> bool {
        relative.abs() >= self.relative && self.carries_information(metric, absolute)
    }

    /// Whether a systematic relative offset of `relative` blocks a merge.
    ///
    /// Only the relative floor applies here. The absolute floor already chose which
    /// offsets were informative enough to enter the median, and a median of fractions
    /// has no units left to compare against a magnitude.
    pub(crate) fn blocks_merge(self, relative: f64) -> bool {
        relative.abs() >= self.relative
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
    let offsets = level_offsets(baseline_points, incoming_points, tolerance);
    let systematic = systematic_offset(&offsets, tolerance);

    GroupPair {
        baseline_key: baseline_key.to_owned(),
        incoming_key: incoming_key.to_owned(),
        interleaving,
        blocks,
        offsets,
        systematic,
        manufactures_step: systematic
            .is_some_and(|systematic| tolerance.blocks_merge(systematic.relative)),
    }
}

/// How far apart two groups sit as a family: the median of the relative offsets of the
/// shared series whose move clears its metric's absolute floor.
///
/// Offsets below that floor are left out rather than counted as agreement. A move too
/// small to mean anything is not evidence that the two groups sit at the same level,
/// and a crowd of them pulling the median toward zero would hide a family that really
/// did move. When no offset clears its floor at all there is nothing a splice could
/// turn into a step, and the pair has no systematic offset to report.
///
/// The median rather than the mean, so a minority of benchmarks whose *code* changed
/// while one group's stretch of history was active cannot drag the verdict with them.
fn systematic_offset(
    offsets: &[MetricOffset],
    tolerance: MergeTolerance,
) -> Option<SystematicOffset> {
    let informative: Vec<f64> = offsets
        .iter()
        .filter(|offset| tolerance.carries_information(offset.metric, offset.absolute))
        .map(|offset| offset.relative)
        .collect();
    Some(SystematicOffset {
        relative: median(&informative)?,
        offsets: informative.len(),
    })
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
                beyond_tolerance: tolerance.is_beyond(metric, absolute, relative),
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
    use crate::rekey::production_merges::{
        ALL_THE_TIME_ARM_WINDOWS, ALLOC_TRACKER_X64_LINUX, ALLOC_TRACKER_X64_WINDOWS,
        CAPTURED_PAIRS, CRITERION_ARM_WINDOWS, CapturedPair, captured_store,
    };

    /// Builds a measurement of the `bench` benchmark's instruction count.
    fn count_point(position: usize, value: f64) -> MeasuredPoint {
        MeasuredPoint {
            benchmark: "pkg/bench".to_owned(),
            metric: MetricKind::InstructionCount,
            value,
            position: Some(position),
        }
    }

    /// Builds a measurement of the instruction count of the `index`th benchmark, so a
    /// pair can share a whole family of them.
    fn family_point(index: usize, position: usize, value: f64) -> MeasuredPoint {
        MeasuredPoint {
            benchmark: format!("pkg/bench{index}"),
            ..count_point(position, value)
        }
    }

    /// One group's measurements of a family of benchmarks, one point each, all at the
    /// same commit position.
    fn family(position: usize, values: &[f64]) -> Vec<MeasuredPoint> {
        values
            .iter()
            .enumerate()
            .map(|(index, value)| family_point(index, position, *value))
            .collect()
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
    fn the_per_benchmark_reading_requires_both_floors_to_be_reached() {
        let tolerance = merge_offset_tolerance();
        // A large fraction of a tiny level: below the absolute floor, so it cannot
        // produce a finding however large the percentage reads.
        assert!(!tolerance.is_beyond(MetricKind::InstructionCount, 1.0, 1.0));
        // A large absolute move that is a negligible fraction of a huge level.
        assert!(!tolerance.is_beyond(MetricKind::InstructionCount, 100.0, 0.001));
        // Both floors reached.
        assert!(tolerance.is_beyond(MetricKind::InstructionCount, 100.0, 0.5));
    }

    #[test]
    fn an_offset_under_the_absolute_floor_carries_no_information() {
        let tolerance = merge_offset_tolerance();
        let floor = tolerance.absolute_for(MetricKind::InstructionCount);
        assert!(!tolerance.carries_information(MetricKind::InstructionCount, 1.0));
        assert!(tolerance.carries_information(MetricKind::InstructionCount, 100.0));
        // The floor itself admits the offset: it is what a move must reach, not what
        // it must exceed.
        assert!(tolerance.carries_information(MetricKind::InstructionCount, floor));
        // The floor is a magnitude, so the direction of the move is irrelevant.
        assert!(tolerance.carries_information(MetricKind::InstructionCount, -100.0));
    }

    #[test]
    fn the_merge_verdict_reads_the_relative_floor_alone() {
        let tolerance = merge_offset_tolerance();
        assert!(!tolerance.blocks_merge(tolerance.relative / 2.0));
        assert!(tolerance.blocks_merge(tolerance.relative));
        assert!(tolerance.blocks_merge(-tolerance.relative));
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
    fn matching_levels_produce_no_blocking_pair() {
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
        let (_, pair) = blocking[0];
        let systematic = pair
            .systematic
            .expect("the offset clears the absolute floor");
        assert!((systematic.relative - 1.0).abs() < f64::EPSILON);
        assert_eq!(systematic.offsets, 1);
        assert!(pair.manufactures_step);
    }

    #[test]
    fn scatter_in_both_directions_around_zero_does_not_block() {
        // Six benchmarks whose individual offsets run from -5% to +5% — every one of
        // them far beyond the tolerance — but symmetric, so the family as a whole did
        // not move and a merge cannot manufacture a step from it. This is what a pair
        // of partitions that really is one machine looks like: per-benchmark
        // measurement noise, centred on zero.
        let groups = grouped(&[
            ("aaa", family(0, &[1_000.0; 6])),
            (
                "bbb",
                family(1, &[1_050.0, 950.0, 1_040.0, 960.0, 1_030.0, 970.0]),
            ),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        assert!(analysis.blocking().is_empty());

        let pair = &analysis.merges[0].pairs[0];
        let systematic = pair.systematic.expect("every offset clears the floor");
        assert!(systematic.relative.abs() < f64::EPSILON);
        assert_eq!(systematic.offsets, 6);
        // Every one of them is an outlier read on its own, which is exactly why a
        // per-benchmark gate could not tell this pair from a shifted one.
        assert_eq!(pair.outlying_offsets().count(), 6);
    }

    #[test]
    fn the_same_scatter_shifted_as_a_family_blocks() {
        // The scatter of the previous test with a systematic +10% laid over it: the
        // spread is unchanged, and it is the shift alone that blocks.
        let groups = grouped(&[
            ("aaa", family(0, &[1_000.0; 6])),
            (
                "bbb",
                family(1, &[1_150.0, 1_050.0, 1_140.0, 1_060.0, 1_130.0, 1_070.0]),
            ),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        assert_eq!(analysis.blocking().len(), 1);

        let pair = &analysis.merges[0].pairs[0];
        let systematic = pair.systematic.expect("every offset clears the floor");
        assert!((systematic.relative - 0.1).abs() < f64::EPSILON);
        assert_eq!(systematic.offsets, 6);
    }

    #[test]
    fn a_minority_of_large_outliers_does_not_flip_the_verdict() {
        // Five benchmarks agree to within a few counts and two moved by half; the
        // majority decides, because two benchmarks whose code changed while one
        // group's stretch of history was active say nothing about the machines.
        let baseline = [100_000.0; 7];
        let incoming = [
            100_003.0, 100_003.0, 100_003.0, 100_003.0, 100_003.0, 150_000.0, 150_000.0,
        ];
        let groups = grouped(&[("aaa", family(0, &baseline)), ("bbb", family(1, &incoming))]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        assert!(analysis.blocking().is_empty());

        let pair = &analysis.merges[0].pairs[0];
        let systematic = pair.systematic.expect("every offset clears the floor");
        assert!(systematic.relative < merge_offset_tolerance().relative);
        assert_eq!(systematic.offsets, 7);
        assert_eq!(pair.outlying_offsets().count(), 2);
    }

    #[test]
    fn a_lone_shared_benchmark_decides_the_merge_by_itself() {
        // With one informative benchmark the median is that benchmark, so the gate is
        // exactly as conservative as reading it directly. There is no family to
        // average over, so nothing licenses a more permissive reading.
        let blocked = grouped(&[
            ("aaa", vec![count_point(0, 1_000.0)]),
            ("bbb", vec![count_point(1, 1_030.0)]),
        ]);
        let analysis = analyze_merges(&blocked, merge_offset_tolerance());
        assert_eq!(analysis.blocking().len(), 1);
        assert_eq!(analysis.merges[0].pairs[0].outlying_offsets().count(), 1);

        let cleared = grouped(&[
            ("aaa", vec![count_point(0, 1_000.0)]),
            ("bbb", vec![count_point(1, 1_010.0)]),
        ]);
        let analysis = analyze_merges(&cleared, merge_offset_tolerance());
        assert!(analysis.blocking().is_empty());
        assert_eq!(analysis.merges[0].pairs[0].outlying_offsets().count(), 0);
    }

    #[test]
    fn offsets_under_the_absolute_floor_leave_no_systematic_offset() {
        // Every offset is 2% of its level — beyond the relative tolerance — but only
        // two instruction counts, which is a move the metric cannot resolve. Nothing
        // informative remains, so the pair reports no systematic offset at all rather
        // than a fabricated zero, and it cannot block.
        let groups = grouped(&[
            ("aaa", family(0, &[100.0; 4])),
            ("bbb", family(1, &[102.0; 4])),
        ]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        assert!(analysis.blocking().is_empty());

        let pair = &analysis.merges[0].pairs[0];
        assert!(pair.systematic.is_none());
        assert!(!pair.manufactures_step);
        assert_eq!(pair.outlying_offsets().count(), 0);
    }

    #[test]
    fn an_unresolvable_offset_is_not_counted_as_agreement() {
        // Five benchmarks land on exactly the same level and two moved by half. A move
        // of nothing is not a move the metric resolved, so it says nothing about where
        // the groups sit; letting a crowd of such ties pull the median to zero would
        // clear a merge that manufactures a step on everything that did move.
        let mut baseline = family(0, &[1_000.0; 5]);
        let mut incoming = family(1, &[1_000.0; 5]);
        for index in 5..7 {
            baseline.push(family_point(index, 0, 1_000.0));
            incoming.push(family_point(index, 1, 1_500.0));
        }
        let groups = grouped(&[("aaa", baseline), ("bbb", incoming)]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        assert_eq!(analysis.blocking().len(), 1);

        let systematic = analysis.merges[0].pairs[0]
            .systematic
            .expect("the two resolvable offsets carry the reading");
        assert!((systematic.relative - 0.5).abs() < f64::EPSILON);
        assert_eq!(systematic.offsets, 2);
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

    /// How far a systematic offset recomputed from the captured levels may sit from the
    /// figure the production run reported.
    ///
    /// A capture prints every level at fifteen significant digits, which denotes the
    /// measured `f64` exactly but leaves a recomputation agreeing with the reported
    /// offset to about twelve digits rather than to the bit. The slack is nine orders of
    /// magnitude below the tolerance the offset is read against, so it cannot conceal a
    /// change of verdict.
    const CAPTURE_ROUNDING: f64 = 1e-12;

    /// The relative offset of `threadpool_100` between the two ARM64 Windows
    /// `all_the_time` partitions: the incoming partition measures it 49.4% faster.
    const THREADPOOL_100_OFFSET: f64 = -0.49441907748923536;

    /// Renders a relative offset the way a report states it, so a failure says how far
    /// production sits from where it sat.
    fn percent(relative: f64) -> String {
        format!("{:+.6}%", relative * 100.0)
    }

    /// Names a captured pair's assessment within `analysis`.
    fn assessment<'a>(analysis: &'a MergeAnalysis, captured: &CapturedPair) -> &'a GroupPair {
        let destination = captured.destination();
        let merge = analysis
            .merges
            .iter()
            .find(|merge| merge.set == destination)
            .unwrap_or_else(|| panic!("{} has no assessment", captured.label()));
        merge.pairs.first().unwrap_or_else(|| {
            panic!(
                "{} has two source partitions, so it has a pair",
                captured.label()
            )
        })
    }

    #[test]
    // Replays roughly a thousand measurements, which is more than Miri finishes promptly.
    #[cfg_attr(miri, ignore)]
    fn the_production_store_merges_without_an_override() {
        // The whole capture in one pass, as `rekey` reads it: seven pairs of partitions
        // across four engines and three target triples, every one of them two readings
        // of a single machine. Production migrates cleanly, so none of them may need
        // `--allow-level-shift` — an operator who has to pass it once passes it always,
        // and it switches the gate off for the genuinely different machine too.
        let tolerance = merge_offset_tolerance();
        let analysis = analyze_merges(&captured_store(), tolerance);

        let refused: Vec<String> = analysis
            .blocking()
            .into_iter()
            .map(|(merge, pair)| {
                let relative = pair
                    .systematic
                    .map_or(f64::NAN, |systematic| systematic.relative);
                let captured = CAPTURED_PAIRS
                    .iter()
                    .find(|captured| captured.destination() == merge.set);
                let measured = captured.map_or_else(
                    || "no capture".to_owned(),
                    |captured| percent(captured.systematic_relative),
                );
                format!(
                    "{}/{} ({} -> {}) at {} where production measured {measured}",
                    merge.set.engine.as_str(),
                    merge.set.target_triple.as_str(),
                    pair.baseline_key,
                    pair.incoming_key,
                    percent(relative),
                )
            })
            .collect();
        assert!(
            refused.is_empty(),
            "the production store migrates with no override; these pairs now refuse it \
             against a tolerance of {}: {refused:?}",
            percent(tolerance.relative),
        );
        assert_eq!(
            analysis.merges.len(),
            CAPTURED_PAIRS.len(),
            "the capture holds one merging destination partition per pair"
        );

        for captured in CAPTURED_PAIRS {
            let pair = assessment(&analysis, captured);
            let systematic = pair
                .systematic
                .unwrap_or_else(|| panic!("{} has informative offsets", captured.label()));

            assert_eq!(
                pair.offsets.len(),
                captured.shared_levels(),
                "{}: the capture's shared measurements no longer all reach the assessment",
                captured.label(),
            );
            assert!(
                (systematic.relative - captured.systematic_relative).abs() < CAPTURE_ROUNDING,
                "{}: the pair now reads {} where production measured {}",
                captured.label(),
                percent(systematic.relative),
                percent(captured.systematic_relative),
            );
            assert_eq!(
                systematic.offsets,
                captured.informative_offsets,
                "{}: {} shared offsets now clear their metric's absolute floor where \
                 production found {}",
                captured.label(),
                systematic.offsets,
                captured.informative_offsets,
            );
            assert_eq!(
                pair.outlying_offsets().count(),
                captured.outlying_offsets,
                "{}: {} offsets now lie beyond the tolerance on their own where \
                 production found {}",
                captured.label(),
                pair.outlying_offsets().count(),
                captured.outlying_offsets,
            );
            assert_eq!(
                pair.baseline_key,
                captured.baseline_key,
                "{}: the offset is expressed against the wrong partition",
                captured.label(),
            );
            assert_eq!(
                pair.incoming_key,
                captured.incoming_key,
                "{}",
                captured.label()
            );
            assert_eq!(
                pair.interleaving,
                captured.commit_order.interleaving(),
                "{}: the reconstructed placement no longer classifies as the pattern \
                 production classified this pair as",
                captured.label(),
            );
        }
    }

    #[test]
    // Replays 294 shared measurements at three commits each, which is more than Miri
    // finishes promptly.
    #[cfg_attr(miri, ignore)]
    fn the_production_arm_windows_criterion_partitions_are_one_machine() {
        // The store's largest merge, and the one the machine-key format exists to make
        // possible: 294 shared Criterion measurements of a single GitHub-hosted ARM64
        // Windows runner, split across two partitions because one of its four processors
        // calibrated at 10681 rather than 10678 on some boots. The two sit 0.077% apart —
        // a twentieth of the tolerance — so they are the same machine and must merge.
        let captured = &CRITERION_ARM_WINDOWS;
        let tolerance = merge_offset_tolerance();
        let pair = captured.assess();
        let systematic = pair.systematic.unwrap_or_else(|| {
            panic!("123 of the pair's 294 shared offsets clear their absolute floor")
        });

        assert!(
            !pair.manufactures_step,
            "the ARM64 Windows Criterion partitions sit {} apart — production measured \
             {} — against a tolerance of {}, so a merge cannot manufacture a step; \
             refusing them leaves one runner's history in two stretches, neither long \
             enough to judge",
            percent(systematic.relative),
            percent(captured.systematic_relative),
            percent(tolerance.relative),
        );
        // Read one benchmark at a time, 46 of the 123 informative offsets exceed the
        // tolerance, because per-benchmark wall-clock variation on a shared cloud runner
        // is of the same order as the tolerance. A per-benchmark threshold therefore
        // refuses this pair, so the gate reads the pair and reports the members.
        assert_eq!(
            pair.outlying_offsets().count(),
            captured.outlying_offsets,
            "{} of this pair's offsets lie beyond the tolerance individually where \
             production found {}, while the pair as a whole reads {}",
            pair.outlying_offsets().count(),
            captured.outlying_offsets,
            percent(systematic.relative),
        );
        assert_eq!(
            pair.interleaving,
            Interleaving::Interleaved,
            "the two partitions alternate through history, which one machine whose key \
             wobbles produces and two successive machines cannot",
        );
    }

    #[test]
    fn a_production_outlier_of_half_the_level_does_not_flip_the_verdict() {
        // The ARM64 Windows `all_the_time` partitions of the same runner. `threadpool_100`
        // moved 49.4%, which is a code change that landed while one partition was active
        // rather than a difference between machines, and six further benchmarks pass the
        // tolerance on their own. The verdict is a median, so a large minority cannot
        // carry it: the family as a whole moved 0.2%.
        let captured = &ALL_THE_TIME_ARM_WINDOWS;
        let tolerance = merge_offset_tolerance();
        let pair = captured.assess();
        let outlier = pair
            .offsets
            .iter()
            .find(|offset| offset.benchmark == "threadpool_100")
            .unwrap_or_else(|| panic!("the capture holds threadpool_100"));
        let systematic = pair
            .systematic
            .unwrap_or_else(|| panic!("24 of the pair's shared offsets carry information"));

        assert!(
            (outlier.relative - THREADPOOL_100_OFFSET).abs() < CAPTURE_ROUNDING,
            "threadpool_100 now reads {} where production measured {}",
            percent(outlier.relative),
            percent(THREADPOOL_100_OFFSET),
        );
        assert!(
            outlier.beyond_tolerance,
            "an offset of {} is reported to an operator as a series that may gain a step",
            percent(outlier.relative),
        );
        assert!(
            !pair.manufactures_step,
            "one benchmark {} out and six more past the tolerance leave the family {} \
             apart — production measured {} — inside a tolerance of {}, so the pair \
             still merges",
            percent(outlier.relative),
            percent(systematic.relative),
            percent(captured.systematic_relative),
            percent(tolerance.relative),
        );
    }

    #[test]
    fn the_production_alloc_tracker_pairs_rest_on_one_informative_offset() {
        // Allocation figures barely move between two readings of one machine, so seven of
        // each pair's eight shared offsets are too small in their own units to say
        // anything and the median is taken over the one that remains. The gate is then
        // exactly as strict as reading that single series directly, which is the wanted
        // outcome where there is no family to average over.
        for captured in [&ALLOC_TRACKER_X64_WINDOWS, &ALLOC_TRACKER_X64_LINUX] {
            let pair = captured.assess();
            let systematic = pair
                .systematic
                .unwrap_or_else(|| panic!("{} has one informative offset", captured.label()));

            assert_eq!(
                pair.offsets.len(),
                captured.shared_levels(),
                "{}: the capture's shared measurements no longer all reach the assessment",
                captured.label(),
            );
            assert_eq!(
                systematic.offsets,
                captured.informative_offsets,
                "{}: the median is taken over {} offsets where production used {}",
                captured.label(),
                systematic.offsets,
                captured.informative_offsets,
            );
            assert!(
                !pair.manufactures_step,
                "{}: its one informative offset reads {} where production measured {}, \
                 so the pair merges",
                captured.label(),
                percent(systematic.relative),
                percent(captured.systematic_relative),
            );
        }
    }
}
