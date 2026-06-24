//! The benchmark run: the immutable unit of storage, one file per invocation.

use serde::{Deserialize, Serialize};

use crate::model::{BenchmarkId, Metric, RunContext};

/// Schema version of the stored [`Run`] JSON.
///
/// Bumped whenever the on-disk representation changes in a backward-incompatible
/// way so that `analyze` can refuse or migrate older data.
pub const SCHEMA_VERSION: u32 = 1;

/// A complete benchmark run: the unit of storage (one immutable file per run).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Run {
    /// Schema version of this record (see [`SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Context describing where, when, and against which commit the run happened.
    pub context: RunContext,
    /// One result per benchmark case measured in this run.
    pub results: Vec<BenchmarkResult>,
}

impl Run {
    /// Creates a run stamped with the current [`SCHEMA_VERSION`].
    #[must_use]
    pub fn new(context: RunContext, results: Vec<BenchmarkResult>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            context,
            results,
        }
    }

    /// Serializes this run to pretty-printed JSON, the on-disk format.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Deserializes a run from its JSON representation.
    ///
    /// # Errors
    ///
    /// Returns an error if `json` is not a valid serialized run.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

/// A single benchmark case: a stable identity plus its measured metrics.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BenchmarkResult {
    /// Stable identity of the benchmark case (its series key).
    pub id: BenchmarkId,
    /// Metrics captured for this case in this run.
    pub metrics: Vec<Metric>,
}

impl BenchmarkResult {
    /// Creates a result for `id` carrying `metrics`.
    #[must_use]
    pub fn new(id: BenchmarkId, metrics: Vec<Metric>) -> Self {
        Self { id, metrics }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use nonempty::nonempty;

    use super::*;
    use crate::model::{EnvironmentInfo, GitInfo, MetricKind, ToolchainInfo};

    fn sample_context() -> RunContext {
        let epoch = "2024-01-01T00:00:00Z".parse().unwrap();
        RunContext::new(
            epoch,
            epoch,
            GitInfo::default(),
            EnvironmentInfo::default(),
            ToolchainInfo::default(),
            "0.0.1".to_owned(),
        )
    }

    #[test]
    fn run_new_stamps_schema_version() {
        let run = Run::new(sample_context(), Vec::new());
        assert_eq!(run.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn run_json_roundtrip() {
        let result = BenchmarkResult::new(
            BenchmarkId::new(nonempty![
                "nm".to_owned(),
                "nm::observe".to_owned(),
                "pull".to_owned(),
            ]),
            vec![Metric::new(MetricKind::InstructionCount, 1234.0)],
        );
        let run = Run::new(sample_context(), vec![result]);

        let json = run.to_json().unwrap();
        let parsed = Run::from_json(&json).unwrap();

        assert_eq!(parsed, run);
    }
}
