//! The data model: a benchmark run reduced to a stable identity and a set of
//! named numeric metrics, plus the immutable result set stored per run.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::RunContext;

/// Schema version of the stored [`ResultSet`] JSON.
///
/// Bumped whenever the on-disk representation changes in a backward-incompatible
/// way so that `analyze` can refuse or migrate older data.
pub const SCHEMA_VERSION: u32 = 1;

/// A complete benchmark run: the unit of storage (one immutable file per run).
#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ResultSet {
    /// Schema version of this record (see [`SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Context describing where, when, and against which commit the run happened.
    pub context: RunContext,
    /// One record per benchmark case measured in this run.
    pub results: Vec<ResultRecord>,
}

impl ResultSet {
    /// Creates a result set stamped with the current [`SCHEMA_VERSION`].
    #[must_use]
    pub fn new(context: RunContext, results: Vec<ResultRecord>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            context,
            results,
        }
    }

    /// Serializes this result set to pretty-printed JSON, the on-disk format.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Deserializes a result set from its JSON representation.
    ///
    /// # Errors
    ///
    /// Returns an error if `json` is not a valid serialized result set.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

/// A single benchmark case: a stable identity plus its measured metrics.
#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ResultRecord {
    /// Stable identity of the benchmark case (its series key).
    pub id: BenchmarkId,
    /// Metrics captured for this case in this run.
    pub metrics: Vec<Metric>,
}

impl ResultRecord {
    /// Creates a record for `id` carrying `metrics`.
    #[must_use]
    pub fn new(id: BenchmarkId, metrics: Vec<Metric>) -> Self {
        Self { id, metrics }
    }
}

/// Stable identity of a benchmark series.
///
/// Two runs contribute to the same series if and only if their `BenchmarkId`
/// values are equal, so the components must be reproducible across runs *and*
/// uniquely identify the benchmark within its project.
///
/// The components are kept as separate fields (rather than a single opaque
/// string) so that callers can render the identity at whatever granularity suits
/// them — the full [`qualified`](Self::qualified) form for disambiguation, or the
/// compact [`short`](Self::short) tail for dense listings.
///
/// `package` scopes the identity to the workspace package the benchmark belongs
/// to. Without it, two equally named bench targets in different packages (for
/// example `foo/benches/a.rs` and `bar/benches/a.rs`, each defining the same
/// function) would share a `module_path` and silently merge into one series.
/// For Callgrind it is the final component of the Gungraun `package_dir`; for
/// Criterion it is the package the benchmark binary belongs to. It is optional so
/// that summaries lacking the information degrade gracefully rather than fail.
///
/// The remaining components are, for Callgrind, the Gungraun `module_path`,
/// `function_name`, and optional `id`; for Criterion they are the `group_id`,
/// `function_id`, and `value_str`.
#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BenchmarkId {
    /// Workspace package the benchmark belongs to, scoping the identity so that
    /// equally named bench targets in different packages stay distinct.
    #[serde(default)]
    pub package: Option<String>,
    /// Primary grouping component (Callgrind Gungraun `module_path`; Criterion
    /// `group_id`).
    pub group: String,
    /// Finer-grained component (Callgrind Gungraun `function_name`; Criterion
    /// `function_id`).
    pub case: Option<String>,
    /// Optional parameter/value component (Callgrind Gungraun `id`; Criterion
    /// `value_str`).
    pub value: Option<String>,
}

impl BenchmarkId {
    /// Creates a benchmark identity from its components.
    #[must_use]
    pub fn new(
        package: Option<String>,
        group: String,
        case: Option<String>,
        value: Option<String>,
    ) -> Self {
        Self {
            package,
            group,
            case,
            value,
        }
    }

    /// The fully qualified identity: every present component joined by `/`,
    /// scoped by the package. This form uniquely identifies the series and is
    /// what reports use so that cross-package collisions stay visible.
    #[must_use]
    pub fn qualified(&self) -> String {
        let mut parts: Vec<&str> = Vec::with_capacity(4);
        if let Some(package) = &self.package {
            parts.push(package);
        }
        parts.push(&self.group);
        if let Some(case) = &self.case {
            parts.push(case);
        }
        if let Some(value) = &self.value {
            parts.push(value);
        }
        parts.join("/")
    }

    /// A compact, human-friendly label: the function/case (or the last segment of
    /// `group` when no case is present), suffixed by the parameter value if any.
    ///
    /// This drops the package and module path for readability, so it may be
    /// ambiguous across packages; use [`qualified`](Self::qualified) when a unique
    /// label is required.
    #[must_use]
    pub fn short(&self) -> String {
        let head = self.case.as_deref().unwrap_or_else(|| {
            self.group
                .rsplit("::")
                .next()
                .filter(|segment| !segment.is_empty())
                .unwrap_or(&self.group)
        });
        match &self.value {
            Some(value) => format!("{head}/{value}"),
            None => head.to_owned(),
        }
    }
}

impl fmt::Display for BenchmarkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.qualified())
    }
}

/// A single measured quantity.
#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Metric {
    /// Engine-specific metric name (e.g. `instructions`, `wall_time`).
    pub name: String,
    /// What kind of quantity this is (governs comparison semantics).
    pub kind: MetricKind,
    /// The measured value, in the unit implied by `kind`/`unit`.
    pub value: f64,
    /// Optional unit label for display (e.g. `ns`, `count`).
    pub unit: Option<String>,
    /// Estimated standard deviation of the measurement, when the engine reports
    /// one (Criterion). Absent for deterministic engines (Callgrind).
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
    pub fn new(name: String, kind: MetricKind, value: f64, unit: Option<String>) -> Self {
        Self {
            name,
            kind,
            value,
            unit,
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

/// The category of a [`Metric`], which determines how it is compared over time.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    /// Wall-clock time per iteration (Criterion); hardware-dependent and noisy.
    WallTime,
    /// Retired instruction count (Callgrind); deterministic.
    InstructionCount,
    /// Estimated CPU cycles from the Callgrind cache model.
    EstimatedCycles,
    /// Cache hit counts (Callgrind); higher is better.
    CacheEvents,
    /// Branch / branch-miss counts (Callgrind).
    Branches,
    /// Bytes allocated per iteration (`alloc_tracker`); deterministic.
    AllocationBytes,
    /// Allocation count per iteration (`alloc_tracker`); deterministic.
    AllocationCount,
    /// Processor time per iteration (`all_the_time`); hardware-dependent and noisy.
    ProcessorTime,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::{CiInfo, GitInfo, Timestamps, ToolchainInfo};

    use super::*;

    fn sample_context() -> RunContext {
        let epoch = "2024-01-01T00:00:00Z".parse().unwrap();
        RunContext::new(
            Timestamps::new(epoch, epoch, epoch),
            GitInfo::default(),
            CiInfo::default(),
            ToolchainInfo::default(),
            "0.0.1".to_owned(),
        )
    }

    #[test]
    fn result_set_new_stamps_schema_version() {
        let set = ResultSet::new(sample_context(), Vec::new());
        assert_eq!(set.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn result_set_json_roundtrip() {
        let record = ResultRecord::new(
            BenchmarkId::new(
                Some("nm".to_owned()),
                "nm::observe".to_owned(),
                Some("pull".to_owned()),
                None,
            ),
            vec![Metric::new(
                "instructions".to_owned(),
                MetricKind::InstructionCount,
                1234.0,
                Some("count".to_owned()),
            )],
        );
        let set = ResultSet::new(sample_context(), vec![record]);

        let json = set.to_json().unwrap();
        let parsed = ResultSet::from_json(&json).unwrap();

        assert_eq!(parsed, set);
    }

    #[test]
    fn metric_kind_serializes_snake_case() {
        let json = serde_json::to_string(&MetricKind::InstructionCount).unwrap();
        assert_eq!(json, "\"instruction_count\"");
    }

    #[test]
    fn new_engine_metric_kinds_serialize_snake_case() {
        // The `alloc_tracker` and `all_the_time` engines round-trip through the
        // stored JSON, so their kinds must keep their snake_case wire names.
        for (kind, expected) in [
            (MetricKind::AllocationBytes, "\"allocation_bytes\""),
            (MetricKind::AllocationCount, "\"allocation_count\""),
            (MetricKind::ProcessorTime, "\"processor_time\""),
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, expected);
            let parsed: MetricKind = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn metric_dispersion_is_omitted_when_absent() {
        let metric = Metric::new("ir".to_owned(), MetricKind::InstructionCount, 1.0, None);
        let json = serde_json::to_string(&metric).unwrap();
        assert!(!json.contains("std_dev"), "{json}");
        assert!(!json.contains("interval_low"), "{json}");
    }

    #[test]
    fn metric_dispersion_roundtrips_when_present() {
        let metric = Metric::new(
            "wall_time".to_owned(),
            MetricKind::WallTime,
            26.9,
            Some("ns".to_owned()),
        )
        .with_dispersion(Some(0.47), Some(26.6), Some(27.2));

        let json = serde_json::to_string(&metric).unwrap();
        let parsed: Metric = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, metric);
        assert_eq!(parsed.std_dev, Some(0.47));
        assert_eq!(parsed.interval_low, Some(26.6));
        assert_eq!(parsed.interval_high, Some(27.2));
    }

    #[test]
    fn package_distinguishes_otherwise_equal_ids() {
        let foo = BenchmarkId::new(
            Some("foo".to_owned()),
            "a::bench".to_owned(),
            Some("run".to_owned()),
            None,
        );
        let bar = BenchmarkId::new(
            Some("bar".to_owned()),
            "a::bench".to_owned(),
            Some("run".to_owned()),
            None,
        );

        assert_ne!(foo, bar);
    }

    #[test]
    fn ids_sort_by_package_first() {
        let bar = BenchmarkId::new(Some("bar".to_owned()), "z".to_owned(), None, None);
        let foo = BenchmarkId::new(Some("foo".to_owned()), "a".to_owned(), None, None);

        let mut ids = vec![foo.clone(), bar.clone()];
        ids.sort();

        assert_eq!(ids, vec![bar, foo]);
    }

    #[test]
    fn qualified_joins_present_components() {
        let id = BenchmarkId::new(
            Some("fast_time".to_owned()),
            "a::group".to_owned(),
            Some("capture".to_owned()),
            Some("two_instants".to_owned()),
        );
        assert_eq!(id.qualified(), "fast_time/a::group/capture/two_instants");
        assert_eq!(id.to_string(), id.qualified());
    }

    #[test]
    fn qualified_omits_absent_components() {
        let id = BenchmarkId::new(None, "a::group".to_owned(), None, None);
        assert_eq!(id.qualified(), "a::group");
    }

    #[test]
    fn short_uses_case_and_value() {
        let id = BenchmarkId::new(
            Some("fast_time".to_owned()),
            "a::group".to_owned(),
            Some("capture".to_owned()),
            Some("two_instants".to_owned()),
        );
        assert_eq!(id.short(), "capture/two_instants");
    }

    #[test]
    fn short_falls_back_to_last_group_segment() {
        let id = BenchmarkId::new(
            Some("fast_time".to_owned()),
            "a::group".to_owned(),
            None,
            None,
        );
        assert_eq!(id.short(), "group");
    }

    #[test]
    fn short_keeps_the_whole_group_when_it_ends_in_a_separator() {
        // `rsplit("::")` yields an empty trailing segment for "a::"; the
        // `filter(!is_empty)` fallback must return the whole group, not "".
        let id = BenchmarkId::new(None, "a::".to_owned(), None, None);
        assert_eq!(id.short(), "a::");
    }

    #[test]
    fn deserializes_legacy_id_without_package() {
        let id: BenchmarkId =
            serde_json::from_str(r#"{"group":"a::group","case":"capture","value":null}"#).unwrap();
        assert_eq!(id.package, None);
        assert_eq!(id.group, "a::group");
    }
}
