//! Parsing of `alloc_tracker`'s per-operation JSON into the engine-neutral
//! [`BenchmarkResult`] model.
//!
//! `alloc_tracker` writes one file per operation under `target/alloc_tracker/`,
//! recording how many bytes and how many allocations a benchmark performs per
//! iteration. Both quantities are a deterministic property of the code, so they
//! are stored without a machine key (see [`Engine::is_hardware_dependent`]).
//! The committed fixtures under `tests/fixtures/alloc_tracker/` are real
//! `alloc_tracker` output and act as a schema-drift canary.
//!
//! [`Engine::is_hardware_dependent`]: crate::model::Engine::is_hardware_dependent

use std::error::Error;
use std::fmt;

use nonempty::NonEmpty;
use serde::Deserialize;

use crate::model::{BenchmarkId, BenchmarkResult, Metric, MetricKind};

/// An error encountered while parsing an `alloc_tracker` operation file.
#[derive(Debug)]
pub(crate) struct AllocTrackerParseError(serde_json::Error);

impl fmt::Display for AllocTrackerParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "failed to parse alloc_tracker output: {}", self.0)
    }
}

impl Error for AllocTrackerParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

/// Parses one `alloc_tracker` operation file into a [`BenchmarkResult`].
///
/// # Errors
///
/// Returns [`AllocTrackerParseError`] if the JSON is malformed or does not match
/// the expected shape.
pub(crate) fn parse_alloc_tracker_operation(
    json: &str,
) -> Result<BenchmarkResult, AllocTrackerParseError> {
    let output: OperationOutput = serde_json::from_str(json).map_err(AllocTrackerParseError)?;
    Ok(output_to_record(&output))
}

/// Maps a parsed operation to a [`BenchmarkResult`] (pure).
///
/// `alloc_tracker`'s flat `target/alloc_tracker/` tree carries no package
/// attribution, so the operation name alone identifies the series (mirroring the
/// Criterion adapter).
fn output_to_record(output: &OperationOutput) -> BenchmarkResult {
    let id = BenchmarkId::new(NonEmpty::new(output.operation.clone()));

    let metrics = vec![
        Metric::new(
            MetricKind::AllocatedBytes,
            as_f64(output.mean_bytes_per_iteration),
        ),
        Metric::new(
            MetricKind::AllocationCount,
            as_f64(output.mean_allocations_per_iteration),
        ),
    ];

    BenchmarkResult::new(id, metrics)
}

/// Casts an allocation statistic to `f64`, the model's storage type.
#[expect(
    clippy::cast_precision_loss,
    reason = "allocation counts and byte totals are well below 2^53; precision loss is irrelevant"
)]
fn as_f64(value: u64) -> f64 {
    value as f64
}

/// The subset of an `alloc_tracker` operation file the tool reads. The `total_*`
/// fields are ignored in favor of the per-iteration means, which are comparable
/// across runs with differing iteration counts.
#[derive(Debug, Deserialize)]
struct OperationOutput {
    operation: String,
    mean_bytes_per_iteration: u64,
    mean_allocations_per_iteration: u64,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "allocation statistics are exact integer-derived counts"
    )]
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use super::*;

    const ALLOCATE_VEC_FIXTURE: &str =
        include_str!("../../tests/fixtures/alloc_tracker/allocate_vec.json");

    #[test]
    fn parses_identity_from_operation_name() {
        let record = parse_alloc_tracker_operation(ALLOCATE_VEC_FIXTURE).unwrap();
        assert_eq!(
            record.id,
            BenchmarkId::new(NonEmpty::new("allocate_vec".to_owned()))
        );
    }

    #[test]
    fn alloc_tracker_identity_is_the_operation_name_only() {
        let record = parse_alloc_tracker_operation(ALLOCATE_VEC_FIXTURE).unwrap();
        assert_eq!(
            record.id,
            BenchmarkId::new(NonEmpty::new("allocate_vec".to_owned()))
        );
    }

    #[test]
    fn maps_both_allocation_metrics_from_the_means() {
        let record = parse_alloc_tracker_operation(ALLOCATE_VEC_FIXTURE).unwrap();
        assert_eq!(record.metrics.len(), 2);

        let bytes = metric(&record, MetricKind::AllocatedBytes);
        assert_eq!(bytes.value, 200.0);

        let count = metric(&record, MetricKind::AllocationCount);
        assert_eq!(count.value, 2.0);
    }

    #[test]
    fn allocation_metrics_carry_no_dispersion() {
        // Allocation counts are deterministic, so no confidence interval or
        // standard deviation is reported.
        let record = parse_alloc_tracker_operation(ALLOCATE_VEC_FIXTURE).unwrap();
        for metric in &record.metrics {
            assert_eq!(metric.std_dev, None);
            assert_eq!(metric.interval_low, None);
            assert_eq!(metric.interval_high, None);
        }
    }

    #[test]
    fn preserves_the_original_operation_name() {
        let json = operation_json("group/case name", 4096, 7);
        let record = parse_alloc_tracker_operation(&json).unwrap();
        assert_eq!(
            record.id,
            BenchmarkId::new(NonEmpty::new("group/case name".to_owned()))
        );
    }

    #[test]
    fn rejects_malformed_json() {
        let error = parse_alloc_tracker_operation("{ not json").unwrap_err();
        assert!(
            error.to_string().contains("failed to parse alloc_tracker"),
            "{error}"
        );
        assert!(error.source().is_some());
    }

    fn metric(record: &BenchmarkResult, kind: MetricKind) -> &Metric {
        record
            .metrics
            .iter()
            .find(|metric| metric.kind == kind)
            .unwrap_or_else(|| panic!("missing metric {kind:?}"))
    }

    fn operation_json(operation: &str, bytes: u64, count: u64) -> String {
        format!(
            "{{\"operation\":\"{operation}\",\"total_iterations\":4,\
             \"total_bytes_allocated\":{},\"total_allocations_count\":{},\
             \"mean_bytes_per_iteration\":{bytes},\"mean_allocations_per_iteration\":{count}}}",
            bytes.saturating_mul(4),
            count.saturating_mul(4),
        )
    }
}
