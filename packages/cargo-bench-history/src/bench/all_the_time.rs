//! Parsing of `all_the_time`'s per-operation JSON into the engine-neutral
//! [`BenchmarkResult`] model.
//!
//! `all_the_time` writes one file per operation under `target/all_the_time/`,
//! recording the per-iteration processor time a benchmark spends together with a
//! per-iteration slope and its confidence interval estimated over the operation's
//! measured spans. Processor time depends on the host hardware, so this engine
//! partitions its history by a machine key (see [`Engine::is_hardware_dependent`]).
//! The committed fixtures under `tests/fixtures/all_the_time/` are representative
//! samples of the current schema; the authoritative schema-drift guard is the
//! `super::schema_roundtrip` test, which feeds real producer output through this
//! parser so a field renamed or dropped on either side of the boundary fails the
//! build.
//!
//! [`Engine::is_hardware_dependent`]: crate::model::Engine::is_hardware_dependent

use std::error::Error;
use std::fmt;

use nonempty::NonEmpty;
use serde::Deserialize;

use crate::model::{BenchmarkId, BenchmarkResult, Metric, MetricKind};

/// An error encountered while parsing an `all_the_time` operation file.
#[derive(Debug)]
pub(crate) struct AllTheTimeParseError(serde_json::Error);

impl fmt::Display for AllTheTimeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "failed to parse all_the_time output: {}", self.0)
    }
}

impl Error for AllTheTimeParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

/// Parses one `all_the_time` operation file into a [`BenchmarkResult`].
///
/// # Errors
///
/// Returns [`AllTheTimeParseError`] if the JSON is malformed or does not match
/// the expected shape.
pub(crate) fn parse_all_the_time_operation(
    json: &str,
) -> Result<BenchmarkResult, AllTheTimeParseError> {
    let output: OperationOutput = serde_json::from_str(json).map_err(AllTheTimeParseError)?;
    Ok(output_to_record(&output))
}

/// Maps a parsed operation to a [`BenchmarkResult`] (pure).
///
/// `all_the_time`'s flat `target/all_the_time/` tree carries no package
/// attribution, so the operation name alone identifies the series (mirroring the
/// Criterion adapter).
///
/// The through-origin slope is the per-iteration point estimate, matching the
/// Criterion adapter. A missing or null slope (which the producer writes for a
/// zero-iteration operation that could not run) maps to `NaN`, signalling that no
/// valid per-iteration figure was produced. When the confidence interval is present
/// it is recorded on the metric, so analysis can apply its interval-overlap gate to
/// processor time the same way it does for Criterion wall time.
fn output_to_record(output: &OperationOutput) -> BenchmarkResult {
    let id = BenchmarkId::new(NonEmpty::new(output.operation.clone()));

    let value = output.slope_processor_time_nanos.unwrap_or(f64::NAN);

    let metric = Metric::new(MetricKind::ProcessorTime, value).with_dispersion(
        None,
        output.interval_low_processor_time_nanos,
        output.interval_high_processor_time_nanos,
    );

    BenchmarkResult::new(id, vec![metric])
}

/// The subset of an `all_the_time` operation file the tool reads. The `total_*`
/// fields are ignored in favor of the per-iteration slope, which is comparable
/// across runs with differing iteration counts. The slope and dispersion fields are
/// optional so that a zero-iteration operation (null slope) and single-span output
/// (slope but no interval) both parse.
#[derive(Debug, Deserialize)]
struct OperationOutput {
    operation: String,
    #[serde(default)]
    slope_processor_time_nanos: Option<f64>,
    #[serde(default)]
    interval_low_processor_time_nanos: Option<f64>,
    #[serde(default)]
    interval_high_processor_time_nanos: Option<f64>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "processor-time means are exact integer-derived nanosecond counts"
    )]
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use super::*;

    const READ_CELL_FIXTURE: &str =
        include_str!("../../tests/fixtures/all_the_time/read_cell.json");

    const READ_CELL_DISPERSION_FIXTURE: &str =
        include_str!("../../tests/fixtures/all_the_time/read_cell_dispersion.json");

    #[test]
    fn parses_identity_from_operation_name() {
        let record = parse_all_the_time_operation(READ_CELL_FIXTURE).unwrap();
        assert_eq!(
            record.id,
            BenchmarkId::new(NonEmpty::new("read_cell".to_owned()))
        );
    }

    #[test]
    fn all_the_time_identity_is_the_operation_name_only() {
        let record = parse_all_the_time_operation(READ_CELL_FIXTURE).unwrap();
        assert_eq!(
            record.id,
            BenchmarkId::new(NonEmpty::new("read_cell".to_owned()))
        );
    }

    #[test]
    fn maps_processor_time_from_the_slope() {
        let record = parse_all_the_time_operation(READ_CELL_FIXTURE).unwrap();
        assert_eq!(record.metrics.len(), 1);

        let metric = &record.metrics[0];
        assert_eq!(metric.kind, MetricKind::ProcessorTime);
        assert_eq!(metric.value, 20_000_000.0);
    }

    #[test]
    fn single_span_output_carries_no_interval() {
        // Single-span output records a slope but no interval, so the metric carries
        // no dispersion and analysis relies on rank-testing across regimes rather
        // than interval overlap.
        let record = parse_all_the_time_operation(READ_CELL_FIXTURE).unwrap();
        let metric = &record.metrics[0];
        assert_eq!(metric.std_dev, None);
        assert_eq!(metric.interval_low, None);
        assert_eq!(metric.interval_high, None);
    }

    #[test]
    fn records_dispersion_when_present() {
        let record = parse_all_the_time_operation(READ_CELL_DISPERSION_FIXTURE).unwrap();
        let metric = &record.metrics[0];
        // The slope is the point estimate and the spans' jitter is carried as an
        // analytic confidence interval straddling it. The adapter no longer surfaces
        // a standard deviation.
        assert_eq!(metric.value, 20.0);
        assert_eq!(metric.std_dev, None);
        assert_eq!(metric.interval_low, Some(19.346_678_671_819_987));
        assert_eq!(metric.interval_high, Some(20.653_321_328_180_013));
    }

    #[test]
    fn reads_the_slope_as_the_point_estimate() {
        // The slope is the chosen point estimate, matching the Criterion adapter.
        let json = concat!(
            "{\"operation\":\"op\",",
            "\"slope_processor_time_nanos\":33.5,",
            "\"interval_low_processor_time_nanos\":30.0,",
            "\"interval_high_processor_time_nanos\":37.0}"
        );
        let record = parse_all_the_time_operation(json).unwrap();
        assert_eq!(record.metrics[0].value, 33.5);
    }

    #[test]
    fn missing_slope_maps_to_nan() {
        // A zero-iteration operation the workload could not run leaves the slope
        // null (serialized from NaN), which the adapter surfaces as NaN rather than
        // inventing a figure.
        let json = "{\"operation\":\"op\",\"slope_processor_time_nanos\":null}";
        let record = parse_all_the_time_operation(json).unwrap();
        assert!(record.metrics[0].value.is_nan());
    }

    #[test]
    fn preserves_the_original_operation_name() {
        let json = operation_json("group/case name", 1234);
        let record = parse_all_the_time_operation(&json).unwrap();
        assert_eq!(
            record.id,
            BenchmarkId::new(NonEmpty::new("group/case name".to_owned()))
        );
    }

    #[test]
    fn rejects_malformed_json() {
        let error = parse_all_the_time_operation("{ not json").unwrap_err();
        assert!(
            error.to_string().contains("failed to parse all_the_time"),
            "{error}"
        );
        assert!(error.source().is_some());
    }

    fn operation_json(operation: &str, nanos: u64) -> String {
        format!(
            "{{\"operation\":\"{operation}\",\"total_iterations\":4,\
             \"total_processor_time_nanos\":{},\"span_count\":1,\
             \"slope_processor_time_nanos\":{nanos}}}",
            nanos.saturating_mul(4),
        )
    }
}
