//! The data model: a benchmark run reduced to a stable identity and a set of
//! named numeric metrics, plus the immutable result set stored per run.

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
/// values are equal, so the components must be reproducible across runs. For
/// Callgrind these are the Gungraun `module_path`, `function_name`, and optional
/// `id`; for Criterion they are the `group_id`, `function_id`, and `value_str`.
#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BenchmarkId {
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
    pub fn new(group: String, case: Option<String>, value: Option<String>) -> Self {
        Self { group, case, value }
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
}

impl Metric {
    /// Creates a metric.
    #[must_use]
    pub fn new(name: String, kind: MetricKind, value: f64, unit: Option<String>) -> Self {
        Self {
            name,
            kind,
            value,
            unit,
        }
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
            BenchmarkId::new("nm::observe".to_owned(), Some("pull".to_owned()), None),
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
}
