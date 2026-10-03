//! Self-contained evidence of one successful, clean collection execution.

use std::collections::BTreeSet;

use serde::de::Error as _;
use serde::{Deserialize, Serialize};

use crate::{
    BenchmarkId, BenchmarkResult, DiscriminantSet, Engine, MachineKey, MetricList, Run, RunContext,
    TargetTriple, sanitize_segment,
};

/// Exact measurements captured by one collection, independent of shared storage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CollectionWire")]
pub struct CollectionSnapshot {
    version: u32,
    project: String,
    commit: String,
    target_triple: TargetTriple,
    machine_key: MachineKey,
    runs: Vec<CollectionRun>,
}

impl CollectionSnapshot {
    /// Captures the clean collection's identity and its freshly measured runs.
    ///
    /// Empty collections retain their execution identity without inventing measurements.
    ///
    /// # Errors
    ///
    /// Returns an error when identities or measurement records are inconsistent.
    pub fn new(
        project: &str,
        commit: &str,
        target_triple: TargetTriple,
        machine_key: MachineKey,
        runs: Vec<(Engine, Run)>,
    ) -> Result<Self, serde_json::Error> {
        Self::try_from(CollectionWire {
            version: COLLECTION_VERSION,
            project: sanitize_segment(project),
            commit: commit.to_owned(),
            target_triple,
            machine_key,
            runs: runs
                .into_iter()
                .map(|(engine, run)| CollectionRun {
                    engine,
                    context: run.context,
                    results: run
                        .results
                        .into_iter()
                        .map(|result| CollectionResult {
                            id: result.id,
                            metrics: result.metrics,
                        })
                        .collect(),
                })
                .collect(),
        })
    }

    /// Decodes a supported snapshot, validating its entire measurement envelope.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed or inconsistent evidence.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Serializes the self-contained measurement envelope.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// The canonical project namespace that owns these measurements.
    #[must_use]
    pub fn project(&self) -> &str {
        &self.project
    }

    /// The full clean commit measured by this execution.
    #[must_use]
    pub fn commit(&self) -> &str {
        &self.commit
    }

    /// The hardware fingerprint captured during this execution.
    #[must_use]
    pub fn machine_key(&self) -> &MachineKey {
        &self.machine_key
    }

    /// Reconstructs only the engine runs measured by this execution.
    pub fn runs(&self) -> impl Iterator<Item = (DiscriminantSet, Run)> + '_ {
        self.runs.iter().map(|run| {
            (
                DiscriminantSet::new(run.engine, &self.target_triple, &self.machine_key),
                Run::new(
                    run.context.clone(),
                    run.results
                        .iter()
                        .map(|result| {
                            BenchmarkResult::new(result.id.clone(), result.metrics.clone())
                        })
                        .collect(),
                ),
            )
        })
    }
}

impl TryFrom<CollectionWire> for CollectionSnapshot {
    type Error = serde_json::Error;

    fn try_from(raw: CollectionWire) -> Result<Self, Self::Error> {
        // This is a handoff between paired tools, not a tolerant historical-data reader.
        // Reject unknown versions and metric kinds rather than silently shrinking the roster.
        if raw.version != COLLECTION_VERSION
            || raw.project != sanitize_segment(&raw.project)
            || raw.commit.len() != COMMIT_HEX_LENGTH
            || !raw.commit.bytes().all(|value| value.is_ascii_hexdigit())
            || raw.target_triple.as_str().is_empty()
            || raw.target_triple.as_str() != sanitize_segment(raw.target_triple.as_str())
            || raw.machine_key.as_str().len() != MACHINE_KEY_HEX_LENGTH
            || !raw
                .machine_key
                .as_str()
                .bytes()
                .all(|value| value.is_ascii_hexdigit())
        {
            return Err(Self::Error::custom(
                "invalid collection identity or version",
            ));
        }
        let mut engines = BTreeSet::new();
        for run in &raw.runs {
            if !engines.insert(run.engine)
                || run.context.git.dirty
                || run.context.git.commit.as_deref() != Some(raw.commit.as_str())
                || run.context.toolchain.target_triple != raw.target_triple
                || run
                    .context
                    .machine
                    .as_ref()
                    .map(|machine| machine.fingerprint.as_str())
                    != Some(raw.machine_key.as_str())
                || run.context.best_of.is_none()
                || run.results.is_empty()
            {
                return Err(Self::Error::custom("inconsistent collection run"));
            }
            let mut ids = BTreeSet::new();
            for result in &run.results {
                if !ids.insert(&result.id) || result.metrics.is_empty() {
                    return Err(Self::Error::custom(
                        "empty or duplicate collection benchmark",
                    ));
                }
                let mut kinds = BTreeSet::new();
                for metric in &result.metrics {
                    if !kinds.insert(metric.kind)
                        || !metric.value.is_finite()
                        || metric.std_dev.is_some_and(|value| !value.is_finite())
                        || metric.interval_low.is_some_and(|value| !value.is_finite())
                        || metric.interval_high.is_some_and(|value| !value.is_finite())
                    {
                        return Err(Self::Error::custom(
                            "invalid or duplicate collection metric",
                        ));
                    }
                }
            }
        }
        Ok(Self {
            version: raw.version,
            project: raw.project,
            commit: raw.commit,
            target_triple: raw.target_triple,
            machine_key: raw.machine_key,
            runs: raw.runs,
        })
    }
}

// Independent of the shared-store schema: this format requires exact current evidence.
const COLLECTION_VERSION: u32 = 1;
// Full Git SHA-1 and the collector's hardware-fingerprint wire formats.
const COMMIT_HEX_LENGTH: usize = 40;
const MACHINE_KEY_HEX_LENGTH: usize = 16;

/// Unvalidated transport envelope, including identity for a successful empty collection.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectionWire {
    version: u32,
    project: String,
    commit: String,
    target_triple: TargetTriple,
    machine_key: MachineKey,
    runs: Vec<CollectionRun>,
}

/// One engine's measured payload, retaining the execution's full context.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectionRun {
    engine: Engine,
    context: RunContext,
    results: Vec<CollectionResult>,
}

/// Strict current metrics, unlike historical runs that tolerate retired metric names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectionResult {
    id: BenchmarkId,
    metrics: MetricList,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::indexing_slicing,
        clippy::float_cmp,
        reason = "fixed snapshot fixtures with exact values"
    )]
    use std::num::NonZero;
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use nonempty::nonempty;
    use serde_json::json;

    use super::*;
    use crate::{EnvironmentInfo, GitInfo, MachineInfo, Metric, MetricKind, ToolchainInfo};

    static_assertions::assert_impl_all!(CollectionSnapshot: Send, Sync, UnwindSafe, RefUnwindSafe);

    fn snapshot() -> CollectionSnapshot {
        let commit = "a".repeat(40);
        let key = "0123456789abcdef";
        let triple = TargetTriple::from("x86_64-unknown-linux-gnu");
        let mut context = RunContext::new(
            "2026-01-01T00:00:00Z".parse().unwrap(),
            GitInfo {
                commit: Some(commit.clone()),
                ..GitInfo::default()
            },
            EnvironmentInfo::default(),
            ToolchainInfo {
                target_triple: triple.clone(),
                ..ToolchainInfo::default()
            },
            "test".to_owned(),
        );
        context.machine = Some(MachineInfo {
            processors: 1,
            memory_regions: 1,
            processor_models: Vec::new(),
            processor_speeds: Vec::new(),
            fingerprint: key.to_owned(),
        });
        context.best_of = Some(NonZero::new(2).unwrap());
        CollectionSnapshot::new(
            "project",
            &commit,
            triple,
            key.into(),
            vec![(
                Engine::Criterion,
                Run::new(
                    context,
                    vec![BenchmarkResult::new(
                        BenchmarkId::new(nonempty!["case".to_owned()]),
                        vec![Metric::new(MetricKind::WallTime, 123.5).with_dispersion(
                            Some(2.0),
                            Some(120.0),
                            Some(126.0),
                        )],
                    )],
                ),
            )],
        )
        .unwrap()
    }

    #[test]
    fn preserves_values_intervals_protocol_and_empty_execution_identity() {
        let original = snapshot();
        let decoded = CollectionSnapshot::from_slice(&original.to_json().unwrap()).unwrap();
        assert_eq!(decoded, original);
        let (_, run) = decoded.runs().next().unwrap();
        assert_eq!(run.results[0].metrics[0].value, 123.5);
        assert_eq!(run.results[0].metrics[0].interval_high, Some(126.0));
        assert_eq!(run.context.best_of.unwrap().get(), 2);
        let empty = CollectionSnapshot::new(
            original.project(),
            original.commit(),
            original.target_triple.clone(),
            original.machine_key.clone(),
            Vec::new(),
        )
        .unwrap();
        assert_eq!(empty.runs().count(), 0);
        assert_eq!(
            CollectionSnapshot::from_slice(&empty.to_json().unwrap()).unwrap(),
            empty
        );
    }

    fn reject_changes(changes: &[(&str, serde_json::Value)]) {
        let raw = serde_json::to_value(snapshot()).unwrap();
        for (pointer, value) in changes {
            let mut invalid = raw.clone();
            *invalid.pointer_mut(pointer).unwrap() = value.clone();
            CollectionSnapshot::from_slice(&serde_json::to_vec(&invalid).unwrap()).unwrap_err();
        }
    }

    #[test]
    fn rejects_corrupt_collection_header() {
        reject_changes(&[
            ("/version", json!(2)),
            ("/project", json!("a/b")),
            ("/commit", json!("bad")),
            ("/target_triple", json!("")),
            ("/machine_key", json!("bad")),
        ]);
    }

    #[test]
    fn rejects_mismatched_or_dirty_run_context() {
        reject_changes(&[
            ("/runs/0/context/git/dirty", json!(true)),
            ("/runs/0/context/git/commit", json!("b".repeat(40))),
            ("/runs/0/context/toolchain/target_triple", json!("other")),
            (
                "/runs/0/context/machine/fingerprint",
                json!("fedcba9876543210"),
            ),
            ("/runs/0/context/best_of", json!(null)),
        ]);
    }

    #[test]
    fn rejects_empty_measurements_and_unknown_metric_kinds() {
        reject_changes(&[
            ("/runs/0/results", json!([])),
            ("/runs/0/results/0/metrics", json!([])),
            ("/runs/0/results/0/metrics/0/kind", json!("future_metric")),
        ]);
    }

    #[test]
    fn rejects_non_hex_identity_with_correct_width() {
        reject_changes(&[
            ("/commit", json!("g".repeat(COMMIT_HEX_LENGTH))),
            ("/machine_key", json!("g".repeat(MACHINE_KEY_HEX_LENGTH))),
        ]);
    }

    #[test]
    fn constructors_reject_nonfinite_measurement_components() {
        let snapshot = snapshot();
        for metric in [
            Metric::new(MetricKind::WallTime, f64::NAN),
            Metric::new(MetricKind::WallTime, 1.0).with_dispersion(Some(f64::INFINITY), None, None),
            Metric::new(MetricKind::WallTime, 1.0).with_dispersion(
                None,
                Some(f64::NEG_INFINITY),
                None,
            ),
            Metric::new(MetricKind::WallTime, 1.0).with_dispersion(None, None, Some(f64::NAN)),
        ] {
            let (_, mut run) = snapshot.runs().next().unwrap();
            run.results[0].metrics = vec![metric].into();
            CollectionSnapshot::new(
                snapshot.project(),
                snapshot.commit(),
                snapshot.target_triple.clone(),
                snapshot.machine_key.clone(),
                vec![(Engine::Criterion, run)],
            )
            .unwrap_err();
        }
    }

    #[test]
    fn rejects_duplicate_engine_benchmark_or_metric() {
        let raw = serde_json::to_value(snapshot()).unwrap();
        for pointer in ["/runs", "/runs/0/results", "/runs/0/results/0/metrics"] {
            let mut invalid = raw.clone();
            let array = invalid
                .pointer_mut(pointer)
                .unwrap()
                .as_array_mut()
                .unwrap();
            array.push(array[0].clone());
            CollectionSnapshot::from_slice(&serde_json::to_vec(&invalid).unwrap()).unwrap_err();
        }
    }

    #[test]
    fn bounded_snapshot_decoder_mutations_preserve_successful_round_trips() {
        let corpus = snapshot().to_json().unwrap();
        // Sweep representative byte positions, including malformed UTF-8 and truncation.
        // Keep representative interpreter coverage within the Miri workload budget.
        let positions = if cfg!(miri) { 4 } else { 128 };
        for index in (0..corpus.len()).step_by(corpus.len().div_ceil(positions)) {
            for replacement in [0, b'"', b']', 255] {
                let mut bytes = corpus.clone();
                bytes[index] = replacement;
                if let Ok(decoded) = CollectionSnapshot::from_slice(&bytes) {
                    assert_eq!(
                        CollectionSnapshot::from_slice(&decoded.to_json().unwrap()).unwrap(),
                        decoded
                    );
                }
            }
            CollectionSnapshot::from_slice(&corpus[..index]).unwrap_err();
        }
    }
}
