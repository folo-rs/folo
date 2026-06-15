//! The finding algorithms: compare the latest point of each series against a
//! rolling baseline of its recent history and flag statistically notable moves.
//!
//! Most metrics this tool records are "lower is better" (instruction counts, cycle
//! estimates, branch counts, wall time), so a latest value above the baseline is a
//! [`Direction::Regression`] and one below it is a [`Direction::Improvement`].
//! Cache *hit* counts are the exception — more hits means fewer cache misses, so
//! for that metric the polarity is inverted.

use serde::Serialize;

use crate::analyze::series::{Location, Series};
use crate::model::{BenchmarkId, MetricKind};

/// Number of recent points (immediately before the latest) the baseline spans.
pub(crate) const DEFAULT_WINDOW: usize = 5;
/// Minimum relative move (1%) that registers even when the history is perfectly
/// stable (zero dispersion).
pub(crate) const DEFAULT_RELATIVE_THRESHOLD: f64 = 0.01;
/// Multiplier applied to the baseline's median absolute deviation to derive the
/// noise-aware portion of the threshold.
pub(crate) const DEFAULT_MAD_MULTIPLIER: f64 = 3.0;

/// Relative move at or above which a change is classified [`Severity::Major`].
const MAJOR_RELATIVE_DELTA: f64 = 0.10;
/// Relative move at or above which a change is classified [`Severity::Moderate`].
const MODERATE_RELATIVE_DELTA: f64 = 0.03;

/// Tunable parameters of the rolling-baseline regression detector.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RegressionConfig {
    /// How many points immediately before the latest form the baseline.
    pub(crate) window: usize,
    /// Minimum relative move that registers regardless of dispersion.
    pub(crate) relative_threshold: f64,
    /// Multiplier on the baseline's median absolute deviation.
    pub(crate) mad_multiplier: f64,
}

impl Default for RegressionConfig {
    fn default() -> Self {
        Self {
            window: DEFAULT_WINDOW,
            relative_threshold: DEFAULT_RELATIVE_THRESHOLD,
            mad_multiplier: DEFAULT_MAD_MULTIPLIER,
        }
    }
}

/// The direction of a flagged change relative to the baseline.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Direction {
    /// The latest value is worse (higher) than the baseline.
    Regression,
    /// The latest value is better (lower) than the baseline.
    Improvement,
}

/// How large a flagged change is, in ascending order of importance.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Severity {
    /// A small but threshold-crossing change.
    Minor,
    /// A medium-sized change.
    Moderate,
    /// A large change.
    Major,
}

/// One flagged change: where it is, what moved, and by how much.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Finding {
    /// The comparable location the series belongs to.
    #[serde(flatten)]
    pub(crate) location: Location,
    /// The benchmark identity.
    #[serde(flatten)]
    pub(crate) id: BenchmarkId,
    /// The metric that moved.
    pub(crate) metric: String,
    /// The category of the metric that moved.
    pub(crate) kind: MetricKind,
    /// Whether the move is a regression or an improvement.
    pub(crate) direction: Direction,
    /// How large the move is.
    pub(crate) severity: Severity,
    /// The rolling baseline value the latest point was compared against.
    pub(crate) baseline: f64,
    /// The latest measured value.
    pub(crate) latest: f64,
    /// The absolute change (`latest - baseline`).
    pub(crate) delta: f64,
    /// The change relative to the baseline (`delta / baseline`).
    pub(crate) relative_delta: f64,
    /// Abbreviated commit of the latest point, if known.
    pub(crate) commit: Option<String>,
}

impl Finding {
    /// Whether this finding is a regression (as opposed to an improvement).
    pub(crate) fn is_regression(&self) -> bool {
        self.direction == Direction::Regression
    }
}

/// The median of `values`, or `None` if empty.
///
/// Uses [`f64::total_cmp`] so `NaN` cannot corrupt the ordering, and computes the
/// midpoint index with checked integer arithmetic to satisfy the workspace lints.
fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let len = sorted.len();
    let mid = len.checked_div(2)?;
    if len.checked_rem(2) == Some(1) {
        sorted.get(mid).copied()
    } else {
        let lower = mid.checked_sub(1)?;
        let low = *sorted.get(lower)?;
        let high = *sorted.get(mid)?;
        Some(f64::midpoint(low, high))
    }
}

/// The median absolute deviation of `values` about `center`.
fn median_abs_deviation(values: &[f64], center: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let deviations: Vec<f64> = values.iter().map(|value| (value - center).abs()).collect();
    median(&deviations)
}

/// Classifies the magnitude of a relative move into a [`Severity`] tier.
fn classify_severity(relative_delta: f64) -> Severity {
    let magnitude = relative_delta.abs();
    if magnitude >= MAJOR_RELATIVE_DELTA {
        Severity::Major
    } else if magnitude >= MODERATE_RELATIVE_DELTA {
        Severity::Moderate
    } else {
        Severity::Minor
    }
}

/// Whether a larger value of `kind` indicates better performance.
///
/// Cache hit counts are the sole "higher is better" metric (more hits means fewer
/// misses); every other metric — wall time, instruction counts, estimated cycles,
/// branch counts — is lower-is-better.
fn higher_is_better(kind: MetricKind) -> bool {
    matches!(kind, MetricKind::CacheEvents)
}

/// The direction of a change, given the signed delta from the baseline and the
/// metric's polarity.
///
/// For a lower-is-better metric a positive delta is a regression; for a
/// higher-is-better metric (cache hits) the polarity is inverted. The caller only
/// reaches this with a delta beyond the noise threshold, so the exact zero case
/// never arises in practice; it is defined as an improvement so the classification
/// is total.
fn direction_of(delta: f64, kind: MetricKind) -> Direction {
    let worse = if higher_is_better(kind) {
        delta < 0.0
    } else {
        delta > 0.0
    };
    if worse {
        Direction::Regression
    } else {
        Direction::Improvement
    }
}

/// Evaluates a single series, returning a [`Finding`] if its latest point moved
/// beyond the noise-aware threshold derived from its recent history.
///
/// The baseline is the median of up to `config.window` points immediately before
/// the latest; the threshold is the larger of a relative floor (so a stable
/// series still flags a meaningful move) and a multiple of the baseline's median
/// absolute deviation (so a noisy series tolerates proportionate jitter).
pub(crate) fn evaluate_series(series: &Series, config: &RegressionConfig) -> Option<Finding> {
    let points = &series.points;
    let latest = points.last()?;

    let prior_count = points.len().checked_sub(1)?;
    if prior_count == 0 {
        return None;
    }
    let start = prior_count.saturating_sub(config.window);
    let baseline_points = points.get(start..prior_count)?;
    let baseline_values: Vec<f64> = baseline_points.iter().map(|point| point.value).collect();

    let baseline = median(&baseline_values)?;
    let mad = median_abs_deviation(&baseline_values, baseline)?;

    let delta = latest.value - baseline;
    let threshold = (config.relative_threshold * baseline.abs()).max(config.mad_multiplier * mad);
    if delta.abs() <= threshold {
        return None;
    }

    let relative_delta = if baseline.abs() <= f64::EPSILON {
        // A move away from a (near-)zero baseline is, proportionally, unbounded;
        // treat its sign as a full-magnitude relative move so it ranks as major.
        delta.signum()
    } else {
        delta / baseline
    };
    let direction = direction_of(delta, series.kind);

    Some(Finding {
        location: series.location.clone(),
        id: series.id.clone(),
        metric: series.metric.clone(),
        kind: series.kind,
        direction,
        severity: classify_severity(relative_delta),
        baseline,
        latest: latest.value,
        delta,
        relative_delta,
        commit: latest.commit.clone(),
    })
}

/// Evaluates every series and returns the findings, ranked most-notable first.
///
/// Findings are ordered by descending severity, then descending relative move,
/// then by a deterministic identity tie-break (location, benchmark, metric) so the
/// output is stable across runs.
pub(crate) fn find_changes(series: &[Series], config: &RegressionConfig) -> Vec<Finding> {
    let mut findings: Vec<Finding> = series
        .iter()
        .filter_map(|series| evaluate_series(series, config))
        .collect();
    findings.sort_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then_with(|| {
                right
                    .relative_delta
                    .abs()
                    .total_cmp(&left.relative_delta.abs())
            })
            .then_with(|| left.location.cmp(&right.location))
            .then_with(|| left.id.cmp(&right.id))
            .then_with(|| left.metric.cmp(&right.metric))
    });
    findings
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "metric values are exact integer-derived counts"
    )]
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use jiff::Timestamp;

    use crate::analyze::series::SeriesPoint;
    use crate::model::MetricKind;

    use super::*;

    /// Builds a series whose points carry the given values at ascending times.
    fn series_of(values: &[f64]) -> Series {
        let points = values
            .iter()
            .enumerate()
            .map(|(index, &value)| SeriesPoint {
                effective: Timestamp::from_second(i64::try_from(index).unwrap())
                    .expect("seconds within range"),
                object_key: format!("v1/p/callgrind/t/synthetic/{index}-c-r.json"),
                commit: Some(format!("commit{index}")),
                value,
            })
            .collect();
        Series {
            location: Location {
                system: "callgrind".to_owned(),
                target_triple: "t".to_owned(),
                machine: "synthetic".to_owned(),
            },
            id: BenchmarkId::new("group".to_owned(), Some("case".to_owned()), None),
            metric: "Ir".to_owned(),
            kind: MetricKind::InstructionCount,
            points,
        }
    }

    /// Builds a series like [`series_of`] but tagged with a specific metric kind,
    /// so polarity-dependent direction logic can be exercised.
    fn series_of_kind(values: &[f64], kind: MetricKind) -> Series {
        let mut series = series_of(values);
        series.kind = kind;
        series
    }

    #[test]
    fn median_of_odd_and_even_counts() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), Some(2.5));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn median_abs_deviation_about_center() {
        // Deviations are [2,1,0,1,2]; their median is 1.
        let mad = median_abs_deviation(&[1.0, 2.0, 3.0, 4.0, 5.0], 3.0);
        assert_eq!(mad, Some(1.0));
        assert_eq!(median_abs_deviation(&[], 0.0), None);
    }

    #[test]
    fn classify_severity_tiers() {
        assert_eq!(classify_severity(0.20), Severity::Major);
        assert_eq!(classify_severity(0.10), Severity::Major);
        assert_eq!(classify_severity(0.05), Severity::Moderate);
        assert_eq!(classify_severity(0.03), Severity::Moderate);
        assert_eq!(classify_severity(0.02), Severity::Minor);
        assert_eq!(classify_severity(-0.20), Severity::Major);
    }

    #[test]
    fn severity_orders_minor_below_major() {
        assert!(Severity::Minor < Severity::Moderate);
        assert!(Severity::Moderate < Severity::Major);
    }

    #[test]
    fn direction_of_treats_zero_and_negative_as_improvement() {
        assert_eq!(
            direction_of(1.0, MetricKind::InstructionCount),
            Direction::Regression
        );
        assert_eq!(
            direction_of(-1.0, MetricKind::InstructionCount),
            Direction::Improvement
        );
        // The boundary is improvement: only a strictly positive move regresses.
        assert_eq!(
            direction_of(0.0, MetricKind::InstructionCount),
            Direction::Improvement
        );
    }

    #[test]
    fn direction_of_inverts_for_higher_is_better_metrics() {
        // Cache hits are higher-is-better: more hits improves, fewer regresses.
        assert_eq!(
            direction_of(1.0, MetricKind::CacheEvents),
            Direction::Improvement
        );
        assert_eq!(
            direction_of(-1.0, MetricKind::CacheEvents),
            Direction::Regression
        );
        // The boundary stays improvement so the classification is total.
        assert_eq!(
            direction_of(0.0, MetricKind::CacheEvents),
            Direction::Improvement
        );
    }

    #[test]
    fn fewer_cache_hits_is_a_regression() {
        // Cache hits are higher-is-better, so a drop below the baseline regresses.
        let series = series_of_kind(&[100.0, 100.0, 100.0, 70.0], MetricKind::CacheEvents);
        let finding = evaluate_series(&series, &RegressionConfig::default()).expect("a regression");
        assert_eq!(finding.direction, Direction::Regression);
        assert!(finding.is_regression());
        assert_eq!(finding.delta, -30.0);
    }

    #[test]
    fn more_cache_hits_is_an_improvement() {
        let series = series_of_kind(&[100.0, 100.0, 100.0, 130.0], MetricKind::CacheEvents);
        let finding =
            evaluate_series(&series, &RegressionConfig::default()).expect("an improvement");
        assert_eq!(finding.direction, Direction::Improvement);
        assert!(!finding.is_regression());
        assert_eq!(finding.delta, 30.0);
    }

    #[test]
    fn flat_series_never_flags() {
        let series = series_of(&[100.0, 100.0, 100.0, 100.0]);
        assert!(evaluate_series(&series, &RegressionConfig::default()).is_none());
    }

    #[test]
    fn single_point_series_never_flags() {
        let series = series_of(&[100.0]);
        assert!(evaluate_series(&series, &RegressionConfig::default()).is_none());
    }

    #[test]
    fn large_regression_is_flagged_as_major() {
        // Baseline 100 (flat); latest jumps to 130 → +30% regression.
        let series = series_of(&[100.0, 100.0, 100.0, 130.0]);
        let finding = evaluate_series(&series, &RegressionConfig::default()).expect("a regression");
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.severity, Severity::Major);
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.latest, 130.0);
        assert_eq!(finding.delta, 30.0);
        assert!((finding.relative_delta - 0.30).abs() <= 1e-9);
        assert_eq!(finding.commit.as_deref(), Some("commit3"));
        assert!(finding.is_regression());
    }

    #[test]
    fn improvement_is_flagged_with_improvement_direction() {
        let series = series_of(&[100.0, 100.0, 100.0, 70.0]);
        let finding =
            evaluate_series(&series, &RegressionConfig::default()).expect("an improvement");
        assert_eq!(finding.direction, Direction::Improvement);
        assert!(!finding.is_regression());
        assert!(finding.delta < 0.0);
    }

    #[test]
    fn small_move_below_relative_floor_is_not_flagged() {
        // Flat baseline of 100 → relative floor is 1.0; a +0.5 move stays under it.
        let series = series_of(&[100.0, 100.0, 100.0, 100.5]);
        assert!(evaluate_series(&series, &RegressionConfig::default()).is_none());
    }

    #[test]
    fn move_just_above_relative_floor_is_flagged_as_minor() {
        // Flat baseline of 100 → relative floor is 1.0; a +2 move clears it but is
        // only a 2% change, so it is Minor.
        let series = series_of(&[100.0, 100.0, 100.0, 102.0]);
        let finding = evaluate_series(&series, &RegressionConfig::default()).expect("a move");
        assert_eq!(finding.severity, Severity::Minor);
        assert_eq!(finding.direction, Direction::Regression);
    }

    #[test]
    fn noisy_baseline_tolerates_proportionate_jitter() {
        // A noisy baseline lifts the MAD-based threshold so a same-scale latest
        // point does not flag. Baseline points: 80,120,80,120 → median 100, MAD 20,
        // MAD threshold = 60. Latest 150 → delta 50 ≤ 60, not flagged.
        let series = series_of(&[80.0, 120.0, 80.0, 120.0, 150.0]);
        assert!(evaluate_series(&series, &RegressionConfig::default()).is_none());
    }

    #[test]
    fn noisy_baseline_flags_a_move_beyond_the_mad_threshold() {
        // Same noisy baseline as above (80,120,80,120 → median 100, MAD 20, MAD
        // threshold 60), but the latest jumps to 180. Here the MAD-scaled threshold
        // (60), not the 1.0 relative floor, is the one delta=80 must clear — so this
        // exercises the realistic "regression on a noisy series" detection path. The
        // move is +80% of baseline, so it ranks Major.
        let series = series_of(&[80.0, 120.0, 80.0, 120.0, 180.0]);
        let finding = evaluate_series(&series, &RegressionConfig::default()).expect("a regression");
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.severity, Severity::Major);
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.latest, 180.0);
        assert_eq!(finding.delta, 80.0);
    }

    #[test]
    fn window_limits_baseline_to_recent_points() {
        // An ancient cheap run must not drag the baseline: with window 5 the
        // baseline is the last five priors (all 100), so latest 130 flags.
        let series = series_of(&[1.0, 100.0, 100.0, 100.0, 100.0, 100.0, 130.0]);
        let finding = evaluate_series(&series, &RegressionConfig::default()).expect("a regression");
        assert_eq!(finding.baseline, 100.0);
    }

    #[test]
    fn move_from_zero_baseline_is_major() {
        let series = series_of(&[0.0, 0.0, 0.0, 5.0]);
        let finding = evaluate_series(&series, &RegressionConfig::default()).expect("a move");
        assert_eq!(finding.severity, Severity::Major);
        assert_eq!(finding.direction, Direction::Regression);
    }

    #[test]
    fn find_changes_ranks_major_before_minor() {
        let major = series_of(&[100.0, 100.0, 100.0, 200.0]);
        let minor = series_of(&[100.0, 100.0, 100.0, 102.0]);
        let findings = find_changes(&[minor, major], &RegressionConfig::default());
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].severity, Severity::Major);
        assert_eq!(findings[1].severity, Severity::Minor);
    }

    #[test]
    fn find_changes_breaks_severity_ties_by_relative_move() {
        // Two major regressions: with equal severity the larger relative move
        // ranks first, exercising the relative-delta tie-break.
        let larger = series_of(&[100.0, 100.0, 100.0, 200.0]);
        let smaller = series_of(&[100.0, 100.0, 100.0, 150.0]);
        let findings = find_changes(&[smaller, larger], &RegressionConfig::default());
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].severity, Severity::Major);
        assert_eq!(findings[1].severity, Severity::Major);
        assert!(findings[0].relative_delta.abs() > findings[1].relative_delta.abs());
        assert_eq!(findings[0].latest, 200.0);
        assert_eq!(findings[1].latest, 150.0);
    }
}
