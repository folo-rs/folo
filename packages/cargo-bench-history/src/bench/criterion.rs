//! Parsing of Criterion's `new/benchmark.json` + `new/estimates.json` pair into
//! the engine-neutral [`ResultRecord`] model.
//!
//! Criterion measures wall-clock time per iteration (nanoseconds) and reports a
//! statistical estimate with a confidence interval and standard deviation, all of
//! which this adapter preserves so later analysis can be noise-aware. The
//! committed fixtures under `tests/fixtures/criterion/` are real Criterion output
//! and act as a schema-drift canary: if Criterion changes its format, parsing the
//! fixtures fails and the mismatch is caught immediately.

use std::error::Error;
use std::fmt;

use serde::Deserialize;

use crate::model::{BenchmarkId, Metric, MetricKind, ResultRecord};

/// The metric name recorded for a Criterion wall-clock measurement.
const WALL_TIME_METRIC: &str = "wall_time";

/// The unit Criterion reports its timings in (nanoseconds per iteration).
const TIME_UNIT: &str = "ns";

/// An error encountered while parsing a Criterion result case.
#[derive(Debug)]
pub(crate) enum CriterionParseError {
    /// The `benchmark.json` was not valid JSON or did not match the expected shape.
    Benchmark(serde_json::Error),
    /// The `estimates.json` was not valid JSON or did not match the expected shape.
    Estimates(serde_json::Error),
}

impl fmt::Display for CriterionParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Benchmark(error) => {
                write!(f, "failed to parse Criterion benchmark.json: {error}")
            }
            Self::Estimates(error) => {
                write!(f, "failed to parse Criterion estimates.json: {error}")
            }
        }
    }
}

impl Error for CriterionParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Benchmark(error) | Self::Estimates(error) => Some(error),
        }
    }
}

/// Parses one Criterion result case (its `benchmark.json` and `estimates.json`)
/// into a [`ResultRecord`].
///
/// # Errors
///
/// Returns [`CriterionParseError`] if either document is malformed.
pub(crate) fn parse_criterion_case(
    benchmark_json: &str,
    estimates_json: &str,
) -> Result<ResultRecord, CriterionParseError> {
    let benchmark: Benchmark =
        serde_json::from_str(benchmark_json).map_err(CriterionParseError::Benchmark)?;
    let estimates: Estimates =
        serde_json::from_str(estimates_json).map_err(CriterionParseError::Estimates)?;
    Ok(case_to_record(&benchmark, &estimates))
}

/// Maps a parsed Criterion case to a [`ResultRecord`] (pure).
///
/// The package is `None`: Criterion's files carry no package attribution and the
/// `target/criterion/` tree is flat, so a benchmark's owning crate cannot be
/// recovered. This workspace crate-prefixes its Criterion group ids, so the
/// `group_id` already disambiguates equally named benches across packages.
fn case_to_record(benchmark: &Benchmark, estimates: &Estimates) -> ResultRecord {
    let id = BenchmarkId::new(
        None,
        benchmark.group_id.clone(),
        Some(benchmark.function_id.clone()),
        non_empty(benchmark.value_str.as_deref()),
    );

    // Criterion fits a line through the per-sample timings when it uses linear
    // sampling (`Bencher::iter`), giving a slope estimate that is more robust to
    // per-iteration overhead than the raw mean. Prefer it when present; otherwise
    // fall back to the mean, which is always reported.
    let estimate = estimates.slope.as_ref().unwrap_or(&estimates.mean);
    let std_dev = estimates
        .std_dev
        .as_ref()
        .map(|estimate| estimate.point_estimate);

    let metric = Metric::new(
        WALL_TIME_METRIC.to_owned(),
        MetricKind::WallTime,
        estimate.point_estimate,
        Some(TIME_UNIT.to_owned()),
    )
    .with_dispersion(
        std_dev,
        Some(estimate.confidence_interval.lower_bound),
        Some(estimate.confidence_interval.upper_bound),
    );

    ResultRecord::new(id, vec![metric])
}

/// Returns the trimmed string as `Some` unless it is empty.
fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// The subset of Criterion's `benchmark.json` the tool reads.
#[derive(Debug, Deserialize)]
struct Benchmark {
    group_id: String,
    function_id: String,
    #[serde(default)]
    value_str: Option<String>,
}

/// The subset of Criterion's `estimates.json` the tool reads. `slope` is present
/// only for linearly sampled benchmarks; `std_dev` is always reported.
#[derive(Debug, Deserialize)]
struct Estimates {
    mean: Estimate,
    #[serde(default)]
    slope: Option<Estimate>,
    #[serde(default)]
    std_dev: Option<Estimate>,
}

#[derive(Debug, Deserialize)]
struct Estimate {
    confidence_interval: ConfidenceInterval,
    point_estimate: f64,
}

#[derive(Debug, Deserialize)]
struct ConfidenceInterval {
    lower_bound: f64,
    upper_bound: f64,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "fixture-derived estimates compare exactly"
    )]
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use super::*;

    const STD_INSTANT_BENCHMARK: &str =
        include_str!("../../tests/fixtures/criterion/std_instant/benchmark.json");
    const STD_INSTANT_ESTIMATES: &str =
        include_str!("../../tests/fixtures/criterion/std_instant/estimates.json");
    const FAST_TIME_BENCHMARK: &str =
        include_str!("../../tests/fixtures/criterion/fast_time_clock/benchmark.json");
    const FAST_TIME_ESTIMATES: &str =
        include_str!("../../tests/fixtures/criterion/fast_time_clock/estimates.json");

    #[test]
    fn parses_identity_from_benchmark_json() {
        let record = parse_criterion_case(STD_INSTANT_BENCHMARK, STD_INSTANT_ESTIMATES).unwrap();
        assert_eq!(
            record.id,
            BenchmarkId::new(
                None,
                "fast_time_timestamp_performance/timestamp_capture".to_owned(),
                Some("std_instant".to_owned()),
                Some("now".to_owned()),
            )
        );
    }

    #[test]
    fn criterion_record_has_no_package() {
        let record = parse_criterion_case(STD_INSTANT_BENCHMARK, STD_INSTANT_ESTIMATES).unwrap();
        assert_eq!(record.id.package, None);
    }

    #[test]
    fn maps_wall_time_with_slope_and_dispersion() {
        let record = parse_criterion_case(STD_INSTANT_BENCHMARK, STD_INSTANT_ESTIMATES).unwrap();
        assert_eq!(record.metrics.len(), 1);

        let metric = &record.metrics[0];
        assert_eq!(metric.name, WALL_TIME_METRIC);
        assert_eq!(metric.kind, MetricKind::WallTime);
        assert_eq!(metric.unit.as_deref(), Some("ns"));
        // Slope is present, so its point estimate and confidence interval win.
        assert_eq!(metric.value, 26.929_980_671_639_95);
        assert_eq!(metric.interval_low, Some(26.672_074_791_998_387));
        assert_eq!(metric.interval_high, Some(27.182_943_616_630_872));
        assert_eq!(metric.std_dev, Some(0.474_382_154_340_039_8));
    }

    #[test]
    fn parses_second_fixture_case() {
        let record = parse_criterion_case(FAST_TIME_BENCHMARK, FAST_TIME_ESTIMATES).unwrap();
        assert_eq!(record.id.case.as_deref(), Some("fast_time_clock"));
        assert_eq!(record.metrics[0].value, 1.635_849_872_126_785);
    }

    fn benchmark_json(group: &str, function: &str, value: &str) -> String {
        format!(
            "{{\"group_id\":\"{group}\",\"function_id\":\"{function}\",\"value_str\":\"{value}\"}}"
        )
    }

    fn estimate_json(point: f64, low: f64, high: f64) -> String {
        format!(
            "{{\"confidence_interval\":{{\"confidence_level\":0.95,\
             \"lower_bound\":{low},\"upper_bound\":{high}}},\
             \"point_estimate\":{point},\"standard_error\":0.1}}"
        )
    }

    #[test]
    fn falls_back_to_mean_when_slope_is_absent() {
        // A flat-sampled benchmark (`iter_custom`) reports no slope, so the mean
        // estimate and its interval are used.
        let estimates = format!(
            "{{\"mean\":{},\"median\":{},\"median_abs_dev\":{},\"std_dev\":{}}}",
            estimate_json(50.0, 48.0, 52.0),
            estimate_json(49.0, 47.0, 51.0),
            estimate_json(1.0, 0.5, 1.5),
            estimate_json(2.0, 1.0, 3.0),
        );
        let record =
            parse_criterion_case(&benchmark_json("grp", "fun", "val"), &estimates).unwrap();

        let metric = &record.metrics[0];
        assert_eq!(metric.value, 50.0);
        assert_eq!(metric.interval_low, Some(48.0));
        assert_eq!(metric.interval_high, Some(52.0));
        assert_eq!(metric.std_dev, Some(2.0));
    }

    #[test]
    fn empty_value_str_becomes_no_value_component() {
        let estimates = format!(
            "{{\"mean\":{},\"median\":{},\"median_abs_dev\":{}}}",
            estimate_json(10.0, 9.0, 11.0),
            estimate_json(10.0, 9.0, 11.0),
            estimate_json(1.0, 0.5, 1.5),
        );
        let record = parse_criterion_case(&benchmark_json("grp", "fun", ""), &estimates).unwrap();
        assert_eq!(record.id.value, None);
        // With no slope and no std_dev, only the mean drives the metric.
        assert_eq!(record.metrics[0].std_dev, None);
    }

    #[test]
    fn rejects_malformed_benchmark_json() {
        let error = parse_criterion_case("{ not json", STD_INSTANT_ESTIMATES).unwrap_err();
        assert!(matches!(error, CriterionParseError::Benchmark(_)));
    }

    #[test]
    fn rejects_malformed_estimates_json() {
        let error = parse_criterion_case(STD_INSTANT_BENCHMARK, "{ not json").unwrap_err();
        assert!(matches!(error, CriterionParseError::Estimates(_)));
    }

    #[test]
    fn error_display_and_source() {
        let benchmark_error =
            parse_criterion_case("{ not json", STD_INSTANT_ESTIMATES).unwrap_err();
        assert!(
            benchmark_error
                .to_string()
                .contains("failed to parse Criterion benchmark.json"),
            "{benchmark_error}"
        );
        assert!(benchmark_error.source().is_some());

        let estimates_error =
            parse_criterion_case(STD_INSTANT_BENCHMARK, "{ not json").unwrap_err();
        assert!(
            estimates_error
                .to_string()
                .contains("failed to parse Criterion estimates.json"),
            "{estimates_error}"
        );
        assert!(estimates_error.source().is_some());
    }
}
