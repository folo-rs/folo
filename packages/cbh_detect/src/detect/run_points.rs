//! A stored run reduced to exactly the data the series fold reads.
//!
//! The parallel object loader fans the per-object parse + fold across cores: each
//! worker parses one object at a time into [`RunPoints`] and folds it straight into
//! its own `SeriesBuilder`, dropping the parsed run before the next. Deserializing
//! into [`RunPoints`] instead of the full [`Run`](cbh_model::Run) shrinks that
//! transient per-object footprint: it keeps only, per result, the benchmark id and
//! the metric fields that
//! [`SeriesBuilder::push`](crate::detect::SeriesBuilder::push) actually folds
//! into points — dropping the run context (environment, toolchain, commit,
//! timestamps) and each metric's standard deviation. The commit a point is
//! labelled with comes from the storage key (its commit directory segment), not
//! the run payload, so the projection carries no git fields at all. Serde ignores
//! the unmentioned JSON fields, so a run still parses unchanged; only the discarded
//! parts are never
//! materialized. Metric kinds the tool no longer tracks (the build-layout-volatile
//! Callgrind events dropped in the metric cull) are skipped rather than failing the
//! parse, matching [`cbh_model`'s lenient run read](cbh_model::MetricList) — the
//! same stored files feed both paths, so a single legacy object must not abort the
//! whole history analysis. (The leaner element trims overall peak only marginally —
//! peak is
//! set by the per-worker builders coexisting during the merge — but the lighter
//! parse is still worth keeping; see the load section of `cargo-bench-history`'s
//! `docs/analyze.md`.)

use cbh_model::{BenchmarkId, MetricKind, Run};
use serde::Deserialize;
use smallvec::SmallVec;

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
    /// [`MetricList`](cbh_model::MetricList).
    #[serde(deserialize_with = "deserialize_metric_points")]
    pub metrics: SmallVec<[MetricPoint; 2]>,
}

/// Deserializes the metric-point list, silently dropping any metric whose `kind`
/// is not one of the kinds the tool tracks.
///
/// This mirrors [`cbh_model`'s lenient run read][run]: stored history predates the
/// removal of the build-layout-volatile Callgrind metrics (cache hits per tier,
/// estimated cycles, branch misses), so run files written by an older tool can
/// still carry those kinds. The lean analyze/examine projection parses those same
/// files, so it must drop the unknown kinds too — failing the whole run parse on an
/// unknown-variant error would abort the entire history analysis over any such data.
///
/// The raw list is kept inline in a `SmallVec` matching the projection's capacity so
/// the common one- or two-metric case stays allocation-free on this hot read path.
///
/// [run]: cbh_model::MetricList
fn deserialize_metric_points<'de, D>(
    deserializer: D,
) -> Result<SmallVec<[MetricPoint; 2]>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = SmallVec::<[RawMetricPoint<'de>; 2]>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter_map(RawMetricPoint::into_point)
        .collect())
}

/// A metric point as it appears on disk, with the kind left as its raw wire name so
/// an unknown kind can be recognized and dropped rather than aborting the parse.
///
/// The kind is borrowed straight from the input JSON (a `&str` tied to the
/// deserializer input): it is only needed transiently to resolve a [`MetricKind`]
/// before this raw form is discarded, so borrowing avoids a `String` allocation per
/// metric on the read path. The stored kind names are a fixed vocabulary of unescaped
/// identifiers, so the borrow always succeeds against the [`from_json`] input backing
/// every run decode. The persisted `std_dev` the fold never reads is left unmentioned
/// and thus ignored.
///
/// [`from_json`]: RunPoints::from_json
#[derive(Deserialize)]
struct RawMetricPoint<'a> {
    kind: &'a str,
    value: f64,
    #[serde(default)]
    interval_low: Option<f64>,
    #[serde(default)]
    interval_high: Option<f64>,
}

impl RawMetricPoint<'_> {
    /// Converts to a [`MetricPoint`], or `None` when the kind is not one of the
    /// kinds the tool tracks.
    fn into_point(self) -> Option<MetricPoint> {
        Some(MetricPoint {
            kind: MetricKind::from_name(self.kind)?,
            value: self.value,
            interval_low: self.interval_low,
            interval_high: self.interval_high,
        })
    }
}

/// A metric reduced to the fields the series fold reads.
///
/// Drops the standard deviation the fold never consults; the point estimate and
/// confidence-interval bounds are all it carries into a [`SeriesPoint`].
///
/// [`SeriesPoint`]: crate::detect::SeriesPoint
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

    use cbh_model::{BenchmarkResult, EnvironmentInfo, GitInfo, Metric, RunContext, ToolchainInfo};
    use nonempty::nonempty;
    use smallvec::smallvec;

    use super::*;

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
    fn from_json_drops_unknown_metric_kinds() {
        // Legacy history can still carry metric kinds the tool no longer tracks (the
        // build-layout-volatile Callgrind events removed in the metric cull). The
        // lean analyze/examine projection must skip them rather than fail the whole
        // run parse, matching the full-`Run` read path — otherwise a single old
        // object aborts the entire history analysis.
        let mut value: serde_json::Value =
            serde_json::from_str(&sample_run().to_json().unwrap()).unwrap();
        value["results"][0]["metrics"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "kind": "conditional_branch_misses", "value": 5.0 }));

        let json = serde_json::to_string(&value).unwrap();
        let points = RunPoints::from_json(&json).unwrap();
        let kinds: Vec<MetricKind> = points.results()[0]
            .metrics
            .iter()
            .map(|metric| metric.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![MetricKind::WallTime, MetricKind::InstructionCount]
        );
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
                Metric::new(MetricKind::ConditionalBranches, 3.0),
                Metric::new(MetricKind::IndirectBranches, 4.0),
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
        assert_eq!(
            from_run.results()[0].metrics[3].kind,
            MetricKind::IndirectBranches
        );
        assert_eq!(from_run.results()[0].metrics[3].value, 4.0);
        assert_eq!(from_run.results()[1].metrics.len(), 1);

        assert_eq!(
            RunPoints::from_json(&run.to_json().unwrap()).unwrap(),
            from_run
        );
    }
}
