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
//! [`Engine::is_hardware_dependent`]: cbh_model::Engine::is_hardware_dependent

use std::error::Error;
use std::fmt;

use cbh_model::{BenchmarkId, BenchmarkResult, Metric, MetricKind};
use nonempty::NonEmpty;
use serde::Deserialize;

/// An error encountered while parsing an `alloc_tracker` operation file.
#[derive(Debug)]
pub struct AllocTrackerParseError(serde_json::Error);

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
/// Returns `Ok(None)` when the operation recorded no usable per-iteration
/// measurement (a zero-iteration run the workload could not complete, which the
/// producer writes with null slopes); such an operation has nothing to store.
///
/// # Errors
///
/// Returns [`AllocTrackerParseError`] if the JSON is malformed or does not match
/// the expected shape.
pub fn parse_alloc_tracker_operation(
    json: &str,
) -> Result<Option<BenchmarkResult>, AllocTrackerParseError> {
    let output: OperationOutput = serde_json::from_str(json).map_err(AllocTrackerParseError)?;
    Ok(output_to_record(&output))
}

/// Maps a parsed operation to a [`BenchmarkResult`], or `None` when it carries no
/// usable measurement.
///
/// `alloc_tracker`'s flat `target/alloc_tracker/` tree carries no package
/// attribution, so the operation name alone identifies the series (mirroring the
/// Criterion adapter).
///
/// For each metric the through-origin slope is the per-iteration point estimate,
/// matching the Criterion and `all_the_time` adapters. A zero-iteration operation
/// the workload could not run has no per-iteration rate — the producer writes its
/// slopes as null — so it yields `None` and is dropped rather than stored: a
/// non-finite metric value cannot round-trip through stored history (see
/// [`super::usable_slope`]). When the confidence interval is present it is recorded
/// on the metric, so analysis can apply its interval-overlap gate to allocation
/// figures the same way it does for wall time.
fn output_to_record(output: &OperationOutput) -> Option<BenchmarkResult> {
    let bytes_value = super::usable_slope(output.slope_bytes_per_iteration)?;
    let bytes = Metric::new(MetricKind::AllocatedBytes, bytes_value).with_dispersion(
        None,
        output.interval_low_bytes_per_iteration,
        output.interval_high_bytes_per_iteration,
    );

    let allocations_value = super::usable_slope(output.slope_allocations_per_iteration)?;
    let allocations = Metric::new(MetricKind::AllocationCount, allocations_value).with_dispersion(
        None,
        output.interval_low_allocations_per_iteration,
        output.interval_high_allocations_per_iteration,
    );

    let id = BenchmarkId::new(NonEmpty::new(output.operation.clone()));
    Some(BenchmarkResult::new(id, vec![bytes, allocations]))
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
        let record = parse_record(ALLOCATE_VEC_FIXTURE);
        assert_eq!(
            record.id,
            BenchmarkId::new(NonEmpty::new("allocate_vec".to_owned()))
        );
    }

    #[test]
    fn alloc_tracker_identity_is_the_operation_name_only() {
        let record = parse_record(ALLOCATE_VEC_FIXTURE);
        assert_eq!(
            record.id,
            BenchmarkId::new(NonEmpty::new("allocate_vec".to_owned()))
        );
    }

    #[test]
    fn maps_both_allocation_metrics_from_the_slopes() {
        let record = parse_record(ALLOCATE_VEC_FIXTURE);
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
        let record = parse_record(ALLOCATE_VEC_FIXTURE);
        for metric in &record.metrics {
            assert_eq!(metric.std_dev, None);
            assert_eq!(metric.interval_low, None);
            assert_eq!(metric.interval_high, None);
        }
    }

    #[test]
    fn records_dispersion_when_present() {
        let record = parse_record(ALLOCATE_VEC_DISPERSION_FIXTURE);

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
        let record = parse_record(json);
        assert_eq!(metric(&record, MetricKind::AllocatedBytes).value, 201.5);
        assert_eq!(metric(&record, MetricKind::AllocationCount).value, 3.25);
    }

    #[test]
    fn null_slope_yields_no_record() {
        // A zero-iteration operation the workload could not run leaves both slopes
        // null (serialized from NaN). It has no usable measurement, so the adapter
        // drops it rather than storing a non-finite value that could not round-trip
        // through stored history.
        let json = concat!(
            "{\"operation\":\"op\",",
            "\"slope_bytes_per_iteration\":null,",
            "\"slope_allocations_per_iteration\":null}"
        );
        assert!(parse_alloc_tracker_operation(json).unwrap().is_none());
    }

    #[test]
    fn a_single_missing_slope_yields_no_record() {
        // Even a partially-populated operation (one slope present, the other absent)
        // has no complete allocation profile to store, so the whole operation is
        // dropped rather than recorded with a fabricated figure.
        let json = "{\"operation\":\"op\",\"slope_bytes_per_iteration\":201.5}";
        assert!(parse_alloc_tracker_operation(json).unwrap().is_none());
    }

    #[test]
    fn preserves_the_original_operation_name() {
        let json = operation_json("group/case name", 4096, 7);
        let record = parse_record(&json);
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

    /// Parses a fixture that is expected to yield a stored record.
    fn parse_record(json: &str) -> BenchmarkResult {
        parse_alloc_tracker_operation(json)
            .expect("the fixture is well-formed alloc_tracker output")
            .expect("the operation has a usable measurement")
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
