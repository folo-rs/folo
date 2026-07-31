//! Level data captured from the live `folohistory` benchmark store on 2026-08-01.
//!
//! Every figure here is a real `rekey` dry run's reading of the production store at
//! `origin/main`: 389 stored objects, 35 discriminant sets, seven pairs of machine-key
//! partitions that the current key format merges into one series. For each pair the
//! capture carries the two partitions' median levels on every `(benchmark, metric)`
//! both of them hold, plus the systematic offset, informative-offset count and
//! outlying-offset count the run reported.
//!
//! The tests that consume it are regression witnesses rather than fixtures: they feed
//! the captured levels through the genuine merge assessment, so a change to the gate
//! shows its effect on the store the tool actually serves. When one of them fails, the
//! verdict on real production data has moved, and the failure says by how much.
//!
//! A capture prints each level at fifteen significant digits, so a level recorded here
//! denotes the same `f64` the run measured and a systematic offset recomputed from the
//! levels reproduces the reported figure to about twelve digits — nine orders of
//! magnitude tighter than the tolerance the gate reads it against.

use std::collections::BTreeMap;

use cbh_model::{DiscriminantSet, Engine, MachineKey, MetricKind, TargetTriple};

use crate::rekey::merge::Interleaving;
use crate::rekey::{GroupPair, MeasuredPoint, analyze_merges, merge_offset_tolerance};

/// One `(benchmark, metric)` measurement both partitions of a pair hold: the qualified
/// benchmark identity, then the baseline and incoming partitions' median levels in the
/// metric's own units.
type Level = (&'static str, f64, f64);

/// The levels a pair shares on one metric.
///
/// The capture is grouped by metric because that is how the engines produce it: a
/// Criterion partition holds wall times alone, a Callgrind partition three counts, an
/// `alloc_tracker` partition bytes and allocations.
pub(crate) struct MetricLevels {
    /// The metric every level in the group was measured on.
    pub(crate) metric: MetricKind,
    /// The shared levels, in the order the capture lists them.
    pub(crate) levels: &'static [Level],
}

/// Two machine-key partitions the production store merges into one series, as the
/// `rekey` dry run found them.
pub(crate) struct CapturedPair {
    /// The engine whose partition this is.
    pub(crate) engine: Engine,
    /// The target triple the partition was recorded under.
    pub(crate) target_triple: &'static str,
    /// The current-format machine key both partitions merge into.
    pub(crate) destination_key: &'static str,
    /// The retired-format key of the partition history reaches first.
    pub(crate) baseline_key: &'static str,
    /// The key of the partition history reaches second.
    pub(crate) incoming_key: &'static str,
    /// The interleaving pattern the production run classified this pair as, and with it
    /// the smallest placement of commits that reproduces that classification. The
    /// individual commits behind the pattern are not captured, so the placement is a
    /// reconstruction and only the pattern is production data.
    pub(crate) commit_order: CommitOrder,
    /// The systematic relative offset the production run computed for this pair.
    pub(crate) systematic_relative: f64,
    /// How many shared offsets cleared their metric's absolute floor and so entered
    /// the median behind the systematic offset.
    pub(crate) informative_offsets: usize,
    /// How many shared offsets lie beyond the tolerance read on their own. These are
    /// reported to an operator and decide nothing; the count is pinned because the gap
    /// between it and the verdict is the whole reason the gate reads the pair.
    pub(crate) outlying_offsets: usize,
    /// The shared levels, grouped by metric.
    pub(crate) levels: &'static [MetricLevels],
}

impl CapturedPair {
    /// How the pair names itself in a failure message: engine and target triple, the
    /// two facets that place it in the store beside its destination key.
    pub(crate) fn label(&self) -> String {
        format!("{}/{}", self.engine.as_str(), self.target_triple)
    }

    /// How many `(benchmark, metric)` measurements the capture holds for this pair.
    pub(crate) fn shared_levels(&self) -> usize {
        self.levels.iter().map(|group| group.levels.len()).sum()
    }

    /// The destination partition the two source partitions merge into.
    pub(crate) fn destination(&self) -> DiscriminantSet {
        DiscriminantSet::new(
            self.engine,
            &TargetTriple::from(self.target_triple),
            &MachineKey::from(self.destination_key),
        )
    }

    /// The captured levels as the source-keyed measurements `rekey` reads out of the
    /// store.
    ///
    /// Each partition's captured median is replayed at every commit position that
    /// partition occupies, so the level the assessment recomputes for a series is the
    /// captured median exactly.
    pub(crate) fn measurements(&self) -> BTreeMap<String, Vec<MeasuredPoint>> {
        let (baseline_positions, incoming_positions) = self.commit_order.positions();
        let mut baseline = Vec::new();
        let mut incoming = Vec::new();
        for group in self.levels {
            for &(benchmark, baseline_level, incoming_level) in group.levels {
                for &position in baseline_positions {
                    baseline.push(MeasuredPoint {
                        benchmark: benchmark.to_owned(),
                        metric: group.metric,
                        value: baseline_level,
                        position,
                    });
                }
                for &position in incoming_positions {
                    incoming.push(MeasuredPoint {
                        benchmark: benchmark.to_owned(),
                        metric: group.metric,
                        value: incoming_level,
                        position,
                    });
                }
            }
        }
        BTreeMap::from([
            (self.baseline_key.to_owned(), baseline),
            (self.incoming_key.to_owned(), incoming),
        ])
    }

    /// The merge assessment `rekey` reaches on this pair, produced by the production
    /// analysis rather than restated by the test.
    pub(crate) fn assess(&self) -> GroupPair {
        let groups = BTreeMap::from([(self.destination(), self.measurements())]);
        let analysis = analyze_merges(&groups, merge_offset_tolerance());
        let merge = analysis
            .merges
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("{}: two source partitions are a merge", self.label()));
        merge
            .pairs
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("{}: two source partitions form one pair", self.label()))
    }
}

/// The whole capture as the destination-keyed measurements a `rekey` pass reads out of
/// the store, so a single assessment covers every merging partition the store holds.
pub(crate) fn captured_store() -> BTreeMap<DiscriminantSet, BTreeMap<String, Vec<MeasuredPoint>>> {
    CAPTURED_PAIRS
        .iter()
        .map(|pair| (pair.destination(), pair.measurements()))
        .collect()
}

/// The interleaving pattern the production dry run classified a pair's two partitions
/// as, and with it the smallest placement of commits that reproduces that pattern.
///
/// The commits behind the pattern are not captured, so the placement each variant fixes
/// stands in for the production history rather than reproducing it. It carries the
/// pair's orientation — which partition the offset is expressed against — into the
/// assessment, which is what the captured offsets are stated relative to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommitOrder {
    /// The two partitions' stretches of history overlap: the baseline partition holds
    /// commits both before and after the incoming partition's. That is one machine
    /// whose key wobbled back and forth rather than two machines succeeding each other,
    /// so no single boundary exists for a merge to hang a step change on.
    Overlapping,
    /// Neither partition holds a commit on the target ref's first-parent line, so the
    /// two cannot be ordered against each other at all.
    Unplaced,
}

impl CommitOrder {
    /// The interleaving `rekey` classifies a pair placed this way as.
    pub(crate) fn interleaving(self) -> Interleaving {
        match self {
            Self::Overlapping => Interleaving::Interleaved,
            Self::Unplaced => Interleaving::Unknown,
        }
    }

    /// The commit positions the baseline and the incoming partition's points take.
    fn positions(self) -> (&'static [Option<usize>], &'static [Option<usize>]) {
        match self {
            Self::Overlapping => (
                OVERLAPPING_BASELINE_POSITIONS,
                OVERLAPPING_INCOMING_POSITIONS,
            ),
            Self::Unplaced => (UNPLACED_POSITIONS, UNPLACED_POSITIONS),
        }
    }
}

/// The baseline partition of an overlapping pair holds the first and third of three
/// consecutive commits, with the incoming partition's commit between them.
const OVERLAPPING_BASELINE_POSITIONS: &[Option<usize>] = &[Some(0), Some(2)];

/// The incoming partition of an overlapping pair holds the commit between the
/// baseline's two.
const OVERLAPPING_INCOMING_POSITIONS: &[Option<usize>] = &[Some(1)];

/// A partition with no commit on the first-parent line has no position at all.
const UNPLACED_POSITIONS: &[Option<usize>] = &[None];

/// Every pair of partitions the production store merges, in capture order.
pub(crate) const CAPTURED_PAIRS: &[CapturedPair] = &[
    CRITERION_ARM_WINDOWS,
    ALL_THE_TIME_ARM_WINDOWS,
    CRITERION_X64_WINDOWS,
    CRITERION_X64_LINUX,
    CALLGRIND_X64_LINUX,
    ALLOC_TRACKER_X64_WINDOWS,
    ALLOC_TRACKER_X64_LINUX,
];

/// The ARM64 Windows Criterion partitions that the retired key format split apart.
///
/// One physical GitHub-hosted runner type stands behind both. On most boots all four of
/// its processors reported a relative-speed calibration of 10678; on the others, one of
/// the four reported 10681 instead. The retired format hashed that histogram, so three
/// units in 10678 — 0.028% — filed one runner's history under two keys, neither of them
/// long enough for the detector to judge.
///
/// The two partitions are the same machine to within a tenth of a percent, yet 46 of
/// their 123 informative offsets lie beyond the tolerance individually, because
/// per-benchmark wall-clock variation on a shared cloud runner is of exactly that
/// order. A threshold read against each benchmark refuses this merge, so the gate reads
/// the pair.
pub(crate) const CRITERION_ARM_WINDOWS: CapturedPair = CapturedPair {
    engine: Engine::Criterion,
    target_triple: "aarch64-pc-windows-msvc",
    destination_key: "2e3ad42f4e2cd3e1",
    baseline_key: "846b57d1fb778c2a",
    incoming_key: "3fc6d40058af4b4d",
    commit_order: CommitOrder::Overlapping,
    systematic_relative: -0.000765165934495731,
    informative_offsets: 123,
    outlying_offsets: 46,
    levels: CRITERION_ARM_WINDOWS_LEVELS,
};

/// The ARM64 Windows `all_the_time` partitions of the same runner.
///
/// `threadpool_100` is what makes this pair worth keeping: the incoming partition
/// measures it 49% faster than the baseline, which is a code change that landed while
/// one partition was active rather than a difference between two machines. The
/// systematic offset is a median, so that benchmark — and the six others past the
/// tolerance — cannot drag the verdict away from the 0.2% the family as a whole moved.
pub(crate) const ALL_THE_TIME_ARM_WINDOWS: CapturedPair = CapturedPair {
    engine: Engine::AllTheTime,
    target_triple: "aarch64-pc-windows-msvc",
    destination_key: "2e3ad42f4e2cd3e1",
    baseline_key: "846b57d1fb778c2a",
    incoming_key: "3fc6d40058af4b4d",
    commit_order: CommitOrder::Overlapping,
    systematic_relative: -0.00201422683937757,
    informative_offsets: 24,
    outlying_offsets: 7,
    levels: ALL_THE_TIME_ARM_WINDOWS_LEVELS,
};

/// The x64 Windows Criterion partitions of the AMD EPYC 7763 runner.
pub(crate) const CRITERION_X64_WINDOWS: CapturedPair = CapturedPair {
    engine: Engine::Criterion,
    target_triple: "x86_64-pc-windows-msvc",
    destination_key: "8c651396431bc05b",
    baseline_key: "8c651396431bc05b",
    incoming_key: "a401012bc82c8396",
    commit_order: CommitOrder::Unplaced,
    systematic_relative: -0.00329641064194916,
    informative_offsets: 8,
    outlying_offsets: 3,
    levels: CRITERION_X64_WINDOWS_LEVELS,
};

/// The x64 Linux Criterion partitions of the AMD EPYC 9V74 runner. Its systematic
/// offset is the only positive one in the capture, so the gate is pinned against a
/// merge in each direction.
pub(crate) const CRITERION_X64_LINUX: CapturedPair = CapturedPair {
    engine: Engine::Criterion,
    target_triple: "x86_64-unknown-linux-gnu",
    destination_key: "76110f7cbbb5a5e0",
    baseline_key: "76110f7cbbb5a5e0",
    incoming_key: "f566ff2f037beb1a",
    commit_order: CommitOrder::Unplaced,
    systematic_relative: 0.000505138126587886,
    informative_offsets: 7,
    outlying_offsets: 2,
    levels: CRITERION_X64_LINUX_LEVELS,
};

/// The x64 Linux Callgrind partitions of the AMD EPYC 9V74 runner.
///
/// Simulated instruction and branch counts barely move between the two partitions: ten
/// of twelve shared offsets carry information and their median is under two thousandths
/// of a percent, which is what two readings of one machine look like on a metric that
/// does not depend on machine load.
pub(crate) const CALLGRIND_X64_LINUX: CapturedPair = CapturedPair {
    engine: Engine::Callgrind,
    target_triple: "x86_64-unknown-linux-gnu",
    destination_key: "76110f7cbbb5a5e0",
    baseline_key: "76110f7cbbb5a5e0",
    incoming_key: "f566ff2f037beb1a",
    commit_order: CommitOrder::Unplaced,
    systematic_relative: -1.45117170164961e-5,
    informative_offsets: 10,
    outlying_offsets: 1,
    levels: CALLGRIND_X64_LINUX_LEVELS,
};

/// The x64 Windows `alloc_tracker` partitions of the AMD EPYC 7763 runner.
///
/// Allocation figures are near-exact, so seven of the pair's eight shared offsets are
/// too small in their own units to say anything about a level and only one enters the
/// median. That degenerate case is deliberate rather than accidental: with no family to
/// average over, the gate stays exactly as strict as reading that single series
/// directly.
pub(crate) const ALLOC_TRACKER_X64_WINDOWS: CapturedPair = CapturedPair {
    engine: Engine::AllocTracker,
    target_triple: "x86_64-pc-windows-msvc",
    destination_key: "8c651396431bc05b",
    baseline_key: "8c651396431bc05b",
    incoming_key: "a401012bc82c8396",
    commit_order: CommitOrder::Unplaced,
    systematic_relative: -7.3713696002288e-5,
    informative_offsets: 1,
    outlying_offsets: 0,
    levels: ALLOC_TRACKER_X64_WINDOWS_LEVELS,
};

/// The x64 Linux `alloc_tracker` partitions of the AMD EPYC 9V74 runner — the same
/// single-informative-offset shape as its Windows sibling, on different hardware.
pub(crate) const ALLOC_TRACKER_X64_LINUX: CapturedPair = CapturedPair {
    engine: Engine::AllocTracker,
    target_triple: "x86_64-unknown-linux-gnu",
    destination_key: "76110f7cbbb5a5e0",
    baseline_key: "76110f7cbbb5a5e0",
    incoming_key: "f566ff2f037beb1a",
    commit_order: CommitOrder::Unplaced,
    systematic_relative: -7.37136959730662e-5,
    informative_offsets: 1,
    outlying_offsets: 0,
    levels: ALLOC_TRACKER_X64_LINUX_LEVELS,
};

/// The 294 shared measurements of [`CRITERION_ARM_WINDOWS`].
const CRITERION_ARM_WINDOWS_LEVELS: &[MetricLevels] = &[MetricLevels {
    metric: MetricKind::WallTime,
    levels: &[
        (
            "all_the_time_example/example/computation",
            0.294850351600159,
            0.294872029914049,
        ),
        (
            "all_the_time_example/example/read_cell",
            0.295526111466685,
            0.295600662363994,
        ),
        (
            "all_the_time_example/example/string_formatting",
            125.465238418895,
            125.493735811765,
        ),
        (
            "all_the_time_report_lifecycle/merge/ops_100",
            11201.0088848227,
            11190.359962342,
        ),
        (
            "all_the_time_report_lifecycle/render/ops_10",
            2502.92211780709,
            2498.87200032459,
        ),
        (
            "all_the_time_report_lifecycle/render/ops_100",
            20542.0444798135,
            20516.9601004877,
        ),
        (
            "all_the_time_report_lifecycle/setup/session_100_ops",
            22347.833272581,
            22307.3231597787,
        ),
        (
            "all_the_time_report_lifecycle/snapshot/ops_100",
            15255.2368879499,
            15325.1760704522,
        ),
        (
            "all_the_time_report_lifecycle/write/ops_25",
            3978853.66987179,
            3694645.5,
        ),
        (
            "all_the_time_tracking_overhead/overhead/baseline_empty",
            0.294951252198856,
            0.294794675713642,
        ),
        (
            "all_the_time_tracking_overhead/overhead/batch_span_empty_1000_iterations",
            422.733980005574,
            421.470027846122,
        ),
        (
            "all_the_time_tracking_overhead/overhead/batch_span_empty_100_iterations",
            422.013005402286,
            422.851910682739,
        ),
        (
            "all_the_time_tracking_overhead/overhead/batch_span_empty_10_iterations",
            421.996734899382,
            423.282696946142,
        ),
        (
            "all_the_time_tracking_overhead/overhead/process_span_empty",
            505.018701440227,
            506.083961732344,
        ),
        (
            "all_the_time_tracking_overhead/overhead/thread_span_empty",
            421.181207345509,
            422.520650809414,
        ),
        (
            "alloc_tracker_example/example/string_formatting",
            138.754293525695,
            138.825598893407,
        ),
        (
            "alloc_tracker_example/example/vector_creation",
            48.2882983735778,
            48.2557954823963,
        ),
        (
            "alloc_tracker_report_lifecycle/merge/ops_100",
            11844.0709164733,
            11789.275045854,
        ),
        (
            "alloc_tracker_report_lifecycle/render/ops_10",
            6268.627482082,
            6311.09991255804,
        ),
        (
            "alloc_tracker_report_lifecycle/render/ops_100",
            57106.4668160846,
            57342.9933172422,
        ),
        (
            "alloc_tracker_report_lifecycle/setup/session_100_ops",
            22401.1965475652,
            22407.1829236655,
        ),
        (
            "alloc_tracker_report_lifecycle/snapshot/ops_100",
            17067.1417034738,
            17228.603780378,
        ),
        (
            "alloc_tracker_report_lifecycle/write/ops_25",
            4013849.76923077,
            3987922.64285714,
        ),
        (
            "alloc_tracker_tracking_overhead/overhead/baseline_empty",
            0.294809283989194,
            0.294829782284044,
        ),
        (
            "alloc_tracker_tracking_overhead/overhead/process_span_empty",
            62.8654184750342,
            62.4818667555297,
        ),
        (
            "alloc_tracker_tracking_overhead/overhead/thread_span_empty",
            34.340799544683,
            34.3318494188594,
        ),
        (
            "awaiter_set/is_empty/empty",
            0.6538218637635,
            0.649435006219287,
        ),
        (
            "awaiter_set/is_empty/populated",
            0.653892959287122,
            0.651803847684691,
        ),
        (
            "awaiter_set/notify_one_prior_generation/eligible",
            18.5150835997778,
            18.5363306973219,
        ),
        (
            "awaiter_set/register_notify_take/empty",
            17.0812873332345,
            17.2342282471589,
        ),
        (
            "awaiter_set/register_unregister/empty",
            5.92227280748427,
            5.88196130651809,
        ),
        (
            "awaiter_set/register_unregister/with_10_anchors",
            6.07249917761367,
            6.16181199227675,
        ),
        (
            "cbh_codec/compress/large",
            394973.583518053,
            394946.385104182,
        ),
        (
            "cbh_codec/compress/small",
            47789.0454657406,
            47757.8752489321,
        ),
        (
            "cbh_codec/decompress/large",
            49305.3105792905,
            49243.6425135051,
        ),
        (
            "cbh_codec/decompress/small",
            8494.63491137209,
            8493.64641630638,
        ),
        (
            "cbh_model/identity/qualified",
            78.7496505889605,
            78.5255995890251,
        ),
        (
            "cbh_model/storage_key/clean_key",
            776.18451247859,
            773.70343599417,
        ),
        (
            "cbh_model/storage_key/parse_key",
            412.696444329503,
            412.050339058275,
        ),
        (
            "cbh_model/storage_key/sanitize_segment",
            222.082185242151,
            222.271953313396,
        ),
        (
            "events_contended/auto_reset/event-listener/1_threads",
            38.9640365861249,
            39.6091836447059,
        ),
        (
            "events_contended/auto_reset/event-listener/2_threads",
            191.476066346444,
            187.023828501296,
        ),
        (
            "events_contended/auto_reset/event-listener/4_threads",
            470.441143515676,
            508.276276089232,
        ),
        (
            "events_contended/auto_reset/events/1_threads",
            18.2980878371395,
            18.3051063927867,
        ),
        (
            "events_contended/auto_reset/events/2_threads",
            106.865110414429,
            106.738977269546,
        ),
        (
            "events_contended/auto_reset/events/4_threads",
            326.508120761984,
            337.918103946005,
        ),
        (
            "events_contended/manual_reset/event-listener/1_threads",
            39.3269927388229,
            38.7131258586521,
        ),
        (
            "events_contended/manual_reset/event-listener/2_threads",
            211.81346600899,
            186.888929392234,
        ),
        (
            "events_contended/manual_reset/event-listener/4_threads",
            530.116507017826,
            438.342831232272,
        ),
        (
            "events_contended/manual_reset/events/1_threads",
            20.5481869429133,
            21.2913913591255,
        ),
        (
            "events_contended/manual_reset/events/2_threads",
            112.664506134282,
            124.364025348312,
        ),
        (
            "events_contended/manual_reset/events/4_threads",
            400.158765081192,
            405.50019732552,
        ),
        (
            "events_once_local/local/poll_connected",
            2.90653565638164,
            2.97579641399329,
        ),
        (
            "events_once_local/local/poll_disconnected",
            2.87983511397373,
            3.04392277700449,
        ),
        (
            "events_once_local/local/send_receive",
            4.6541626994555,
            4.63896802433514,
        ),
        (
            "events_once_local/local/set_connected",
            2.81012258496247,
            2.59796938419986,
        ),
        (
            "events_once_local/local/set_disconnected",
            2.7789695945874,
            2.50961690255087,
        ),
        (
            "events_once_sync/sync/poll_connected",
            11.0869833500575,
            11.1915223001269,
        ),
        (
            "events_once_sync/sync/poll_disconnected",
            2.91479495139796,
            2.68521062992717,
        ),
        (
            "events_once_sync/sync/send_receive",
            12.2400709860484,
            12.2944095513783,
        ),
        (
            "events_once_sync/sync/set_connected",
            10.4037422031592,
            10.6075600410306,
        ),
        (
            "events_once_sync/sync/set_disconnected",
            12.9430107242389,
            12.9393243305306,
        ),
        (
            "events_once_vs_3p/single_poll/local_boxed_send_receive",
            29.9028121907519,
            29.892227083203,
        ),
        (
            "events_once_vs_3p/single_poll/local_lake_send_receive",
            28.0781561331014,
            28.1516463662932,
        ),
        (
            "events_once_vs_3p/single_poll/local_pooled_send_receive",
            17.5554797294975,
            17.5278969665509,
        ),
        (
            "events_once_vs_3p/single_poll/local_raw_lake_send_receive",
            24.2040981021434,
            24.1638585243844,
        ),
        (
            "events_once_vs_3p/single_poll/local_raw_pooled_send_receive",
            14.447804484434,
            14.4217771998004,
        ),
        (
            "events_once_vs_3p/single_poll/oneshot_send_receive",
            55.0880654230441,
            55.1071298989518,
        ),
        (
            "events_once_vs_3p/single_poll/sync_boxed_send_receive",
            40.1951459291027,
            40.1944381182398,
        ),
        (
            "events_once_vs_3p/single_poll/sync_lake_send_receive",
            92.9412137898806,
            92.9154787732218,
        ),
        (
            "events_once_vs_3p/single_poll/sync_pooled_send_receive",
            78.8930203581652,
            78.6527826507848,
        ),
        (
            "events_once_vs_3p/single_poll/sync_raw_lake_send_receive",
            71.9217514912411,
            72.1006494049109,
        ),
        (
            "events_once_vs_3p/single_poll/sync_raw_pooled_send_receive",
            51.9260816159924,
            51.8595730553014,
        ),
        (
            "events_once_vs_3p/two_poll/local_boxed_send_receive_2poll",
            32.4419002343439,
            32.3044877704842,
        ),
        (
            "events_once_vs_3p/two_poll/local_lake_send_receive_2poll",
            32.5178163431494,
            32.5917174769008,
        ),
        (
            "events_once_vs_3p/two_poll/local_pooled_send_receive_2poll",
            21.8971516989022,
            22.0408061635128,
        ),
        (
            "events_once_vs_3p/two_poll/local_raw_lake_send_receive_2poll",
            26.5313578061047,
            26.5620073251246,
        ),
        (
            "events_once_vs_3p/two_poll/local_raw_pooled_send_receive_2poll",
            16.74781555976,
            16.7640111449488,
        ),
        (
            "events_once_vs_3p/two_poll/oneshot_send_receive_2poll",
            78.2665965692798,
            78.0622849674763,
        ),
        (
            "events_once_vs_3p/two_poll/sync_boxed_send_receive_2poll",
            59.1015541172272,
            58.9765164736621,
        ),
        (
            "events_once_vs_3p/two_poll/sync_lake_send_receive_2poll",
            112.999866888674,
            112.546486495629,
        ),
        (
            "events_once_vs_3p/two_poll/sync_pooled_send_receive_2poll",
            100.438653214202,
            100.225502378067,
        ),
        (
            "events_once_vs_3p/two_poll/sync_raw_lake_send_receive_2poll",
            92.0694208246148,
            91.4610793204449,
        ),
        (
            "events_once_vs_3p/two_poll/sync_raw_pooled_send_receive_2poll",
            76.0352241006505,
            75.3755047300069,
        ),
        (
            "events_uncontended/async_poll_ready/event_listener/Event",
            136.579454386891,
            135.683848177738,
        ),
        (
            "events_uncontended/async_poll_ready/event_listener/Event (listener!)",
            98.4071339837151,
            98.0660576745921,
        ),
        (
            "events_uncontended/async_poll_ready/events/AutoResetEvent",
            30.604263627779,
            30.6046607865964,
        ),
        (
            "events_uncontended/async_poll_ready/events/LocalAutoResetEvent",
            11.888020605378,
            11.7834161017001,
        ),
        (
            "events_uncontended/async_poll_ready/events/LocalManualResetEvent",
            9.85924711124932,
            9.78722947097047,
        ),
        (
            "events_uncontended/async_poll_ready/events/ManualResetEvent",
            15.8457502599548,
            15.8348644022479,
        ),
        (
            "events_uncontended/async_poll_ready/events/embedded/AutoResetEvent",
            20.5139531803189,
            20.5737507627827,
        ),
        (
            "events_uncontended/async_poll_ready/events/embedded/ManualResetEvent",
            5.86063503987663,
            5.84445946009301,
        ),
        (
            "events_uncontended/creation_boxed/events/AutoResetEvent",
            52.9788739086801,
            53.0437691914272,
        ),
        (
            "events_uncontended/creation_boxed/events/LocalAutoResetEvent",
            37.1844765272978,
            37.1951560019736,
        ),
        (
            "events_uncontended/creation_boxed/events/LocalManualResetEvent",
            37.3305066239625,
            37.3206273809342,
        ),
        (
            "events_uncontended/creation_boxed/events/ManualResetEvent",
            52.9466826621052,
            53.0135665749798,
        ),
        (
            "events_uncontended/creation_embedded/event_listener/Event",
            0.353669248357957,
            0.353767982487122,
        ),
        (
            "events_uncontended/creation_embedded/events/embedded/AutoResetEvent",
            0.804966465826725,
            0.805023484239824,
        ),
        (
            "events_uncontended/creation_embedded/events/embedded/LocalAutoResetEvent",
            0.781577715051962,
            0.781634315444526,
        ),
        (
            "events_uncontended/creation_embedded/events/embedded/LocalManualResetEvent",
            0.810159539787053,
            0.768163605873677,
        ),
        (
            "events_uncontended/creation_embedded/events/embedded/ManualResetEvent",
            0.804914149963237,
            0.804677560824829,
        ),
        (
            "events_uncontended/creation_embedded/rsevents/AutoResetEvent",
            0.294884502151371,
            0.295198849453503,
        ),
        (
            "events_uncontended/creation_embedded/rsevents/ManualResetEvent",
            0.294784880315547,
            0.295157881071194,
        ),
        (
            "events_uncontended/many_waiters/event_listener/Event",
            13210.4681385153,
            13198.3716774128,
        ),
        (
            "events_uncontended/many_waiters/events/AutoResetEvent",
            10127.5783387848,
            10134.1757955146,
        ),
        (
            "events_uncontended/many_waiters/events/LocalAutoResetEvent",
            2926.57640211608,
            2920.6512542217,
        ),
        (
            "events_uncontended/many_waiters/events/LocalManualResetEvent",
            2618.12566916946,
            2607.50052273362,
        ),
        (
            "events_uncontended/many_waiters/events/ManualResetEvent",
            8480.08123131487,
            8450.06396304072,
        ),
        (
            "events_uncontended/signal_round_trip/event_listener/Event",
            135.400783902679,
            135.216832525125,
        ),
        (
            "events_uncontended/signal_round_trip/event_listener/Event (listener!)",
            97.7451251361545,
            97.8418808948602,
        ),
        (
            "events_uncontended/signal_round_trip/events/AutoResetEvent",
            16.4985866744879,
            16.496476040046,
        ),
        (
            "events_uncontended/signal_round_trip/events/LocalAutoResetEvent",
            2.29597305418247,
            2.29611242918888,
        ),
        (
            "events_uncontended/signal_round_trip/events/LocalManualResetEvent",
            0.295571213643497,
            0.2956581285979,
        ),
        (
            "events_uncontended/signal_round_trip/events/ManualResetEvent",
            0.663049446451477,
            0.626485784901357,
        ),
        (
            "events_uncontended/signal_round_trip/events/embedded/AutoResetEvent",
            16.7188254862103,
            16.7436855897907,
        ),
        (
            "events_uncontended/signal_round_trip/events/embedded/ManualResetEvent",
            0.589684906161414,
            0.626350188758522,
        ),
        (
            "events_uncontended/signal_round_trip/rsevents/AutoResetEvent",
            15.4710385013686,
            15.4704608397236,
        ),
        (
            "events_uncontended/signal_round_trip/rsevents/ManualResetEvent",
            0.786187533206901,
            0.786193922203739,
        ),
        (
            "fast_time_timestamp_performance/timestamp_capture/fast_time_clock/now",
            1.16202661567676,
            1.15174048226895,
        ),
        (
            "fast_time_timestamp_performance/timestamp_capture/std_instant/now",
            38.1365021982589,
            38.1269398444695,
        ),
        (
            "future_deque/local/few_items_all_active",
            1710.42759822926,
            1707.22112058875,
        ),
        (
            "future_deque/local/many_items_mostly_active",
            3117131.93382353,
            3116071.625,
        ),
        (
            "future_deque/local/many_items_mostly_inactive",
            447718.792079208,
            447149.058864095,
        ),
        (
            "future_deque/sync/few_items_all_active",
            2019.00229478054,
            2022.24890279345,
        ),
        (
            "future_deque/sync/many_items_mostly_active",
            3147825.54227941,
            3142296.52941176,
        ),
        (
            "future_deque/sync/many_items_mostly_inactive",
            482166.292497906,
            481408.466183932,
        ),
        (
            "infinity_pool_focused/focused/blind_insert_with_5_layouts",
            197.973708727135,
            190.032201727635,
        ),
        (
            "infinity_pool_focused/focused/deref_handle",
            0.365543704531018,
            0.376929315156354,
        ),
        (
            "infinity_pool_focused/focused/iter_full",
            27781.7800888298,
            27771.1319639427,
        ),
        (
            "infinity_pool_focused/focused/iter_sparse",
            7518.7815335225,
            7519.91461481798,
        ),
        (
            "infinity_pool_focused/focused/raw_drop_full_string",
            127963.8136545,
            127865.900103443,
        ),
        (
            "infinity_pool_focused/focused/raw_drop_full_u64",
            21422.3889812862,
            21505.2116823623,
        ),
        (
            "infinity_pool_vs_std/churn_insertion/Arc::pin()",
            11452691.775,
            11000737.85,
        ),
        (
            "infinity_pool_vs_std/churn_insertion/BlindPool",
            8924473.025,
            8917751.8,
        ),
        (
            "infinity_pool_vs_std/churn_insertion/Box::pin()",
            6828790.35,
            6803795.95,
        ),
        (
            "infinity_pool_vs_std/churn_insertion/LocalBlindPool",
            5543692.65,
            5545991.975,
        ),
        (
            "infinity_pool_vs_std/churn_insertion/LocalOpaquePool",
            4386482.32916667,
            4381763.53333333,
        ),
        (
            "infinity_pool_vs_std/churn_insertion/LocalPinnedPool",
            4294596.81666667,
            4293952.78333334,
        ),
        (
            "infinity_pool_vs_std/churn_insertion/OpaquePool",
            7582824.0125,
            7551467.775,
        ),
        (
            "infinity_pool_vs_std/churn_insertion/PinnedPool",
            7487711.0375,
            7461133.9,
        ),
        (
            "infinity_pool_vs_std/churn_insertion/RawBlindPool",
            4103286.30833333,
            4113159.65000001,
        ),
        (
            "infinity_pool_vs_std/churn_insertion/RawOpaquePool",
            3541863.4,
            3542052.21666667,
        ),
        (
            "infinity_pool_vs_std/churn_insertion/RawPinnedPool",
            3498310.10833333,
            3485883.2,
        ),
        (
            "infinity_pool_vs_std/churn_insertion/Rc::pin()",
            9378623.825,
            9284786.6,
        ),
        (
            "linked_instance_per_thread/access/deref",
            3.14122685555122,
            3.25503675681517,
        ),
        (
            "linked_instance_per_thread/access/vs_static_lazy_lock",
            3.54915988376448,
            3.68665173763939,
        ),
        (
            "linked_instance_per_thread/access/vs_static_lazy_lock_mt",
            3.86741090832087,
            3.83460654406017,
        ),
        (
            "linked_instance_per_thread/access/vs_std_thread_local",
            3.61511880820176,
            3.75019717554752,
        ),
        (
            "linked_instance_per_thread/acquire/clone",
            14.7701038627286,
            15.1767079144748,
        ),
        (
            "linked_instance_per_thread/acquire/new_not_single",
            40.7566787395088,
            41.098980650533,
        ),
        (
            "linked_instance_per_thread/acquire/new_single",
            40.9023864175908,
            41.0773907936475,
        ),
        (
            "linked_instance_per_thread/acquire_two_threaded/clone",
            81.444035318061,
            89.468768570052,
        ),
        (
            "linked_instance_per_thread/acquire_two_threaded/new_not_single",
            253.221341366254,
            238.707712594357,
        ),
        (
            "linked_instance_per_thread/acquire_two_threaded/new_single",
            240.233432678296,
            233.454093396951,
        ),
        (
            "linked_instance_per_thread/create/clone",
            13.4747372344068,
            13.4551626152967,
        ),
        (
            "linked_instance_per_thread/create/new",
            75.5699534975715,
            69.4693289814159,
        ),
        (
            "linked_instance_per_thread_sync/access/deref",
            3.1139658220745,
            3.26012758490273,
        ),
        (
            "linked_instance_per_thread_sync/access/vs_static_lazy_lock",
            3.47546544121954,
            3.64730029300931,
        ),
        (
            "linked_instance_per_thread_sync/access/vs_static_lazy_lock_mt",
            3.83566402750507,
            3.98867450567274,
        ),
        (
            "linked_instance_per_thread_sync/access/vs_std_thread_local",
            3.59010920678534,
            3.74574256017151,
        ),
        (
            "linked_instance_per_thread_sync/acquire/clone",
            19.403665680457,
            19.8525892500128,
        ),
        (
            "linked_instance_per_thread_sync/acquire/new_not_single",
            44.7585185454217,
            44.3983998217804,
        ),
        (
            "linked_instance_per_thread_sync/acquire/new_single",
            44.8370843703097,
            44.4946652255481,
        ),
        (
            "linked_instance_per_thread_sync/acquire_two_threaded/clone",
            131.750655254751,
            121.513073550401,
        ),
        (
            "linked_instance_per_thread_sync/acquire_two_threaded/new_not_single",
            360.950686916012,
            231.632183428979,
        ),
        (
            "linked_instance_per_thread_sync/acquire_two_threaded/new_single",
            374.350549734963,
            232.105592809037,
        ),
        (
            "linked_instance_per_thread_sync/create/clone",
            13.2436953315371,
            13.5437928740368,
        ),
        (
            "linked_instance_per_thread_sync/create/new",
            88.0665093291119,
            71.7028873419795,
        ),
        (
            "linked_instances/get/one-threaded",
            42.5537977325397,
            42.8142795686795,
        ),
        (
            "linked_instances/get/two-threaded",
            294.920932254207,
            216.318938678362,
        ),
        (
            "linked_instances/get_1000/one-threaded",
            49959.283419536,
            50088.1559110389,
        ),
        (
            "linked_instances/get_1000/two-threaded",
            115882.821371689,
            111201.583022183,
        ),
        (
            "linked_static_thread_local_arc/access_one_threaded/to_arc",
            15.3745579195787,
            14.816006034112,
        ),
        (
            "linked_static_thread_local_arc/access_one_threaded/vs_static_lazy_lock",
            3.67108462640516,
            3.6584252594509,
        ),
        (
            "linked_static_thread_local_arc/access_one_threaded/vs_std_thread_local",
            3.75309164844414,
            3.73243474298647,
        ),
        (
            "linked_static_thread_local_arc/access_one_threaded/with",
            3.98142893492892,
            4.01181752538536,
        ),
        (
            "linked_static_thread_local_arc/access_two_threaded/to_arc",
            15.2029709067415,
            15.0953414731401,
        ),
        (
            "linked_static_thread_local_arc/access_two_threaded/vs_static_lazy_lock",
            3.70672209860519,
            3.90805449882335,
        ),
        (
            "linked_static_thread_local_arc/access_two_threaded/vs_std_thread_local",
            3.89866984821037,
            3.92319834452594,
        ),
        (
            "linked_static_thread_local_arc/access_two_threaded/with",
            4.09956935028271,
            4.23388332396634,
        ),
        (
            "linked_static_thread_local_rc/access_one_threaded/to_rc",
            4.59355433690341,
            4.82721871330466,
        ),
        (
            "linked_static_thread_local_rc/access_one_threaded/vs_static_lazy_lock",
            3.48185867566127,
            3.67198762906033,
        ),
        (
            "linked_static_thread_local_rc/access_one_threaded/vs_std_thread_local",
            3.61767602555136,
            3.76611722738621,
        ),
        (
            "linked_static_thread_local_rc/access_one_threaded/with",
            4.01475246842805,
            4.00291253996543,
        ),
        (
            "linked_static_thread_local_rc/access_two_threaded/to_rc",
            4.96293430211414,
            4.90036428930656,
        ),
        (
            "linked_static_thread_local_rc/access_two_threaded/vs_static_lazy_lock",
            3.87303332609301,
            3.94673424459731,
        ),
        (
            "linked_static_thread_local_rc/access_two_threaded/vs_std_thread_local",
            3.93069434657521,
            3.87944478224099,
        ),
        (
            "linked_static_thread_local_rc/access_two_threaded/with",
            4.15889842909367,
            4.19017508999544,
        ),
        (
            "many_cpus_benchmarking_harness_demo/CopyBytes/ConstrainedSameMemoryRegion",
            13108501.5,
            13685121.6590909,
        ),
        (
            "many_cpus_benchmarking_harness_demo/CopyBytes/PinnedSameMemoryRegion",
            13301773.5454546,
            13555189.6363636,
        ),
        (
            "many_cpus_benchmarking_harness_demo/CopyBytes/PinnedSameProcessor",
            15951659.6428572,
            16844250.0416667,
        ),
        (
            "many_cpus_benchmarking_harness_demo/CopyBytes/PinnedSelf",
            13298478.3636364,
            13588915.9772727,
        ),
        (
            "many_cpus_benchmarking_harness_demo/CopyBytes/UnpinnedPerMemoryRegionSelf",
            13036353.7272727,
            13747471.1886364,
        ),
        (
            "many_cpus_benchmarking_harness_demo/CopyBytes/UnpinnedSelf",
            13032218.0909091,
            13780462.6659091,
        ),
        (
            "many_cpus_hardware_info/hardware_info/max_processor_id",
            0.295466660435911,
            0.295177626561702,
        ),
        (
            "many_cpus_hardware_tracker/current/current_memory_region_id_pinned",
            3.1705958743906,
            3.16093057751282,
        ),
        (
            "many_cpus_hardware_tracker/current/current_memory_region_id_unpinned",
            11.1701901197942,
            11.1577251094669,
        ),
        (
            "many_cpus_hardware_tracker/current/current_processor_id_pinned",
            3.14911411917965,
            3.1410874043413,
        ),
        (
            "many_cpus_hardware_tracker/current/current_processor_id_unpinned",
            8.66878673418385,
            8.7042531386155,
        ),
        (
            "many_cpus_hardware_tracker/current/current_processor_pinned",
            4.49531604603147,
            4.48955834771054,
        ),
        (
            "many_cpus_hardware_tracker/current/current_processor_unpinned",
            11.1481221802544,
            11.1538492494928,
        ),
        (
            "many_cpus_pal_windows/pal/affinity_mask_to_processor_id_1",
            12.1059151942213,
            12.1062324110939,
        ),
        (
            "many_cpus_pal_windows/pal/affinity_mask_to_processor_id_16",
            13.4623130443573,
            13.3755606626922,
        ),
        (
            "many_cpus_pal_windows/pal/current_thread_processors",
            1746.3516305354,
            1742.29094458634,
        ),
        (
            "many_cpus_pal_windows/pal/get_all_processors",
            101684.318364546,
            104549.796841616,
        ),
        (
            "many_cpus_pal_windows/pal/pin_thread_to_default_set",
            801.881397289728,
            801.732087520413,
        ),
        (
            "many_cpus_processor_set_builder/processor_set_builder/all_mt",
            191885.622327519,
            192909.956031881,
        ),
        (
            "many_cpus_processor_set_builder/processor_set_builder/all_st",
            100522.001046955,
            106231.228676094,
        ),
        (
            "many_cpus_processor_set_builder/processor_set_builder/one_mt",
            191716.051692988,
            193319.96542519,
        ),
        (
            "many_cpus_processor_set_builder/processor_set_builder/one_st",
            100586.889704941,
            106582.502762883,
        ),
        (
            "many_cpus_processor_set_builder/processor_set_builder/only_evens_mt",
            386539.11500665,
            384951.666834454,
        ),
        (
            "many_cpus_processor_set_builder/processor_set_builder/only_evens_st",
            201074.705768189,
            211123.524080568,
        ),
        (
            "nm_performance/collection/collect_mt",
            1826.42114431047,
            1816.14388365514,
        ),
        (
            "nm_performance/collection/collect_st",
            1366.2494262455,
            1380.3667388447,
        ),
        (
            "nm_performance/observation/counter_batch_100_st_pull",
            10.466159721959,
            10.0141113012563,
        ),
        (
            "nm_performance/observation/counter_batch_10_st_pull",
            10.4580100365887,
            9.8482545835532,
        ),
        (
            "nm_performance/observation/counter_batch_10k_st_pull",
            10.3055122939034,
            9.86174783763407,
        ),
        (
            "nm_performance/observation/counter_batch_1_st_pull",
            10.4701487717527,
            10.2553335623444,
        ),
        (
            "nm_performance/observation/counter_batch_1k_st_pull",
            10.4691544733515,
            10.1628095891147,
        ),
        (
            "nm_performance/observation/counter_mt_pull",
            10.2492125895075,
            10.2577719627656,
        ),
        (
            "nm_performance/observation/counter_mt_push",
            2.57306216375575,
            2.594270291069,
        ),
        (
            "nm_performance/observation/counter_st_pull",
            10.4505707532854,
            10.2594198218098,
        ),
        (
            "nm_performance/observation/counter_st_push",
            2.54940659189592,
            2.5880358327406,
        ),
        (
            "nm_performance/observation/large_histogram_last_bucket_st_pull",
            26.2245163279773,
            26.2892665431651,
        ),
        (
            "nm_performance/observation/large_histogram_last_bucket_st_push",
            17.7327513327929,
            18.6073596462295,
        ),
        (
            "nm_performance/observation/large_histogram_max_mt_pull",
            26.1107244522041,
            26.1368347173674,
        ),
        (
            "nm_performance/observation/large_histogram_max_mt_push",
            17.778288079479,
            17.7071182817246,
        ),
        (
            "nm_performance/observation/large_histogram_max_st_pull",
            26.009044056285,
            25.9254053030699,
        ),
        (
            "nm_performance/observation/large_histogram_max_st_push",
            17.7378803553491,
            17.6883360996133,
        ),
        (
            "nm_performance/observation/large_histogram_zero_mt_pull",
            14.561952525107,
            14.6537592943177,
        ),
        (
            "nm_performance/observation/large_histogram_zero_mt_push",
            4.05116338234727,
            4.11951432043705,
        ),
        (
            "nm_performance/observation/large_histogram_zero_st_pull",
            14.4836301302427,
            14.5222785185883,
        ),
        (
            "nm_performance/observation/large_histogram_zero_st_push",
            4.03762316629163,
            4.0677402094344,
        ),
        (
            "nm_performance/observation/plain_mt_pull",
            10.2719025255153,
            10.2596438943105,
        ),
        (
            "nm_performance/observation/plain_mt_push",
            2.66241708018104,
            2.66981735964221,
        ),
        (
            "nm_performance/observation/plain_st_pull",
            10.4781572119964,
            10.2865104477055,
        ),
        (
            "nm_performance/observation/plain_st_push",
            2.63821546429014,
            2.66524643736412,
        ),
        (
            "nm_performance/observation/small_histogram_last_bucket_st_pull",
            14.7338432382315,
            14.8650670732853,
        ),
        (
            "nm_performance/observation/small_histogram_last_bucket_st_push",
            5.49740970564638,
            5.53059328917104,
        ),
        (
            "nm_performance/observation/small_histogram_max_mt_pull",
            10.7721935568073,
            10.7730752384043,
        ),
        (
            "nm_performance/observation/small_histogram_max_mt_push",
            5.01074702274952,
            5.0368614587193,
        ),
        (
            "nm_performance/observation/small_histogram_max_st_pull",
            10.7378079368953,
            10.8505394666122,
        ),
        (
            "nm_performance/observation/small_histogram_max_st_push",
            4.95467854813892,
            5.01560400083551,
        ),
        (
            "nm_performance/observation/small_histogram_zero_mt_pull",
            14.5563158993561,
            14.6431952898512,
        ),
        (
            "nm_performance/observation/small_histogram_zero_mt_push",
            4.05695838434989,
            4.08422090128017,
        ),
        (
            "nm_performance/observation/small_histogram_zero_st_pull",
            14.5527734241653,
            14.6332444578843,
        ),
        (
            "nm_performance/observation/small_histogram_zero_st_push",
            4.05694866015049,
            4.06650665635758,
        ),
        (
            "nm_performance/push/push_mt",
            5.63188477985697,
            5.63564707858003,
        ),
        (
            "nm_performance/push/push_st",
            5.62747088660617,
            5.55559178294241,
        ),
        (
            "nm_performance/timing/timing_mt_pull",
            16.0463716313473,
            15.9452528892351,
        ),
        (
            "nm_performance/timing/timing_mt_push",
            10.1057375786183,
            10.181252726458,
        ),
        (
            "nm_performance/timing/timing_st_pull",
            16.4519181865814,
            15.6749653209356,
        ),
        (
            "nm_performance/timing/timing_st_push",
            10.0874084112636,
            10.0531516271267,
        ),
        (
            "nm_push_bulk/push_bulk/observe_pull_counter",
            9689.96792544057,
            9691.53444830065,
        ),
        (
            "nm_push_bulk/push_bulk/observe_pull_histogram",
            14589.627869679,
            14609.6929106651,
        ),
        (
            "nm_push_bulk/push_bulk/observe_push_only_counter",
            1933.00434311564,
            1932.17178020499,
        ),
        (
            "nm_push_bulk/push_bulk/observe_push_only_histogram",
            4035.38734020209,
            4035.71429512043,
        ),
        (
            "nm_push_bulk/push_bulk/observe_sparse_then_push_counter",
            927.411404984201,
            918.845637808244,
        ),
        (
            "nm_push_bulk/push_bulk/observe_sparse_then_push_histogram",
            928.70399946102,
            934.981564229127,
        ),
        (
            "nm_push_bulk/push_bulk/observe_then_push_counter",
            5189.06564580034,
            5189.62363162158,
        ),
        (
            "nm_push_bulk/push_bulk/observe_then_push_histogram",
            9197.44197877499,
            9176.0468121439,
        ),
        (
            "nm_push_bulk/push_bulk/push_idle_counter",
            806.793707754529,
            802.318654654679,
        ),
        (
            "nm_push_bulk/push_bulk/push_idle_histogram",
            839.013220381272,
            838.732233520056,
        ),
        (
            "par_bench_basic/atomic_increments/multi_thread",
            160.530645850111,
            162.298437468902,
        ),
        (
            "par_bench_basic/atomic_increments/single_thread",
            7.41525098873163,
            7.51654119785607,
        ),
        (
            "par_bench_manual/atomic_increments/multi_thread",
            163.66916141521,
            160.186217746093,
        ),
        (
            "par_bench_manual/atomic_increments/single_thread",
            7.44805577081597,
            7.62878109072891,
        ),
        (
            "par_bench_overhead/overhead/par_bench_overhead",
            0.796394215418739,
            0.791668686178426,
        ),
        (
            "region_cached/get_set_pin/par_get_set_same_region",
            651.298209772708,
            723.897729274129,
        ),
        (
            "region_cached/get_set_pin/par_with_set_busy_same_region",
            1748.84951767864,
            1674.00544961125,
        ),
        (
            "region_cached/read/get_pin",
            26.3556185464963,
            27.5613740933753,
        ),
        (
            "region_cached/read/get_unpin",
            43.8137527197482,
            43.8343336855717,
        ),
        (
            "region_cached/read/par_get_same_region",
            26.5610903722644,
            26.3228832321916,
        ),
        (
            "region_cached/write/set_pin",
            471.958662285566,
            471.938197084314,
        ),
        (
            "region_cached/write/set_unpin",
            263.450093906908,
            263.836013349568,
        ),
        (
            "region_local/get_set_pin/par_get_set_same_region",
            188.47063467682,
            183.752225878259,
        ),
        (
            "region_local/get_set_pin/par_with_set_busy_same_region",
            356.276415382733,
            337.590854894334,
        ),
        (
            "region_local/read/get_pin",
            24.3847085524147,
            24.7462378649759,
        ),
        (
            "region_local/read/get_pin_two",
            24.6951480612469,
            24.8567458917334,
        ),
        (
            "region_local/read/get_unpin",
            40.0911863991285,
            39.9337083876633,
        ),
        (
            "region_local/write/set_pin",
            272.94279031891,
            273.499736165026,
        ),
        (
            "region_local/write/set_unpin",
            276.050199094611,
            276.017877761433,
        ),
        (
            "vicinal/fire_and_forget/spawn_and_forget_100",
            502745.116398207,
            518025.104132801,
        ),
        (
            "vicinal/fire_and_forget/spawn_and_forget_single",
            8768.70859545154,
            8782.84364481224,
        ),
        (
            "vicinal/fire_and_forget/spawn_with_event_100",
            543681.55135215,
            545253.526599675,
        ),
        (
            "vicinal/fire_and_forget/spawn_with_event_single",
            9009.15999118921,
            8948.16711498683,
        ),
        (
            "vicinal/spawn/spawn_100",
            512303.329589675,
            509366.944288459,
        ),
        (
            "vicinal/spawn/spawn_single",
            8715.64420426706,
            8652.1067695078,
        ),
        (
            "vicinal/spawn/spawn_urgent_100",
            509526.599379341,
            512619.83123984,
        ),
        (
            "vicinal/spawn/spawn_urgent_single",
            8752.92858315682,
            8777.55857681756,
        ),
        (
            "vicinal/spawn/thread_100",
            5891028.22222222,
            5748455.33333333,
        ),
        (
            "vicinal/spawn/thread_single",
            118846.375974911,
            123329.241839482,
        ),
        (
            "vicinal/spawn/threadpool_100",
            157748.338406975,
            152453.640999388,
        ),
        (
            "vicinal/spawn/threadpool_single",
            13009.4563701999,
            13099.6087793711,
        ),
    ],
}];

/// The 27 shared measurements of [`ALL_THE_TIME_ARM_WINDOWS`].
const ALL_THE_TIME_ARM_WINDOWS_LEVELS: &[MetricLevels] = &[MetricLevels {
    metric: MetricKind::ProcessorTime,
    levels: &[
        (
            "all_the_time_report_lifecycle/merge/ops_100",
            11204.0610813,
            11223.8744001314,
        ),
        (
            "all_the_time_report_lifecycle/render/ops_10",
            2511.72098837884,
            2502.42560291773,
        ),
        (
            "all_the_time_report_lifecycle/render/ops_100",
            20486.1390198907,
            20469.0803807642,
        ),
        (
            "all_the_time_report_lifecycle/setup/session_100_ops",
            22246.3864223081,
            22040.8038943609,
        ),
        (
            "all_the_time_report_lifecycle/snapshot/ops_100",
            15047.1318275034,
            14825.5138676208,
        ),
        (
            "all_the_time_report_lifecycle/write/ops_25",
            3655292.69291124,
            3543091.77108026,
        ),
        (
            "alloc_tracker_report_lifecycle/merge/ops_100",
            11793.4608645798,
            11701.5772502672,
        ),
        (
            "alloc_tracker_report_lifecycle/render/ops_10",
            6274.65172433514,
            6282.82566628281,
        ),
        (
            "alloc_tracker_report_lifecycle/render/ops_100",
            56896.9841398146,
            56916.6917705116,
        ),
        (
            "alloc_tracker_report_lifecycle/setup/session_100_ops",
            22393.7319811559,
            22451.8497343513,
        ),
        (
            "alloc_tracker_report_lifecycle/snapshot/ops_100",
            17198.0290708302,
            17143.0682644232,
        ),
        (
            "alloc_tracker_report_lifecycle/write/ops_25",
            3655761.75206386,
            3630375.88892652,
        ),
        ("computation", 0.292816857691525, 0.294120573642823),
        ("read_cell", 0.294638751862432, 0.294717536236849),
        ("spawn_100", 432091.243316731, 457626.037138009),
        ("spawn_and_forget_100", 447196.878146177, 452888.388705135),
        (
            "spawn_and_forget_single",
            8557.02013241054,
            8484.22568650405,
        ),
        ("spawn_single", 8546.78272556418, 8660.33538398623),
        ("spawn_urgent_100", 445257.184296486, 445966.356041532),
        ("spawn_urgent_single", 8521.47120469395, 8389.01485663278),
        ("spawn_with_event_100", 469420.052024077, 470543.728389399),
        (
            "spawn_with_event_single",
            8819.90062416423,
            8757.93001019056,
        ),
        ("string_formatting", 125.004879798099, 125.061297698427),
        ("thread_100", 12872325.3841078, 12586888.3515225),
        ("thread_single", 102016.38816628, 103619.770728446),
        ("threadpool_100", 312336.509324277, 157911.38051796),
        ("threadpool_single", 11789.6477667352, 12273.5263167989),
    ],
}];

/// The 8 shared measurements of [`CRITERION_X64_WINDOWS`].
const CRITERION_X64_WINDOWS_LEVELS: &[MetricLevels] = &[MetricLevels {
    metric: MetricKind::WallTime,
    levels: &[
        (
            "cbh_codec/compress/large",
            648843.611053643,
            647196.245603665,
        ),
        ("cbh_codec/compress/small", 76730.4320742063, 76470.66183174),
        (
            "cbh_codec/decompress/large",
            37466.314330859,
            37346.1475132588,
        ),
        (
            "cbh_codec/decompress/small",
            8044.62698246233,
            8046.15205909132,
        ),
        (
            "cbh_model/identity/qualified",
            104.285955080285,
            101.689706472741,
        ),
        (
            "cbh_model/storage_key/clean_key",
            804.254809125897,
            799.929257804135,
        ),
        (
            "cbh_model/storage_key/parse_key",
            436.790175151208,
            489.691224737136,
        ),
        (
            "cbh_model/storage_key/sanitize_segment",
            230.245461311531,
            226.740318795638,
        ),
    ],
}];

/// The 8 shared measurements of [`CRITERION_X64_LINUX`].
const CRITERION_X64_LINUX_LEVELS: &[MetricLevels] = &[MetricLevels {
    metric: MetricKind::WallTime,
    levels: &[
        (
            "cbh_codec/compress/large",
            574707.550462539,
            574352.726020393,
        ),
        (
            "cbh_codec/compress/small",
            77523.1497252504,
            77612.4754835116,
        ),
        (
            "cbh_codec/decompress/large",
            36337.6062679178,
            36355.9617782727,
        ),
        (
            "cbh_codec/decompress/small",
            7799.14371792709,
            7832.01655502793,
        ),
        (
            "cbh_model/identity/qualified",
            50.7359055470803,
            51.1720769056184,
        ),
        (
            "cbh_model/storage_key/clean_key",
            440.627971722932,
            427.005480910922,
        ),
        (
            "cbh_model/storage_key/parse_key",
            253.703372730808,
            261.584318266532,
        ),
        (
            "cbh_model/storage_key/sanitize_segment",
            111.691693136391,
            110.672649080536,
        ),
    ],
}];

/// The 12 shared measurements of [`CALLGRIND_X64_LINUX`].
const CALLGRIND_X64_LINUX_LEVELS: &[MetricLevels] = &[
    MetricLevels {
        metric: MetricKind::InstructionCount,
        levels: &[
            (
                "cbh_codec/cbh_codec_cg::compress::compress_large/compress_large/run",
                6499793.0,
                6499721.0,
            ),
            (
                "cbh_codec/cbh_codec_cg::compress::compress_small/compress_small/run",
                1413898.0,
                1413862.0,
            ),
            (
                "cbh_codec/cbh_codec_cg::decompress::decompress_large/decompress_large/run",
                742587.0,
                742517.0,
            ),
            (
                "cbh_codec/cbh_codec_cg::decompress::decompress_small/decompress_small/run",
                226873.0,
                227099.0,
            ),
        ],
    },
    MetricLevels {
        metric: MetricKind::ConditionalBranches,
        levels: &[
            (
                "cbh_codec/cbh_codec_cg::compress::compress_large/compress_large/run",
                1408418.0,
                1408400.0,
            ),
            (
                "cbh_codec/cbh_codec_cg::compress::compress_small/compress_small/run",
                800338.0,
                800325.0,
            ),
            (
                "cbh_codec/cbh_codec_cg::decompress::decompress_large/decompress_large/run",
                490223.0,
                490214.0,
            ),
            (
                "cbh_codec/cbh_codec_cg::decompress::decompress_small/decompress_small/run",
                167465.0,
                167486.0,
            ),
        ],
    },
    MetricLevels {
        metric: MetricKind::IndirectBranches,
        levels: &[
            (
                "cbh_codec/cbh_codec_cg::compress::compress_large/compress_large/run",
                120.0,
                118.0,
            ),
            (
                "cbh_codec/cbh_codec_cg::compress::compress_small/compress_small/run",
                119.0,
                117.0,
            ),
            (
                "cbh_codec/cbh_codec_cg::decompress::decompress_large/decompress_large/run",
                1220.0,
                1217.0,
            ),
            (
                "cbh_codec/cbh_codec_cg::decompress::decompress_small/decompress_small/run",
                261.0,
                271.0,
            ),
        ],
    },
];

/// The 8 shared measurements of [`ALLOC_TRACKER_X64_WINDOWS`].
const ALLOC_TRACKER_X64_WINDOWS_LEVELS: &[MetricLevels] = &[
    MetricLevels {
        metric: MetricKind::AllocatedBytes,
        levels: &[
            ("cbh_codec/compress/large", 126321.0, 126321.0),
            (
                "cbh_codec/compress/small",
                13001.0002584984,
                13001.0002584984,
            ),
            ("cbh_codec/decompress/large", 198766.0, 198766.0),
            (
                "cbh_codec/decompress/small",
                13566.0000004471,
                13565.0000004471,
            ),
        ],
    },
    MetricLevels {
        metric: MetricKind::AllocationCount,
        levels: &[
            ("cbh_codec/compress/large", 1.0, 1.0),
            (
                "cbh_codec/compress/small",
                1.00000000470166,
                1.00000000470166,
            ),
            ("cbh_codec/decompress/large", 3.0, 3.0),
            (
                "cbh_codec/decompress/small",
                2.00000000001033,
                2.00000000001033,
            ),
        ],
    },
];

/// The 8 shared measurements of [`ALLOC_TRACKER_X64_LINUX`].
const ALLOC_TRACKER_X64_LINUX_LEVELS: &[MetricLevels] = &[
    MetricLevels {
        metric: MetricKind::AllocatedBytes,
        levels: &[
            ("cbh_codec/compress/large", 126321.0, 126321.0),
            (
                "cbh_codec/compress/small",
                13001.0002584984,
                13001.0002584984,
            ),
            ("cbh_codec/decompress/large", 198766.0, 198766.0),
            (
                "cbh_codec/decompress/small",
                13566.0000004456,
                13565.000000446,
            ),
        ],
    },
    MetricLevels {
        metric: MetricKind::AllocationCount,
        levels: &[
            ("cbh_codec/compress/large", 1.0, 1.0),
            (
                "cbh_codec/compress/small",
                1.00000000470166,
                1.00000000470166,
            ),
            ("cbh_codec/decompress/large", 3.0, 3.0),
            (
                "cbh_codec/decompress/small",
                2.00000000001029,
                2.0000000000103,
            ),
        ],
    },
];
