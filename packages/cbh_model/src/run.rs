//! The benchmark run: the immutable unit of storage, one file per invocation.

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::{BenchmarkId, Metric, MetricKind, RunContext};

/// Schema version of the stored [`Run`] JSON.
///
/// Records which on-disk format a file was written with. Reads are not gated on
/// it: version 2 dropped the redundant per-object `commit` timestamp (a run's
/// timeline position is resolved from git topology, keyed by its commit ID),
/// version 3 dropped the redundant `short_commit` (an abbreviation of the full
/// commit ID the analysis already has from the storage key), and version 4 added
/// the optional host-hardware provenance (`context.machine`). Every one of these
/// changes is wire-compatible: older records still deserialize because a removed
/// field is ignored and an added field defaults to absent. The version is bumped
/// so a file's provenance stays legible, not because a read path branches on it.
pub const SCHEMA_VERSION: u32 = 4;

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

/// The metrics carried by a [`BenchmarkResult`].
///
/// A result holds at most one metric per [`MetricKind`], and the noisy
/// single-metric engines (Criterion wall time, `all_the_time` processor time) are
/// the common case, so a small result is kept inline rather than heap allocated.
/// This drops an allocation per result on the deserialization-heavy `analyze`
/// path, where every stored run's results are reconstructed in bulk. Larger
/// (Callgrind) results spill to the heap as usual.
///
/// Its on-disk form is a plain JSON array, identical to a `Vec<Metric>`, so the
/// change is wire-compatible and needs no [`SCHEMA_VERSION`] bump.
///
/// [`MetricKind`]: crate::MetricKind
pub type MetricList = SmallVec<[Metric; 2]>;

/// Deserializes the metric list, silently dropping any metric whose `kind` is not
/// one of the kinds the tool tracks.
///
/// Stored history predates the removal of the build-layout-volatile Callgrind
/// metrics (cache hits per tier, estimated cycles, branch misses), so run files
/// written by an older tool can still carry those kinds. Rather than fail the whole
/// run parse on an unknown-variant error — which would break `analyze`, `list`, and
/// `examine` over any such history — the unknown metrics are skipped, matching the
/// current policy of never persisting them again.
///
/// The raw list is kept inline in a `SmallVec` matching [`MetricList`]'s capacity so
/// the common one- or two-metric case stays allocation-free on this hot read path.
fn deserialize_metrics<'de, D>(deserializer: D) -> Result<MetricList, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = SmallVec::<[RawMetric<'de>; 2]>::deserialize(deserializer)?;
    Ok(raw.into_iter().filter_map(RawMetric::into_metric).collect())
}

/// A metric as it appears on disk, with the kind left as its raw wire name so an
/// unknown kind can be recognized and dropped rather than aborting the parse.
///
/// The kind is borrowed straight from the input JSON (a `&str` tied to the
/// deserializer input): it is only needed transiently to resolve a [`MetricKind`]
/// before this raw form is discarded, so borrowing avoids a `String` allocation per
/// metric on the read path. The stored kind names are a fixed vocabulary of unescaped
/// identifiers, so the borrow always succeeds against the `serde_json::from_str` input
/// backing every run decode.
///
/// [`MetricKind`]: crate::MetricKind
#[derive(Deserialize)]
struct RawMetric<'a> {
    kind: &'a str,
    value: f64,
    #[serde(default)]
    std_dev: Option<f64>,
    #[serde(default)]
    interval_low: Option<f64>,
    #[serde(default)]
    interval_high: Option<f64>,
}

impl RawMetric<'_> {
    /// Converts to a [`Metric`], or `None` when the kind is not one of the kinds the
    /// tool tracks.
    fn into_metric(self) -> Option<Metric> {
        Some(Metric {
            kind: MetricKind::from_name(self.kind)?,
            value: self.value,
            std_dev: self.std_dev,
            interval_low: self.interval_low,
            interval_high: self.interval_high,
        })
    }
}

/// A single benchmark case: a stable identity plus its measured metrics.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BenchmarkResult {
    /// Stable identity of the benchmark case (its series key).
    pub id: BenchmarkId,
    /// Metrics captured for this case in this run.
    #[serde(deserialize_with = "deserialize_metrics")]
    pub metrics: MetricList,
}

impl BenchmarkResult {
    /// Creates a result for `id` carrying `metrics`.
    #[must_use]
    pub fn new(id: BenchmarkId, metrics: impl Into<MetricList>) -> Self {
        Self {
            id,
            metrics: metrics.into(),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use nonempty::nonempty;

    use super::*;
    use crate::{EnvironmentInfo, GitInfo, MachineInfo, MetricKind, ToolchainInfo};

    fn sample_context() -> RunContext {
        let epoch = "2024-01-01T00:00:00Z".parse().unwrap();
        let mut context = RunContext::new(
            epoch,
            GitInfo::default(),
            EnvironmentInfo::default(),
            ToolchainInfo::default(),
            "0.0.1".to_owned(),
        );
        context.machine = Some(MachineInfo {
            processors: 8,
            memory_regions: 1,
            processor_models: vec!["Test CPU 3000".to_owned()],
            processor_speeds: vec![(3141, 8)],
            fingerprint: "test-fingerprint".to_owned(),
        });
        context
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

    #[test]
    fn metrics_serialize_as_a_plain_json_array() {
        // The on-disk form must stay a JSON array, identical to a `Vec<Metric>`,
        // so objects written before the field became a `SmallVec` keep
        // deserializing unchanged.
        let result = BenchmarkResult::new(
            BenchmarkId::new(nonempty!["pkg".to_owned(), "case".to_owned()]),
            vec![
                Metric::new(MetricKind::InstructionCount, 10.0),
                Metric::new(MetricKind::ConditionalBranches, 20.0),
            ],
        );

        let json = serde_json::to_string(&result).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let metrics = value.get("metrics").expect("the metrics field is present");
        assert!(
            metrics.is_array(),
            "metrics must serialize as a JSON array: {json}"
        );
        assert_eq!(metrics.as_array().unwrap().len(), 2);

        let parsed: BenchmarkResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, result);
    }

    #[test]
    fn result_roundtrips_across_the_inline_capacity_boundary() {
        // One and two metrics stay inline; three spill to the heap. Serde must
        // treat all three identically. Each metric uses a distinct kind, as a
        // result carries at most one metric per `MetricKind`.
        const KINDS: [MetricKind; 3] = [
            MetricKind::InstructionCount,
            MetricKind::ConditionalBranches,
            MetricKind::IndirectBranches,
        ];
        for count in [1_usize, 2, 3] {
            let metrics: Vec<Metric> = KINDS
                .iter()
                .take(count)
                .map(|&kind| Metric::new(kind, 1.0))
                .collect();
            let result = BenchmarkResult::new(
                BenchmarkId::new(nonempty!["pkg".to_owned(), "case".to_owned()]),
                metrics,
            );

            let json = serde_json::to_string(&result).unwrap();
            let parsed: BenchmarkResult = serde_json::from_str(&json).unwrap();

            assert_eq!(parsed, result);
            assert_eq!(parsed.metrics.len(), count);
        }
    }

    #[test]
    fn unknown_metric_kinds_are_dropped_on_read() {
        // Legacy history can still carry metric kinds the tool no longer tracks (the
        // build-layout-volatile Callgrind events). Reading must skip them rather than
        // fail the whole run parse, so analysis over old data keeps working.
        let result = BenchmarkResult::new(
            BenchmarkId::new(nonempty!["pkg".to_owned(), "case".to_owned()]),
            vec![Metric::new(MetricKind::InstructionCount, 10.0)],
        );
        let mut value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
        value
            .get_mut("metrics")
            .and_then(serde_json::Value::as_array_mut)
            .unwrap()
            .push(serde_json::json!({ "kind": "estimated_cycles", "value": 907.0 }));

        // Decode through `from_str` (the production run-file path), where the borrowed
        // `&str` kind is read straight from the input rather than an owned `Value`.
        let json = serde_json::to_string(&value).unwrap();
        let parsed: BenchmarkResult = serde_json::from_str(&json).unwrap();
        let kinds: Vec<MetricKind> = parsed.metrics.iter().map(|metric| metric.kind).collect();
        assert_eq!(kinds, vec![MetricKind::InstructionCount]);
    }
}
