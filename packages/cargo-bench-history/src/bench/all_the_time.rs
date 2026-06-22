//! Parsing of `all_the_time`'s per-operation JSON into the engine-neutral
//! [`ResultRecord`] model.
//!
//! `all_the_time` writes one file per operation under `target/all_the_time/`,
//! recording the mean processor time a benchmark spends per iteration. Processor
//! time depends on the host hardware, so this engine partitions its history by a
//! machine key (see [`EngineSystem::is_hardware_dependent`]). The committed
//! fixtures under `tests/fixtures/all_the_time/` are real `all_the_time` output
//! and act as a schema-drift canary.
//!
//! [`EngineSystem::is_hardware_dependent`]: crate::comparability::EngineSystem::is_hardware_dependent

use std::error::Error;
use std::fmt;

use serde::Deserialize;

use crate::model::{BenchmarkId, Metric, MetricKind, ResultRecord};

/// The metric name recorded for the per-iteration processor-time measurement.
const PROCESSOR_TIME_METRIC: &str = "processor_time";

/// The unit `all_the_time` reports its timings in (nanoseconds per iteration).
const TIME_UNIT: &str = "ns";

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

/// Parses one `all_the_time` operation file into a [`ResultRecord`].
///
/// # Errors
///
/// Returns [`AllTheTimeParseError`] if the JSON is malformed or does not match
/// the expected shape.
pub(crate) fn parse_all_the_time_operation(
    json: &str,
) -> Result<ResultRecord, AllTheTimeParseError> {
    let output: OperationOutput = serde_json::from_str(json).map_err(AllTheTimeParseError)?;
    Ok(output_to_record(&output))
}

/// Maps a parsed operation to a [`ResultRecord`] (pure).
///
/// The package is `None`: `all_the_time`'s flat `target/all_the_time/` tree
/// carries no package attribution, so the operation name alone identifies the
/// series (mirroring the Criterion adapter).
///
/// Only the mean per-iteration value is recorded; `all_the_time` reports no
/// dispersion, so the metric carries no confidence interval. The processor-time
/// metric is still treated as noisy by analysis, which then relies on
/// rank-testing across regimes rather than per-sample interval overlap.
fn output_to_record(output: &OperationOutput) -> ResultRecord {
    let id = BenchmarkId::new(None, output.operation.clone(), None, None);

    let metric = Metric::new(
        PROCESSOR_TIME_METRIC.to_owned(),
        MetricKind::ProcessorTime,
        as_f64(output.mean_processor_time_nanos),
        Some(TIME_UNIT.to_owned()),
    );

    ResultRecord::new(id, vec![metric])
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
/// fields are ignored in favor of the per-iteration mean, which is comparable
/// across runs with differing iteration counts.
#[derive(Debug, Deserialize)]
struct OperationOutput {
    operation: String,
    mean_processor_time_nanos: u64,
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

    #[test]
    fn parses_identity_from_operation_name() {
        let record = parse_all_the_time_operation(READ_CELL_FIXTURE).unwrap();
        assert_eq!(
            record.id,
            BenchmarkId::new(None, "read_cell".to_owned(), None, None)
        );
    }

    #[test]
    fn all_the_time_record_has_no_package() {
        let record = parse_all_the_time_operation(READ_CELL_FIXTURE).unwrap();
        assert_eq!(record.id.package, None);
    }

    #[test]
    fn maps_processor_time_from_the_mean() {
        let record = parse_all_the_time_operation(READ_CELL_FIXTURE).unwrap();
        assert_eq!(record.metrics.len(), 1);

        let metric = &record.metrics[0];
        assert_eq!(metric.name, PROCESSOR_TIME_METRIC);
        assert_eq!(metric.kind, MetricKind::ProcessorTime);
        assert_eq!(metric.unit.as_deref(), Some("ns"));
        assert_eq!(metric.value, 20_000_000.0);
    }

    #[test]
    fn processor_time_carries_no_dispersion() {
        // `all_the_time` reports only a mean, so no interval or standard deviation
        // is available; analysis falls back to rank-testing across regimes.
        let record = parse_all_the_time_operation(READ_CELL_FIXTURE).unwrap();
        let metric = &record.metrics[0];
        assert_eq!(metric.std_dev, None);
        assert_eq!(metric.interval_low, None);
        assert_eq!(metric.interval_high, None);
    }

    #[test]
    fn preserves_the_original_operation_name() {
        let json = operation_json("group/case name", 1234);
        let record = parse_all_the_time_operation(&json).unwrap();
        assert_eq!(record.id.group, "group/case name");
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
