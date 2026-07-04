//! Parsing of `alloc_tracker`'s per-operation JSON into the engine-neutral
//! [`BenchmarkResult`] model.
//!
//! `alloc_tracker` writes one file per operation under `target/alloc_tracker/`,
//! recording how many bytes and how many allocations a benchmark performs per
//! iteration, together with a per-iteration slope and its confidence interval
//! estimated over the operation's measured spans. Allocation behavior does not
//! depend on the host hardware, so this engine stores its history without a
//! machine key (see [`Engine::is_hardware_dependent`]). It is *not* deterministic,
//! however: warmup and buffer-resize allocations are amortized over a
//! Criterion-chosen iteration count, so the per-iteration figures jitter run to
//! run. The adapter therefore prefers the warmup-robust slope. Multi-span output
//! carries a confidence interval, so the adapter normally reads one; the interval
//! fields are parsed as optional to tolerate single-span or legacy mean-only
//! files, which then fall back to a single figure. The committed fixtures under
//! `tests/fixtures/alloc_tracker/` are real `alloc_tracker` output and act as a
//! schema-drift canary.
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
///
/// For each metric the through-origin slope is preferred as the per-iteration
/// point estimate, matching the Criterion and `all_the_time` adapters; output
/// that records no slope falls back to the mean. When the confidence interval is
/// present it is recorded on the metric, so analysis can apply its interval-overlap
/// gate to allocation figures the same way it does for wall time.
fn output_to_record(output: &OperationOutput) -> BenchmarkResult {
    let bytes_value = output
        .slope_bytes_per_iteration
        .unwrap_or_else(|| as_f64(output.mean_bytes_per_iteration));
    let bytes = Metric::new(MetricKind::AllocatedBytes, bytes_value).with_dispersion(
        None,
        output.interval_low_bytes_per_iteration,
        output.interval_high_bytes_per_iteration,
    );

    let allocations_value = output
        .slope_allocations_per_iteration
        .unwrap_or_else(|| as_f64(output.mean_allocations_per_iteration));
    let allocations = Metric::new(MetricKind::AllocationCount, allocations_value).with_dispersion(
        None,
        output.interval_low_allocations_per_iteration,
        output.interval_high_allocations_per_iteration,
    );

    let id = BenchmarkId::new(NonEmpty::new(output.operation.clone()));
    BenchmarkResult::new(id, vec![bytes, allocations])
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
/// fields are ignored in favor of the per-iteration slope (or mean), which is
/// comparable across runs with differing iteration counts. The dispersion fields
/// are optional so that output recording only a mean still parses.
#[derive(Debug, Deserialize)]
struct OperationOutput {
    operation: String,
    mean_bytes_per_iteration: u64,
    mean_allocations_per_iteration: u64,
    #[serde(default)]
    slope_bytes_per_iteration: Option<f64>,
    #[serde(default)]
    interval_low_bytes_per_iteration: Option<f64>,
    #[serde(default)]
    interval_high_bytes_per_iteration: Option<f64>,
    #[serde(default)]
    slope_allocations_per_iteration: Option<f64>,
    #[serde(default)]
    interval_low_allocations_per_iteration: Option<f64>,
    #[serde(default)]
    interval_high_allocations_per_iteration: Option<f64>,
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

    const ALLOCATE_VEC_DISPERSION_FIXTURE: &str =
        include_str!("../../tests/fixtures/alloc_tracker/allocate_vec_dispersion.json");

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
    fn minimal_output_carries_no_dispersion() {
        // Output that records only means (no slope or interval) carries no
        // dispersion, so each point estimate falls back to the mean and analysis
        // relies on rank-testing across regimes rather than interval overlap.
        let record = parse_alloc_tracker_operation(ALLOCATE_VEC_FIXTURE).unwrap();
        for metric in &record.metrics {
            assert_eq!(metric.std_dev, None);
            assert_eq!(metric.interval_low, None);
            assert_eq!(metric.interval_high, None);
        }
    }

    #[test]
    fn records_dispersion_when_present() {
        let record = parse_alloc_tracker_operation(ALLOCATE_VEC_DISPERSION_FIXTURE).unwrap();

        let bytes = metric(&record, MetricKind::AllocatedBytes);
        // The slope is preferred as the point estimate, and the bytes metric
        // carries the jitter from warmup and buffer-resize allocations as an
        // analytic confidence interval straddling the slope. The adapter no longer
        // surfaces a standard deviation.
        assert_eq!(bytes.value, 200.0);
        assert_eq!(bytes.std_dev, None);
        assert_eq!(bytes.interval_low, Some(199.346_678_671_825_4));
        assert_eq!(bytes.interval_high, Some(200.653_321_328_174_6));

        let count = metric(&record, MetricKind::AllocationCount);
        // The allocation count is stable across spans, so its interval collapses
        // onto the slope.
        assert_eq!(count.value, 2.0);
        assert_eq!(count.std_dev, None);
        assert_eq!(count.interval_low, Some(2.0));
        assert_eq!(count.interval_high, Some(2.0));
    }

    #[test]
    fn prefers_the_slope_over_the_mean() {
        // Slopes distinct from the means prove the slope is the chosen point
        // estimate for each metric, matching the Criterion adapter's preference.
        let json = concat!(
            "{\"operation\":\"op\",\"mean_bytes_per_iteration\":200,",
            "\"mean_allocations_per_iteration\":2,",
            "\"slope_bytes_per_iteration\":201.5,",
            "\"slope_allocations_per_iteration\":3.25}"
        );
        let record = parse_alloc_tracker_operation(json).unwrap();
        assert_eq!(metric(&record, MetricKind::AllocatedBytes).value, 201.5);
        assert_eq!(metric(&record, MetricKind::AllocationCount).value, 3.25);
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
