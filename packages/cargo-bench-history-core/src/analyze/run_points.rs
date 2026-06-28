//! A stored run reduced to exactly the data the series fold reads.
//!
//! The parallel object loader fans the per-object parse + fold across cores: each
//! worker parses one object at a time into [`RunPoints`] and folds it straight into
//! its own `SeriesBuilder`, dropping the parsed run before the next. Deserializing
//! into [`RunPoints`] instead of the full [`Run`](crate::model::Run) shrinks that
//! transient per-object footprint: it keeps only, per result, the benchmark id and
//! the metric fields that
//! [`SeriesBuilder::push`](crate::analyze::SeriesBuilder::push) actually folds
//! into points — dropping the run context (environment, toolchain, commit,
//! timestamps) and each metric's standard deviation. The commit a point is
//! labelled with comes from the storage key (its commit directory segment), not
//! the run payload, so the projection carries no git fields at all. Serde ignores
//! the unmentioned JSON fields, so a run still parses unchanged; only the discarded
//! parts are never
//! materialized. (The leaner element trims overall peak only marginally — peak is
//! set by the per-worker builders coexisting during the merge — but the lighter
//! parse is still worth keeping; see `cargo-bench-history`'s `docs/DESIGN.md`
//! decision 36.)

use serde::Deserialize;
use smallvec::SmallVec;

use crate::model::{BenchmarkId, MetricKind, Run};

/// A stored run reduced to the data the series fold reads.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RunPoints {
    /// One entry per benchmark result in the run.
    results: Vec<ResultPoints>,
}

impl RunPoints {
    /// Deserializes the fold-relevant projection of a run from its JSON form.
    ///
    /// # Errors
    ///
    /// Returns an error if `json` is not a valid serialized run.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// The benchmark results carried by the run, one entry per measured case.
    #[must_use]
    pub fn results(&self) -> &[ResultPoints] {
        &self.results
    }
}

impl From<&Run> for RunPoints {
    fn from(run: &Run) -> Self {
        Self {
            results: run
                .results
                .iter()
                .map(|result| ResultPoints {
                    id: result.id.clone(),
                    metrics: result
                        .metrics
                        .iter()
                        .map(|metric| MetricPoint {
                            kind: metric.kind,
                            value: metric.value,
                            interval_low: metric.interval_low,
                            interval_high: metric.interval_high,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

/// A benchmark result reduced to its identity and fold-relevant metrics.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ResultPoints {
    /// Stable identity of the benchmark case (its series key).
    pub id: BenchmarkId,
    /// The fold-relevant projection of each captured metric. Inline up to two, as
    /// the noisy single-metric engines are the common case, matching
    /// [`MetricList`](crate::model::MetricList).
    pub metrics: SmallVec<[MetricPoint; 2]>,
}

/// A metric reduced to the fields the series fold reads.
///
/// Drops the standard deviation the fold never consults; the point estimate and
/// confidence-interval bounds are all it carries into a [`SeriesPoint`].
///
/// [`SeriesPoint`]: crate::analyze::SeriesPoint
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct MetricPoint {
    /// What kind of quantity this is (governs unit and comparison semantics).
    pub kind: MetricKind,
    /// The per-iteration point estimate, in the unit implied by `kind`.
    pub value: f64,
    /// Lower bound of the value's confidence interval, when reported.
    pub interval_low: Option<f64>,
    /// Upper bound of the value's confidence interval, when reported.
    pub interval_high: Option<f64>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "metric values round-trip exactly through serde_json"
    )]
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use nonempty::nonempty;
    use smallvec::smallvec;

    use super::*;
    use crate::model::{
        BenchmarkResult, EnvironmentInfo, GitInfo, Metric, RunContext, ToolchainInfo,
    };

    fn sample_run() -> Run {
        let context = RunContext::new(
            "2024-01-01T00:00:00Z".parse().unwrap(),
            GitInfo {
                commit: Some("0123456789abcdef".to_owned()),
                branch: Some("main".to_owned()),
                dirty: false,
            },
            EnvironmentInfo::default(),
            ToolchainInfo {
                target_triple: "x86_64-unknown-linux-gnu".to_owned(),
                rustc_version: Some("1.80.0".to_owned()),
            },
            "9.9.9".to_owned(),
        );
        let result = BenchmarkResult::new(
            BenchmarkId::new(nonempty![
                "pkg".to_owned(),
                "group".to_owned(),
                "case".to_owned()
            ]),
            smallvec![
                Metric::new(MetricKind::WallTime, 26.9).with_dispersion(
                    Some(0.47),
                    Some(26.6),
                    Some(27.2),
                ),
                Metric::new(MetricKind::InstructionCount, 1234.0),
            ],
        );
        Run::new(context, vec![result])
    }

    #[test]
    fn from_json_matches_from_run() {
        // Parsing the lean projection straight from JSON must agree with converting
        // a fully parsed `Run`, guarding against drift between the JSON field names
        // and the lean structs.
        let run = sample_run();
        let json = run.to_json().unwrap();

        let from_json = RunPoints::from_json(&json).unwrap();
        let from_run = RunPoints::from(&run);

        assert_eq!(from_json, from_run);
    }

    #[test]
    fn projection_keeps_only_fold_relevant_fields() {
        let run = sample_run();
        let points = RunPoints::from(&run);

        assert_eq!(points.results().len(), 1);
        let result = &points.results()[0];
        assert_eq!(result.id, run.results[0].id);
        assert_eq!(result.metrics.len(), 2);
        assert_eq!(result.metrics[0].kind, MetricKind::WallTime);
        assert_eq!(result.metrics[0].value, 26.9);
        assert_eq!(result.metrics[0].interval_low, Some(26.6));
        assert_eq!(result.metrics[0].interval_high, Some(27.2));
    }

    #[test]
    fn projection_keeps_metric_without_interval_bounds() {
        // The second metric of `sample_run` (instruction count) carries no
        // dispersion, so its confidence-interval bounds must project as absent
        // rather than defaulting to a fabricated value.
        let points = RunPoints::from(&sample_run());
        let metric = points.results()[0].metrics[1];

        assert_eq!(metric.kind, MetricKind::InstructionCount);
        assert_eq!(metric.value, 1234.0);
        assert_eq!(metric.interval_low, None);
        assert_eq!(metric.interval_high, None);
    }

    #[test]
    fn from_json_rejects_invalid_json() {
        // A payload that is not a serialized run must surface a parse error rather
        // than silently yielding an empty or partial projection.
        RunPoints::from_json("not valid json").unwrap_err();
    }

    #[test]
    fn projection_handles_a_run_with_no_results() {
        // An empty run (no benchmark results) projects to an empty point set and
        // still round-trips through JSON, so the fold simply contributes nothing.
        let context = RunContext::new(
            "2024-01-01T00:00:00Z".parse().unwrap(),
            GitInfo {
                commit: Some("0123456789abcdef".to_owned()),
                branch: Some("main".to_owned()),
                dirty: false,
            },
            EnvironmentInfo::default(),
            ToolchainInfo::default(),
            "9.9.9".to_owned(),
        );
        let run = Run::new(context, Vec::new());

        let from_run = RunPoints::from(&run);
        assert!(from_run.results().is_empty());
        assert_eq!(
            RunPoints::from_json(&run.to_json().unwrap()).unwrap(),
            from_run
        );
    }

    #[test]
    fn projection_preserves_multiple_results_and_spilled_metrics() {
        // A run with several results, one of which carries more metrics than the
        // inline SmallVec capacity, must project every result and every metric
        // (the spilled tail included) and agree with the from-JSON parse.
        let context = RunContext::new(
            "2024-01-01T00:00:00Z".parse().unwrap(),
            GitInfo {
                commit: Some("0123456789abcdef".to_owned()),
                branch: Some("main".to_owned()),
                dirty: false,
            },
            EnvironmentInfo::default(),
            ToolchainInfo::default(),
            "9.9.9".to_owned(),
        );
        let many_metrics = BenchmarkResult::new(
            BenchmarkId::new(nonempty!["pkg".to_owned(), "wide".to_owned()]),
            smallvec![
                Metric::new(MetricKind::WallTime, 1.0).with_dispersion(
                    Some(0.1),
                    Some(0.9),
                    Some(1.1),
                ),
                Metric::new(MetricKind::InstructionCount, 2.0),
                Metric::new(MetricKind::EstimatedCycles, 3.0),
                Metric::new(MetricKind::RamHits, 4.0),
            ],
        );
        let single_metric = BenchmarkResult::new(
            BenchmarkId::new(nonempty!["pkg".to_owned(), "narrow".to_owned()]),
            smallvec![Metric::new(MetricKind::WallTime, 5.0)],
        );
        let run = Run::new(context, vec![many_metrics, single_metric]);

        let from_run = RunPoints::from(&run);
        assert_eq!(from_run.results().len(), 2);
        assert_eq!(
            from_run.results()[0].metrics.len(),
            4,
            "the metric list spilled past inline capacity must survive projection"
        );
        assert_eq!(from_run.results()[0].metrics[3].kind, MetricKind::RamHits);
        assert_eq!(from_run.results()[0].metrics[3].value, 4.0);
        assert_eq!(from_run.results()[1].metrics.len(), 1);

        assert_eq!(
            RunPoints::from_json(&run.to_json().unwrap()).unwrap(),
            from_run
        );
    }
}
