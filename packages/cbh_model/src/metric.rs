//! A single measured quantity and the categories a benchmark engine can report.

use serde::{Deserialize, Serialize};

/// A single measured quantity.
///
/// The [`kind`](Self::kind) fully determines the metric's meaning, including its
/// unit (see [`MetricKind::as_unit`]). Every kind is lower-is-better, so a rise is
/// always a regression and a fall an improvement. A benchmark result carries at
/// most one metric of each kind.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Metric {
    /// What kind of quantity this is (governs unit and comparison semantics).
    pub kind: MetricKind,
    /// The per-iteration point estimate, in the unit implied by `kind`.
    ///
    /// This is the through-origin regression slope when the engine reports one
    /// (Criterion wall time, `all_the_time` processor time, `alloc_tracker`
    /// allocations), otherwise the mean or the single measured value. No engine is
    /// treated as noise-free: even Callgrind instruction and event counts jitter a
    /// few percent from run to run, so this is always a point estimate rather than
    /// an exact truth.
    pub value: f64,
    /// Estimated standard deviation of the measurement, when the engine reports
    /// one (Criterion, `all_the_time`, `alloc_tracker`). Absent for engines that
    /// report no dispersion (Callgrind).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub std_dev: Option<f64>,
    /// Lower bound of the value's confidence interval, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_low: Option<f64>,
    /// Upper bound of the value's confidence interval, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_high: Option<f64>,
}

impl Metric {
    /// Creates a metric with no dispersion information.
    #[must_use]
    pub fn new(kind: MetricKind, value: f64) -> Self {
        Self {
            kind,
            value,
            std_dev: None,
            interval_low: None,
            interval_high: None,
        }
    }

    /// Attaches dispersion information (standard deviation and confidence-interval
    /// bounds) to this metric, for noise-aware comparison of noisy engines.
    #[must_use]
    pub fn with_dispersion(
        mut self,
        std_dev: Option<f64>,
        interval_low: Option<f64>,
        interval_high: Option<f64>,
    ) -> Self {
        self.std_dev = std_dev;
        self.interval_low = interval_low;
        self.interval_high = interval_high;
        self
    }
}

/// The category of a [`Metric`], which fully determines its unit and how it is
/// compared over time.
///
/// Each kind has exactly one unit (see [`as_unit`](Self::as_unit)) and is
/// lower-is-better: a rise is a regression, a fall an improvement.
///
/// Only the Callgrind metrics that stay stable across builds are modelled —
/// instruction and branch-execution counts. The cache-simulation and
/// branch-misprediction events (cache hits at each tier, estimated cycles, branch
/// misses) are deliberately absent: at the small magnitudes typical of
/// microbenchmarks their values track binary layout (embedded build paths,
/// section sizes, code/data placement) rather than code behaviour, so they cannot
/// be compared build to build across a history. See the crate design notes for
/// the evidence.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    /// Wall-clock time per iteration in nanoseconds (Criterion); hardware-dependent
    /// and noisy.
    WallTime,
    /// Processor time per iteration in nanoseconds (`all_the_time`);
    /// hardware-dependent and noisy.
    ProcessorTime,
    /// Retired instruction count (Callgrind); low-noise but not exact.
    InstructionCount,
    /// Executed conditional branches (Callgrind); low-noise but not exact.
    ConditionalBranches,
    /// Executed indirect branches (Callgrind); low-noise but not exact.
    IndirectBranches,
    /// Bytes allocated per iteration (`alloc_tracker`); hardware-independent but
    /// not deterministic (warmup and buffer-resize allocations jitter the
    /// per-iteration figure).
    AllocatedBytes,
    /// Allocation count per iteration (`alloc_tracker`); hardware-independent but
    /// not deterministic.
    AllocationCount,
}

impl MetricKind {
    /// Every metric kind, in declaration order.
    ///
    /// The single source of truth for enumerating the kinds — used to list the
    /// valid names when a name lookup fails (see [`from_name`](Self::from_name))
    /// and to exercise every kind in tests.
    pub const ALL: [Self; 7] = [
        Self::WallTime,
        Self::ProcessorTime,
        Self::InstructionCount,
        Self::ConditionalBranches,
        Self::IndirectBranches,
        Self::AllocatedBytes,
        Self::AllocationCount,
    ];

    /// The stable `snake_case` label for this kind, matching its serialized wire
    /// name. Used as the metric's display name in reports.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WallTime => "wall_time",
            Self::ProcessorTime => "processor_time",
            Self::InstructionCount => "instruction_count",
            Self::ConditionalBranches => "conditional_branches",
            Self::IndirectBranches => "indirect_branches",
            Self::AllocatedBytes => "allocated_bytes",
            Self::AllocationCount => "allocation_count",
        }
    }

    /// Parses a stable `snake_case` metric name (as produced by
    /// [`as_str`](Self::as_str)) back into its kind, or `None` when the name is
    /// not one of the known metrics.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    /// The unit this kind is always measured in, for display.
    #[must_use]
    pub fn as_unit(self) -> &'static str {
        match self {
            Self::WallTime | Self::ProcessorTime => "ns",
            Self::AllocatedBytes => "bytes",
            Self::InstructionCount
            | Self::ConditionalBranches
            | Self::IndirectBranches
            | Self::AllocationCount => "count",
        }
    }

    /// Whether this kind moves in discrete integer steps with no dispersion.
    ///
    /// The Callgrind counts (instructions and branches) are exact integers for a
    /// fixed binary and input, reported without a confidence interval, so they can
    /// only move by whole units. At a small baseline a single-unit run-to-run
    /// difference is therefore a large *percentage* move that a purely relative gate
    /// would misread as a regression, so the analysis holds these kinds to an
    /// absolute-magnitude floor in addition to the relative one. The time and
    /// `alloc_tracker` metrics are continuous slopes carrying dispersion, so they are
    /// not quantized and only the relative gate applies.
    #[must_use]
    pub fn is_quantized(self) -> bool {
        match self {
            Self::InstructionCount | Self::ConditionalBranches | Self::IndirectBranches => true,
            Self::WallTime | Self::ProcessorTime | Self::AllocatedBytes | Self::AllocationCount => {
                false
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn metric_kind_serializes_snake_case() {
        let json = serde_json::to_string(&MetricKind::InstructionCount).unwrap();
        assert_eq!(json, "\"instruction_count\"");
    }

    #[test]
    fn metric_kind_wire_name_matches_as_str() {
        // The kinds round-trip through stored JSON, so the serialized wire name
        // and the display label must stay in lockstep.
        for kind in MetricKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            let parsed: MetricKind = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn from_name_round_trips_every_kind_and_rejects_unknown() {
        for kind in MetricKind::ALL {
            assert_eq!(MetricKind::from_name(kind.as_str()), Some(kind));
        }
        assert_eq!(MetricKind::from_name("not_a_metric"), None);
        assert_eq!(MetricKind::from_name(""), None);
        // The match is exact, not a prefix or case-insensitive one.
        assert_eq!(MetricKind::from_name("Instruction_Count"), None);
        assert_eq!(MetricKind::from_name("instruction"), None);
    }

    #[test]
    fn metric_kind_units_are_fixed_per_kind() {
        assert_eq!(MetricKind::WallTime.as_unit(), "ns");
        assert_eq!(MetricKind::ProcessorTime.as_unit(), "ns");
        assert_eq!(MetricKind::AllocatedBytes.as_unit(), "bytes");
        assert_eq!(MetricKind::InstructionCount.as_unit(), "count");
        assert_eq!(MetricKind::ConditionalBranches.as_unit(), "count");
    }

    #[test]
    fn only_the_callgrind_counts_are_quantized() {
        // The Callgrind integer counts move in whole units with no dispersion, so
        // they earn the absolute-magnitude floor; the continuous, dispersion-bearing
        // metrics do not.
        assert!(MetricKind::InstructionCount.is_quantized());
        assert!(MetricKind::ConditionalBranches.is_quantized());
        assert!(MetricKind::IndirectBranches.is_quantized());
        assert!(!MetricKind::WallTime.is_quantized());
        assert!(!MetricKind::ProcessorTime.is_quantized());
        assert!(!MetricKind::AllocatedBytes.is_quantized());
        assert!(!MetricKind::AllocationCount.is_quantized());
    }

    #[test]
    fn metric_dispersion_is_omitted_when_absent() {
        let metric = Metric::new(MetricKind::InstructionCount, 1.0);
        let json = serde_json::to_string(&metric).unwrap();
        assert!(!json.contains("std_dev"), "{json}");
        assert!(!json.contains("interval_low"), "{json}");
    }

    #[test]
    fn metric_dispersion_roundtrips_when_present() {
        let metric = Metric::new(MetricKind::WallTime, 26.9).with_dispersion(
            Some(0.47),
            Some(26.6),
            Some(27.2),
        );

        let json = serde_json::to_string(&metric).unwrap();
        let parsed: Metric = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, metric);
        assert_eq!(parsed.std_dev, Some(0.47));
        assert_eq!(parsed.interval_low, Some(26.6));
        assert_eq!(parsed.interval_high, Some(27.2));
    }
}
