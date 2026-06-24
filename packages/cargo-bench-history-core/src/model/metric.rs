//! A single measured quantity and the categories a benchmark engine can report.

use serde::{Deserialize, Serialize};

/// A single measured quantity.
///
/// The [`kind`](Self::kind) fully determines the metric's meaning, including its
/// unit (see [`MetricKind::as_unit`]) and its comparison polarity (see
/// [`MetricKind::higher_is_better`]). A benchmark result carries at most one
/// metric of each kind.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Metric {
    /// What kind of quantity this is (governs unit and comparison semantics).
    pub kind: MetricKind,
    /// The per-iteration point estimate, in the unit implied by `kind`.
    ///
    /// For noisy engines (Criterion wall time, `all_the_time` processor time)
    /// this is the through-origin regression slope when the engine reports one,
    /// otherwise the mean. For deterministic engines (Callgrind instruction
    /// counts and cache/branch events, `alloc_tracker` allocations) it is the
    /// exact measured count.
    pub value: f64,
    /// Estimated standard deviation of the measurement, when the engine reports
    /// one (Criterion, `all_the_time`). Absent for deterministic engines.
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
/// Each kind has exactly one unit (see [`as_unit`](Self::as_unit)) and one
/// comparison polarity (see [`higher_is_better`](Self::higher_is_better)).
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    /// Wall-clock time per iteration in nanoseconds (Criterion); hardware-dependent
    /// and noisy.
    WallTime,
    /// Processor time per iteration in nanoseconds (`all_the_time`);
    /// hardware-dependent and noisy.
    ProcessorTime,
    /// Retired instruction count (Callgrind); deterministic.
    InstructionCount,
    /// Estimated CPU cycles from the Callgrind cache model; deterministic.
    EstimatedCycles,
    /// Accesses served by the L1 cache (Callgrind); the cheap outcome, so higher
    /// is better.
    L1CacheHits,
    /// Accesses served by the last-level cache (Callgrind); an L1 miss escalating
    /// to slower memory, so lower is better.
    LastLevelCacheHits,
    /// Accesses served by main memory (Callgrind); the most expensive tier, so
    /// lower is better.
    RamHits,
    /// Executed conditional branches (Callgrind); deterministic.
    ConditionalBranches,
    /// Mispredicted conditional branches (Callgrind); deterministic.
    ConditionalBranchMisses,
    /// Executed indirect branches (Callgrind); deterministic.
    IndirectBranches,
    /// Mispredicted indirect branches (Callgrind); deterministic.
    IndirectBranchMisses,
    /// Bytes allocated per iteration (`alloc_tracker`); deterministic.
    AllocatedBytes,
    /// Allocation count per iteration (`alloc_tracker`); deterministic.
    AllocationCount,
}

impl MetricKind {
    /// The stable `snake_case` label for this kind, matching its serialized wire
    /// name. Used as the metric's display name in reports.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WallTime => "wall_time",
            Self::ProcessorTime => "processor_time",
            Self::InstructionCount => "instruction_count",
            Self::EstimatedCycles => "estimated_cycles",
            Self::L1CacheHits => "l1_cache_hits",
            Self::LastLevelCacheHits => "last_level_cache_hits",
            Self::RamHits => "ram_hits",
            Self::ConditionalBranches => "conditional_branches",
            Self::ConditionalBranchMisses => "conditional_branch_misses",
            Self::IndirectBranches => "indirect_branches",
            Self::IndirectBranchMisses => "indirect_branch_misses",
            Self::AllocatedBytes => "allocated_bytes",
            Self::AllocationCount => "allocation_count",
        }
    }

    /// The unit this kind is always measured in, for display.
    #[must_use]
    pub fn as_unit(self) -> &'static str {
        match self {
            Self::WallTime | Self::ProcessorTime => "ns",
            Self::AllocatedBytes => "bytes",
            Self::InstructionCount
            | Self::EstimatedCycles
            | Self::L1CacheHits
            | Self::LastLevelCacheHits
            | Self::RamHits
            | Self::ConditionalBranches
            | Self::ConditionalBranchMisses
            | Self::IndirectBranches
            | Self::IndirectBranchMisses
            | Self::AllocationCount => "count",
        }
    }

    /// Whether a *rise* in this metric is an improvement.
    ///
    /// Only L1 cache hits are higher-is-better: an access served by L1 is the
    /// cheap outcome. Every other kind (times, instruction and cycle counts,
    /// last-level and RAM hits, branch counts and misses, allocations) is
    /// lower-is-better.
    #[must_use]
    pub fn higher_is_better(self) -> bool {
        matches!(self, Self::L1CacheHits)
    }

    /// Whether this kind is measured by a deterministic engine.
    ///
    /// Wall time and processor time are the noisy metrics; every Callgrind- and
    /// `alloc_tracker`-derived metric is exact. Analysis uses this to decide
    /// whether a change needs noise-aware gating or can be judged exactly.
    #[must_use]
    pub fn is_deterministic(self) -> bool {
        !matches!(self, Self::WallTime | Self::ProcessorTime)
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
        for kind in [
            MetricKind::WallTime,
            MetricKind::ProcessorTime,
            MetricKind::InstructionCount,
            MetricKind::EstimatedCycles,
            MetricKind::L1CacheHits,
            MetricKind::LastLevelCacheHits,
            MetricKind::RamHits,
            MetricKind::ConditionalBranches,
            MetricKind::ConditionalBranchMisses,
            MetricKind::IndirectBranches,
            MetricKind::IndirectBranchMisses,
            MetricKind::AllocatedBytes,
            MetricKind::AllocationCount,
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            let parsed: MetricKind = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn metric_kind_units_are_fixed_per_kind() {
        assert_eq!(MetricKind::WallTime.as_unit(), "ns");
        assert_eq!(MetricKind::ProcessorTime.as_unit(), "ns");
        assert_eq!(MetricKind::AllocatedBytes.as_unit(), "bytes");
        assert_eq!(MetricKind::InstructionCount.as_unit(), "count");
        assert_eq!(MetricKind::L1CacheHits.as_unit(), "count");
    }

    #[test]
    fn only_l1_cache_hits_is_higher_is_better() {
        assert!(MetricKind::L1CacheHits.higher_is_better());
        for kind in [
            MetricKind::WallTime,
            MetricKind::ProcessorTime,
            MetricKind::InstructionCount,
            MetricKind::EstimatedCycles,
            MetricKind::LastLevelCacheHits,
            MetricKind::RamHits,
            MetricKind::ConditionalBranches,
            MetricKind::ConditionalBranchMisses,
            MetricKind::IndirectBranches,
            MetricKind::IndirectBranchMisses,
            MetricKind::AllocatedBytes,
            MetricKind::AllocationCount,
        ] {
            assert!(!kind.higher_is_better(), "{kind:?}");
        }
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
