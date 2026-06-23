//! Parsing of `all_the_time`'s per-operation JSON into the engine-neutral
//! [`BenchmarkResult`] model.
//!
//! `all_the_time` writes one file per operation under `target/all_the_time/`,
//! recording the per-iteration processor time a benchmark spends together with
//! bootstrap dispersion statistics over the operation's measured spans. Processor
//! time depends on the host hardware, so this engine partitions its history by a
//! machine key (see [`Engine::is_hardware_dependent`]). The committed
//! fixtures under `tests/fixtures/all_the_time/` are real `all_the_time` output
//! and act as a schema-drift canary.
//!
//! [`Engine::is_hardware_dependent`]: crate::comparability::Engine::is_hardware_dependent

use std::error::Error;
use std::fmt;

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
/// The through-origin slope is preferred as the per-iteration point estimate,
/// matching the Criterion adapter; output that records no slope falls back to the
/// mean. When the bootstrap confidence interval is present it is recorded on the
/// metric, so analysis can apply its interval-overlap gate to processor time the
/// same way it does for Criterion wall time.
fn output_to_record(output: &OperationOutput) -> BenchmarkResult {
    let id = BenchmarkId::new(vec![output.operation.clone()]);

    let value = output
        .slope_processor_time_nanos
        .unwrap_or_else(|| as_f64(output.mean_processor_time_nanos));

    let metric = Metric::new(MetricKind::ProcessorTime, value).with_dispersion(
        output.std_dev_processor_time_nanos,
        output.interval_low_processor_time_nanos,
        output.interval_high_processor_time_nanos,
    );

    BenchmarkResult::new(id, vec![metric])
}

/// Casts a nanosecond count to `f64`, the model's storage type.
#[expect(
    clippy::cast_precision_loss,
    reason = "per-iteration nanosecond means are well below 2^53; precision loss is irrelevant"
)]
fn as_f64(value: u64) -> f64 {
    value as f64
}

/// The subset of an `all_the_time` operation file the tool reads. The `total_*`
/// fields are ignored in favor of the per-iteration slope (or mean), which is
/// comparable across runs with differing iteration counts. The dispersion fields
/// are optional so that output recording only a mean still parses.
#[derive(Debug, Deserialize)]
struct OperationOutput {
    operation: String,
    mean_processor_time_nanos: u64,
    #[serde(default)]
    slope_processor_time_nanos: Option<f64>,
    #[serde(default)]
    std_dev_processor_time_nanos: Option<f64>,
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
        assert_eq!(record.id, BenchmarkId::new(vec!["read_cell".to_owned()]));
    }

    #[test]
    fn all_the_time_identity_is_the_operation_name_only() {
        let record = parse_all_the_time_operation(READ_CELL_FIXTURE).unwrap();
        assert_eq!(record.id.segments, vec!["read_cell".to_owned()]);
    }

    #[test]
    fn maps_processor_time_from_the_mean() {
        let record = parse_all_the_time_operation(READ_CELL_FIXTURE).unwrap();
        assert_eq!(record.metrics.len(), 1);

        let metric = &record.metrics[0];
        assert_eq!(metric.kind, MetricKind::ProcessorTime);
        assert_eq!(metric.value, 20_000_000.0);
    }

    #[test]
    fn output_without_dispersion_carries_no_interval() {
        // Output that records only a mean (no slope or interval) carries no
        // dispersion, so the point estimate falls back to the mean and analysis
        // relies on rank-testing across regimes rather than interval overlap.
        let record = parse_all_the_time_operation(READ_CELL_FIXTURE).unwrap();
        let metric = &record.metrics[0];
        assert_eq!(metric.std_dev, None);
        assert_eq!(metric.interval_low, None);
        assert_eq!(metric.interval_high, None);
    }

    #[test]
    fn records_dispersion_when_present() {
        use std::f64::consts::FRAC_1_SQRT_2;

        let record = parse_all_the_time_operation(READ_CELL_DISPERSION_FIXTURE).unwrap();
        let metric = &record.metrics[0];
        // The fixture's spans have a sample standard deviation of exactly
        // sqrt(0.5) = 1/sqrt(2) nanoseconds.
        assert_eq!(metric.std_dev, Some(FRAC_1_SQRT_2));
        assert_eq!(metric.interval_low, Some(19.5));
        assert_eq!(metric.interval_high, Some(20.5));
    }

    #[test]
    fn prefers_the_slope_over_the_mean() {
        // A slope distinct from the mean proves the slope is the chosen point
        // estimate, matching the Criterion adapter's preference.
        let json = concat!(
            "{\"operation\":\"op\",\"mean_processor_time_nanos\":20,",
            "\"slope_processor_time_nanos\":33.5,",
            "\"interval_low_processor_time_nanos\":30.0,",
            "\"interval_high_processor_time_nanos\":37.0}"
        );
        let record = parse_all_the_time_operation(json).unwrap();
        assert_eq!(record.metrics[0].value, 33.5);
    }

    #[test]
    fn preserves_the_original_operation_name() {
        let json = operation_json("group/case name", 1234);
        let record = parse_all_the_time_operation(&json).unwrap();
        assert_eq!(record.id.segments, vec!["group/case name".to_owned()]);
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

    fn operation_json(operation: &str, mean_nanos: u64) -> String {
        format!(
            "{{\"operation\":\"{operation}\",\"total_iterations\":4,\
             \"total_processor_time_nanos\":{},\"mean_processor_time_nanos\":{mean_nanos}}}",
            mean_nanos.saturating_mul(4),
        )
    }
}
