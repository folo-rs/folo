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
//! run. The adapter therefore reads the warmup-robust per-iteration slope. Every
//! metric carries a slope; multi-span output additionally carries a confidence
//! interval, so the interval fields are parsed as optional (a single span has no
//! dispersion). The committed fixtures under `tests/fixtures/alloc_tracker/` are
//! representative samples of the current schema; the authoritative schema-drift
//! guard is the `super::schema_roundtrip` test, which feeds real producer output
//! through this parser so a field renamed or dropped on either side of the
//! boundary fails the build.
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
/// For each metric the through-origin slope is the per-iteration point estimate,
/// matching the Criterion and `all_the_time` adapters. A missing or null slope
/// (which the producer writes for a zero-iteration operation that could not run)
/// maps to `NaN`, signalling that no valid per-iteration figure was produced. When
/// the confidence interval is present it is recorded on the metric, so analysis can
/// apply its interval-overlap gate to allocation figures the same way it does for
/// wall time.
fn output_to_record(output: &OperationOutput) -> BenchmarkResult {
    let bytes_value = output.slope_bytes_per_iteration.unwrap_or(f64::NAN);
    let bytes = Metric::new(MetricKind::AllocatedBytes, bytes_value).with_dispersion(
        None,
        output.interval_low_bytes_per_iteration,
        output.interval_high_bytes_per_iteration,
    );

    let allocations_value = output.slope_allocations_per_iteration.unwrap_or(f64::NAN);
    let allocations = Metric::new(MetricKind::AllocationCount, allocations_value).with_dispersion(
        None,
        output.interval_low_allocations_per_iteration,
        output.interval_high_allocations_per_iteration,
    );

    let id = BenchmarkId::new(NonEmpty::new(output.operation.clone()));
    BenchmarkResult::new(id, vec![bytes, allocations])
}

/// The subset of an `alloc_tracker` operation file the tool reads. The `total_*`
/// fields are ignored in favor of the per-iteration slope, which is comparable
/// across runs with differing iteration counts. The slope and dispersion fields are
/// optional so that a zero-iteration operation (null slope) and single-span output
/// (slope but no interval) both parse.
#[derive(Debug, Deserialize)]
struct OperationOutput {
    operation: String,
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
    fn maps_both_allocation_metrics_from_the_slopes() {
        let record = parse_alloc_tracker_operation(ALLOCATE_VEC_FIXTURE).unwrap();
        assert_eq!(record.metrics.len(), 2);

        let bytes = metric(&record, MetricKind::AllocatedBytes);
        assert_eq!(bytes.value, 200.0);

        let count = metric(&record, MetricKind::AllocationCount);
        assert_eq!(count.value, 2.0);
    }

    #[test]
    fn single_span_output_carries_no_dispersion() {
        // Single-span output records a slope but no interval, so each metric carries
        // no dispersion and analysis relies on rank-testing across regimes rather
        // than interval overlap.
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
    fn reads_the_slope_as_the_point_estimate() {
        // The slope is the point estimate for each metric, matching the Criterion
        // adapter.
        let json = concat!(
            "{\"operation\":\"op\",",
            "\"slope_bytes_per_iteration\":201.5,",
            "\"slope_allocations_per_iteration\":3.25}"
        );
        let record = parse_alloc_tracker_operation(json).unwrap();
        assert_eq!(metric(&record, MetricKind::AllocatedBytes).value, 201.5);
        assert_eq!(metric(&record, MetricKind::AllocationCount).value, 3.25);
    }

    #[test]
    fn missing_slope_maps_to_nan() {
        // A zero-iteration operation the workload could not run leaves the slope
        // null (serialized from NaN), which the adapter surfaces as NaN rather than
        // inventing a figure.
        let json = "{\"operation\":\"op\",\"slope_bytes_per_iteration\":null}";
        let record = parse_alloc_tracker_operation(json).unwrap();
        assert!(metric(&record, MetricKind::AllocatedBytes).value.is_nan());
        assert!(metric(&record, MetricKind::AllocationCount).value.is_nan());
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
             \"total_bytes_allocated\":{},\"total_allocations_count\":{},\"span_count\":1,\
             \"slope_bytes_per_iteration\":{bytes},\"slope_allocations_per_iteration\":{count}}}",
            bytes.saturating_mul(4),
            count.saturating_mul(4),
        )
    }
}
