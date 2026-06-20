//! The finding algorithms: locate *sustained* level shifts and slow drifts in
//! each series and flag only those that survive engine-aware significance,
//! practical-magnitude, and false-discovery gates.
//!
//! The design deliberately distinguishes the two engine families this tool
//! records:
//!
//! * **Deterministic** engines (Callgrind: instruction counts, estimated cycles,
//!   cache events, branch counts) produce noise-free numbers. A real step is real
//!   no matter how small, so a deterministic change is flagged on persistence
//!   alone — no significance test, no false-discovery correction.
//! * **Noisy** engines (Criterion wall time) jitter from run to run. A move is
//!   flagged only when a Pettitt change-point locates a split that a Mann–Whitney
//!   rank test then confirms, the confidence intervals of the two regimes do not
//!   overlap, and the move clears a practical-magnitude floor — then the surviving
//!   candidates pass a Benjamini–Hochberg false-discovery filter so a batch of
//!   series does not manufacture spurious findings.
//!
//! A separate slow-[`Drift`](FindingMethod::Drift) finding is raised from a
//! Mann–Kendall trend test plus a Theil–Sen slope, and is suppressed when a
//! single step on the same series already explains at least as much movement.
//!
//! Polarity: most metrics are "lower is better" (instruction counts, cycle
//! estimates, branch counts, wall time), so a rise is a [`Direction::Regression`]
//! and a fall is a [`Direction::Improvement`]. Cache *hit* counts invert that —
//! more hits means fewer misses.

use serde::Serialize;

use crate::analyze::discriminant::DiscriminantSet;
use crate::analyze::series::{Series, SeriesPoint};
use crate::analyze::stats;
use crate::model::{BenchmarkId, MetricKind};

/// Relative move at or above which a change is classified [`Severity::Major`].
const MAJOR_RELATIVE_DELTA: f64 = 0.10;
/// Relative move at or above which a change is classified [`Severity::Moderate`].
const MODERATE_RELATIVE_DELTA: f64 = 0.03;

/// Tunable parameters of the engine-aware analysis.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AnalysisConfig {
    /// Minimum points each side of a change must have for the step to be trusted
    /// (persistence): a one-off blip on the latest point cannot flag.
    pub(crate) min_regime: usize,
    /// Significance level a noisy change-point's Mann–Whitney rank test must clear
    /// (Pettitt only locates the split; its analytic p-value is too conservative on
    /// short series to gate significance).
    pub(crate) change_alpha: f64,
    /// Target false-discovery rate for the Benjamini–Hochberg filter over noisy
    /// candidates.
    pub(crate) fdr_q: f64,
    /// Minimum points a series needs before a slow-drift finding is considered.
    pub(crate) drift_min_points: usize,
    /// Significance level a noisy drift's Mann–Kendall trend must clear.
    pub(crate) drift_alpha: f64,
    /// Minimum relative magnitude (3%) a noisy move must reach to matter in
    /// practice, regardless of statistical significance.
    pub(crate) practical_relative: f64,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            min_regime: 2,
            change_alpha: 0.05,
            fdr_q: 0.10,
            drift_min_points: 6,
            drift_alpha: 0.05,
            practical_relative: 0.03,
        }
    }
}

/// Which detector produced a finding.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FindingMethod {
    /// A sustained level shift located by the Pettitt change-point test.
    ChangePoint,
    /// A slow monotonic trend located by the Mann–Kendall / Theil–Sen pair.
    Drift,
}

/// The direction of a flagged change relative to the baseline.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Direction {
    /// The latest value is worse than the baseline.
    Regression,
    /// The latest value is better than the baseline.
    Improvement,
}

/// How large a flagged change is, in ascending order of importance.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Severity {
    /// A small but flagged change.
    Minor,
    /// A medium-sized change.
    Moderate,
    /// A large change.
    Major,
}

/// One flagged change: where it is, what moved, by how much, and how sure we are.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Finding {
    /// The comparable discriminant set the series belongs to.
    #[serde(flatten)]
    pub(crate) set: DiscriminantSet,
    /// The benchmark identity.
    #[serde(flatten)]
    pub(crate) id: BenchmarkId,
    /// The metric that moved.
    pub(crate) metric: String,
    /// The category of the metric that moved.
    pub(crate) kind: MetricKind,
    /// Which detector produced this finding.
    pub(crate) method: FindingMethod,
    /// Whether the move is a regression or an improvement.
    pub(crate) direction: Direction,
    /// How large the move is.
    pub(crate) severity: Severity,
    /// The before-regime representative value the after regime was compared to.
    pub(crate) baseline: f64,
    /// The after-regime representative value.
    pub(crate) latest: f64,
    /// The absolute change (`latest - baseline`).
    pub(crate) delta: f64,
    /// The change relative to the baseline (`delta / baseline`).
    pub(crate) relative_delta: f64,
    /// How confident the detector is (`1 - p_value`; `1.0` for an exact
    /// deterministic step).
    pub(crate) confidence: f64,
    /// Abbreviated commit the change is attributed to, if known.
    pub(crate) commit: Option<String>,
}

impl Finding {
    /// Whether this finding is a regression (as opposed to an improvement).
    pub(crate) fn is_regression(&self) -> bool {
        self.direction == Direction::Regression
    }
}

/// A finding before false-discovery filtering, carrying the p-value the
/// Benjamini–Hochberg pool needs, whether it came from a deterministic engine, and
/// the fitted model parameters used to arbitrate between the two detectors.
struct Candidate {
    /// The finding that will be emitted if it survives filtering.
    finding: Finding,
    /// The p-value contributed to the false-discovery pool (noisy candidates only).
    bh_p: f64,
    /// Whether the source engine is deterministic (bypasses the FDR filter).
    deterministic: bool,
    /// The Pettitt split index, for a change-point candidate.
    split: Option<usize>,
    /// The Theil–Sen `(slope, intercept)`, for a drift candidate.
    line: Option<(f64, f64)>,
}

/// Casts a small count to `f64`. Series lengths are far below 2^53, so the
/// conversion is exact.
#[expect(
    clippy::cast_precision_loss,
    reason = "series lengths are far below 2^53, so the cast is exact"
)]
fn count_to_f64(count: usize) -> f64 {
    count as f64
}

/// Whether a metric `kind` is measured by a deterministic engine.
///
/// Wall time is the sole noisy metric; every Callgrind-derived metric is exact.
fn is_deterministic(kind: MetricKind) -> bool {
    !matches!(kind, MetricKind::WallTime)
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
/// reaches this with a non-zero delta, so the exact zero case never arises in
/// practice; it is defined as an improvement so the classification is total.
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

/// The relative size of `delta` against `baseline`.
///
/// A move away from a (near-)zero baseline is proportionally unbounded; its sign
/// is returned as a full-magnitude move so it ranks as major.
fn relative_delta_of(delta: f64, baseline: f64) -> f64 {
    if baseline.abs() <= f64::EPSILON {
        delta.signum()
    } else {
        delta / baseline
    }
}

/// The representative confidence interval of a regime: the median of its points'
/// lower and upper bounds, available only when the engine reports dispersion.
fn regime_interval(points: &[&SeriesPoint]) -> Option<(f64, f64)> {
    let lows: Vec<f64> = points
        .iter()
        .filter_map(|point| point.interval_low)
        .collect();
    let highs: Vec<f64> = points
        .iter()
        .filter_map(|point| point.interval_high)
        .collect();
    if lows.is_empty() || highs.is_empty() {
        return None;
    }
    Some((stats::median(&lows)?, stats::median(&highs)?))
}

/// Whether two intervals are disjoint (the after regime sits wholly above or
/// wholly below the before regime).
fn intervals_disjoint(before: (f64, f64), after: (f64, f64)) -> bool {
    after.1 < before.0 || after.0 > before.1
}

/// The median confidence-interval half-width across `points`, when the engine
/// reports dispersion. Used as the per-measurement noise floor for noisy drift.
fn median_half_width(points: &[SeriesPoint]) -> Option<f64> {
    let halves: Vec<f64> = points
        .iter()
        .filter_map(|point| match (point.interval_low, point.interval_high) {
            (Some(low), Some(high)) => Some((high - low) / 2.0),
            _ => None,
        })
        .collect();
    if halves.is_empty() {
        return None;
    }
    stats::median(&halves)
}

/// The median absolute residual of the two-regime (step) model: each point's
/// distance from its own regime's median, split at `tau`.
fn step_model_residual(values: &[f64], tau: usize) -> Option<f64> {
    let before = values.get(..tau)?;
    let after = values.get(tau..)?;
    let before_median = stats::median(before)?;
    let after_median = stats::median(after)?;
    let residuals: Vec<f64> = before
        .iter()
        .map(|value| (value - before_median).abs())
        .chain(after.iter().map(|value| (value - after_median).abs()))
        .collect();
    stats::median(&residuals)
}

/// The median absolute residual of the linear (drift) model `intercept + slope·i`.
fn line_model_residual(values: &[f64], slope: f64, intercept: f64) -> Option<f64> {
    let residuals: Vec<f64> = values
        .iter()
        .enumerate()
        .map(|(index, value)| (value - (intercept + slope * count_to_f64(index))).abs())
        .collect();
    stats::median(&residuals)
}

/// Chooses between a change-point and a drift candidate for the same series.
///
/// When both detectors fire, the data is described as whichever model fits it
/// better — a sharp step leaves a flat residual under the two-regime model, while
/// a gradual ramp leaves a flat residual under the line — so we keep the candidate
/// with the smaller median absolute residual (ties favour the more specific
/// change-point). When only one fires, it is kept.
fn arbitrate(
    values: &[f64],
    change: Option<Candidate>,
    drift: Option<Candidate>,
) -> Option<Candidate> {
    match (change, drift) {
        (Some(change), Some(drift)) => {
            let step_residual = change
                .split
                .and_then(|tau| step_model_residual(values, tau));
            let line_residual = drift
                .line
                .and_then(|(slope, intercept)| line_model_residual(values, slope, intercept));
            match (step_residual, line_residual) {
                (Some(step), Some(line)) if line < step => Some(drift),
                _ => Some(change),
            }
        }
        (Some(change), None) => Some(change),
        (None, drift) => drift,
    }
}

/// Locates a sustained level shift in `series`, returning a [`Candidate`] when the
/// engine-appropriate gates pass.
///
/// The Pettitt test *locates* the split (its analytic p-value is conservative for
/// short series, so it is not used as a significance gate); both regimes must hold
/// at least `min_regime` points (persistence). A deterministic engine flags any
/// non-zero step. A noisy engine additionally requires a significant Mann–Whitney
/// rank-sum difference between the regimes, non-overlapping regime confidence
/// intervals (when reported), and a practically meaningful relative magnitude.
fn evaluate_change_point(series: &Series, config: &AnalysisConfig) -> Option<Candidate> {
    let points = &series.points;
    let n = points.len();
    let values: Vec<f64> = points.iter().map(|point| point.value).collect();

    let change = stats::pettitt(&values)?;
    let tau = change.index;
    let before_len = tau;
    let after_len = n.checked_sub(tau)?;
    if before_len < config.min_regime || after_len < config.min_regime {
        return None;
    }

    let before = values.get(..tau)?;
    let after = values.get(tau..)?;
    let baseline = stats::median(before)?;
    let latest = stats::median(after)?;
    let delta = latest - baseline;
    if delta.abs() <= 0.0 {
        return None;
    }
    let relative_delta = relative_delta_of(delta, baseline);

    let deterministic = is_deterministic(series.kind);
    let effective_p = if deterministic {
        0.0
    } else {
        let mann_whitney = stats::mann_whitney_u_pvalue(before, after);
        if mann_whitney >= config.change_alpha {
            return None;
        }
        if relative_delta.abs() < config.practical_relative {
            return None;
        }
        let before_points: Vec<&SeriesPoint> = points.iter().take(tau).collect();
        let after_points: Vec<&SeriesPoint> = points.iter().skip(tau).collect();
        if let (Some(before_ci), Some(after_ci)) = (
            regime_interval(&before_points),
            regime_interval(&after_points),
        ) && !intervals_disjoint(before_ci, after_ci)
        {
            return None;
        }
        mann_whitney
    };

    let commit = points.get(tau).and_then(|point| point.commit.clone());
    Some(Candidate {
        finding: Finding {
            set: series.set.clone(),
            id: series.id.clone(),
            metric: series.metric.clone(),
            kind: series.kind,
            method: FindingMethod::ChangePoint,
            direction: direction_of(delta, series.kind),
            severity: classify_severity(relative_delta),
            baseline,
            latest,
            delta,
            relative_delta,
            confidence: (1.0 - effective_p).clamp(0.0, 1.0),
            commit,
        },
        bh_p: effective_p,
        deterministic,
        split: Some(tau),
        line: None,
    })
}

/// Locates a slow monotonic drift in `series`, returning a [`Candidate`] when the
/// trend is significant and practically meaningful.
///
/// The trend is established by the Mann–Kendall test and quantified by the
/// Theil–Sen line, so a single outlier cannot manufacture a drift. For a noisy
/// engine the total movement must also exceed the per-measurement noise floor
/// (twice the median confidence-interval half-width), so jitter does not read as a
/// trend.
fn evaluate_drift(series: &Series, config: &AnalysisConfig) -> Option<Candidate> {
    let points = &series.points;
    let n = points.len();
    if n < config.drift_min_points {
        return None;
    }
    let values: Vec<f64> = points.iter().map(|point| point.value).collect();

    let trend = stats::mann_kendall(&values);
    if trend.p_value >= config.drift_alpha {
        return None;
    }
    let (slope, intercept) = stats::theil_sen_line(&values)?;
    let span = count_to_f64(n.checked_sub(1)?);
    let baseline = intercept;
    let latest = intercept + slope * span;
    let delta = latest - baseline;
    if delta.abs() <= 0.0 {
        return None;
    }
    let relative_delta = relative_delta_of(delta, baseline);
    if relative_delta.abs() < config.practical_relative {
        return None;
    }

    let deterministic = is_deterministic(series.kind);
    // A noisy trend must clear the measurement noise floor: the endpoints have to
    // separate by more than the run-to-run dispersion, or it is just jitter.
    if !deterministic
        && let Some(half_width) = median_half_width(points)
        && delta.abs() <= 2.0 * half_width
    {
        return None;
    }

    let commit = points.last().and_then(|point| point.commit.clone());
    Some(Candidate {
        finding: Finding {
            set: series.set.clone(),
            id: series.id.clone(),
            metric: series.metric.clone(),
            kind: series.kind,
            method: FindingMethod::Drift,
            direction: direction_of(delta, series.kind),
            severity: classify_severity(relative_delta),
            baseline,
            latest,
            delta,
            relative_delta,
            confidence: (1.0 - trend.p_value).clamp(0.0, 1.0),
            commit,
        },
        bh_p: trend.p_value,
        deterministic,
        split: None,
        line: Some((slope, intercept)),
    })
}

/// Evaluates every series and returns the surviving findings, ranked
/// most-notable first.
///
/// Each series contributes at most one finding: a change-point and a drift may
/// both fire, in which case the better-fitting model is kept. The surviving noisy
/// candidates are passed through a Benjamini–Hochberg false-discovery filter at
/// `config.fdr_q`; deterministic candidates bypass it (their values carry no
/// measurement noise). Survivors are ordered by descending severity, then
/// descending relative move, then method, then a deterministic identity tie-break.
pub(crate) fn find_changes(series: &[Series], config: &AnalysisConfig) -> Vec<Finding> {
    let mut candidates: Vec<Candidate> = Vec::new();
    for one in series {
        let values: Vec<f64> = one.points.iter().map(|point| point.value).collect();
        let change = evaluate_change_point(one, config);
        let drift = evaluate_drift(one, config);
        if let Some(chosen) = arbitrate(&values, change, drift) {
            candidates.push(chosen);
        }
    }

    // Control the false-discovery rate across the noisy candidates only; the
    // deterministic ones are exact and need no correction.
    let noisy_p: Vec<f64> = candidates
        .iter()
        .filter(|candidate| !candidate.deterministic)
        .map(|candidate| candidate.bh_p)
        .collect();
    let keep = stats::benjamini_hochberg(&noisy_p, config.fdr_q);
    let mut keep_iter = keep.into_iter();

    // `candidates` and `noisy_p` were built in the same order, so advancing
    // `keep_iter` exactly for the noisy candidates keeps the mask aligned.
    let mut findings: Vec<Finding> = candidates
        .into_iter()
        .filter_map(|candidate| {
            let survive = if candidate.deterministic {
                true
            } else {
                keep_iter.next().unwrap_or(false)
            };
            survive.then_some(candidate.finding)
        })
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
            .then_with(|| left.method.cmp(&right.method))
            .then_with(|| left.set.cmp(&right.set))
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

    use crate::analyze::discriminant::DiscriminantSet;
    use crate::analyze::series::SeriesPoint;
    use crate::model::MetricKind;

    use super::*;

    /// Builds a deterministic (Callgrind) series carrying `values` in topological
    /// order, with no dispersion.
    fn series_of(values: &[f64]) -> Series {
        series_with(values, MetricKind::InstructionCount, &[])
    }

    /// Builds a series tagged with `kind`. When `intervals` is non-empty it
    /// supplies a per-point confidence half-width, modelling a noisy engine; an
    /// empty `intervals` leaves the points without dispersion.
    fn series_with(values: &[f64], kind: MetricKind, intervals: &[f64]) -> Series {
        let points = values
            .iter()
            .enumerate()
            .map(|(index, &value)| {
                let half = intervals.get(index).copied();
                SeriesPoint {
                    topo_index: index,
                    dirty: false,
                    effective: Timestamp::from_second(i64::try_from(index).unwrap())
                        .expect("seconds within range"),
                    object_key: format!("v2/p/engine/t/synthetic/commit{index}/clean.json"),
                    commit: Some(format!("commit{index}")),
                    value,
                    interval_low: half.map(|half| value - half),
                    interval_high: half.map(|half| value + half),
                }
            })
            .collect();
        Series {
            set: DiscriminantSet {
                engine: "callgrind".to_owned(),
                target_triple: "t".to_owned(),
                machine: "synthetic".to_owned(),
            },
            id: BenchmarkId::new(None, "group".to_owned(), Some("case".to_owned()), None),
            metric: "metric".to_owned(),
            kind,
            points,
        }
    }

    /// A wall-time (noisy) series with a uniform confidence half-width on each
    /// point.
    fn wall_series(values: &[f64], half_width: f64) -> Series {
        let intervals = vec![half_width; values.len()];
        series_with(values, MetricKind::WallTime, &intervals)
    }

    fn only(findings: Vec<Finding>) -> Finding {
        assert_eq!(findings.len(), 1, "expected exactly one finding");
        findings.into_iter().next().expect("one finding")
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
    fn change_point_method_sorts_before_drift() {
        assert!(FindingMethod::ChangePoint < FindingMethod::Drift);
    }

    #[test]
    fn direction_of_respects_polarity() {
        assert_eq!(
            direction_of(1.0, MetricKind::InstructionCount),
            Direction::Regression
        );
        assert_eq!(
            direction_of(-1.0, MetricKind::InstructionCount),
            Direction::Improvement
        );
        // Cache hits invert: more hits improve, fewer regress.
        assert_eq!(
            direction_of(1.0, MetricKind::CacheEvents),
            Direction::Improvement
        );
        assert_eq!(
            direction_of(-1.0, MetricKind::CacheEvents),
            Direction::Regression
        );
    }

    #[test]
    fn deterministic_sustained_step_is_flagged_as_a_major_change_point() {
        // A clean step from 100 to 130 with three points each side.
        let series = series_of(&[100.0, 100.0, 100.0, 130.0, 130.0, 130.0]);
        let finding = only(find_changes(&[series], &AnalysisConfig::default()));
        assert_eq!(finding.method, FindingMethod::ChangePoint);
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.severity, Severity::Major);
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.latest, 130.0);
        assert_eq!(finding.delta, 30.0);
        assert!((finding.relative_delta - 0.30).abs() <= 1e-9);
        // An exact engine reports full confidence and attributes the change to the
        // first commit of the after regime.
        assert_eq!(finding.confidence, 1.0);
        assert_eq!(finding.commit.as_deref(), Some("commit3"));
    }

    #[test]
    fn deterministic_tiny_exact_step_is_still_flagged() {
        // A one-instruction step is real on a deterministic engine, so it flags
        // despite being far below any practical-magnitude floor.
        let series = series_of(&[1000.0, 1000.0, 1000.0, 1001.0, 1001.0, 1001.0]);
        let finding = only(find_changes(&[series], &AnalysisConfig::default()));
        assert_eq!(finding.method, FindingMethod::ChangePoint);
        assert_eq!(finding.delta, 1.0);
        assert_eq!(finding.severity, Severity::Minor);
        assert_eq!(finding.confidence, 1.0);
    }

    #[test]
    fn deterministic_cache_hit_drop_is_a_regression() {
        let series = series_with(
            &[100.0, 100.0, 100.0, 70.0, 70.0, 70.0],
            MetricKind::CacheEvents,
            &[],
        );
        let finding = only(find_changes(&[series], &AnalysisConfig::default()));
        assert!(finding.is_regression());
        assert_eq!(finding.delta, -30.0);
    }

    #[test]
    fn flat_series_never_flags() {
        let series = series_of(&[100.0, 100.0, 100.0, 100.0, 100.0, 100.0]);
        assert!(find_changes(&[series], &AnalysisConfig::default()).is_empty());
    }

    #[test]
    fn a_lone_blip_does_not_flag_a_change_point() {
        // A single spike returns to baseline: the after regime is one point, which
        // fails the persistence requirement.
        let series = series_of(&[100.0, 100.0, 100.0, 100.0, 100.0, 175.0]);
        assert!(find_changes(&[series], &AnalysisConfig::default()).is_empty());
    }

    #[test]
    fn step_in_the_final_point_fails_persistence() {
        // The shift only has one point after it (< min_regime), so it is rejected
        // even though the levels differ.
        let series = series_of(&[100.0, 100.0, 100.0, 100.0, 130.0]);
        assert!(find_changes(&[series], &AnalysisConfig::default()).is_empty());
    }

    #[test]
    fn noisy_jitter_around_a_stable_mean_is_not_flagged() {
        // Pure measurement jitter with no real shift must stay silent.
        let series = wall_series(&[100.0, 103.0, 98.0, 101.0, 99.0, 102.0, 97.0, 100.0], 5.0);
        assert!(find_changes(&[series], &AnalysisConfig::default()).is_empty());
    }

    #[test]
    fn noisy_sustained_step_with_disjoint_intervals_is_flagged() {
        // Two well-separated regimes (≈100 then ≈130) with tight, non-overlapping
        // confidence intervals: the realistic "regression on a noisy series" path.
        let series = wall_series(
            &[
                98.0, 100.0, 102.0, 99.0, 101.0, 128.0, 130.0, 132.0, 129.0, 131.0,
            ],
            2.0,
        );
        let finding = only(find_changes(&[series], &AnalysisConfig::default()));
        assert_eq!(finding.method, FindingMethod::ChangePoint);
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.latest, 130.0);
        // A genuinely significant step reports high (but sub-unit) confidence.
        assert!(finding.confidence > 0.95, "{}", finding.confidence);
        assert!(finding.confidence < 1.0, "{}", finding.confidence);
    }

    #[test]
    fn noisy_step_below_the_practical_floor_is_suppressed() {
        // A real but tiny (~1%) shift clears the statistical tests yet falls under
        // the 3% practical-magnitude floor, so it is not reported.
        let series = wall_series(
            &[
                1000.0, 1001.0, 999.0, 1000.0, 1001.0, 1010.0, 1011.0, 1009.0, 1010.0, 1011.0,
            ],
            1.0,
        );
        assert!(find_changes(&[series], &AnalysisConfig::default()).is_empty());
    }

    #[test]
    fn noisy_step_with_overlapping_intervals_is_suppressed() {
        // The point values separate cleanly, but each regime's confidence interval
        // is so wide that they overlap, so the change-point gate rejects it.
        let series = wall_series(
            &[
                98.0, 100.0, 102.0, 99.0, 101.0, 128.0, 130.0, 132.0, 129.0, 131.0,
            ],
            60.0,
        );
        assert!(find_changes(&[series], &AnalysisConfig::default()).is_empty());
    }

    #[test]
    fn deterministic_monotonic_drift_is_flagged() {
        // A steady climb with no single dominant step surfaces as a drift finding.
        let series = series_of(&[100.0, 104.0, 108.0, 112.0, 116.0, 120.0]);
        let finding = only(find_changes(&[series], &AnalysisConfig::default()));
        assert_eq!(finding.method, FindingMethod::Drift);
        assert_eq!(finding.direction, Direction::Regression);
        assert!(finding.delta > 0.0);
        // baseline = fitted intercept (100), latest = intercept + slope*(n-1).
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.latest, 120.0);
    }

    #[test]
    fn a_sharp_step_is_reported_as_a_change_point_not_a_drift() {
        // A series that both trends and steps: the two-regime model fits the sharp
        // jump better than a line, so it is reported once, as a change-point.
        let series = series_of(&[100.0, 101.0, 102.0, 160.0, 161.0, 162.0]);
        let findings = find_changes(&[series], &AnalysisConfig::default());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].method, FindingMethod::ChangePoint);
    }

    #[test]
    fn a_batch_of_pure_noise_produces_no_findings() {
        // Twelve independent noisy series, each only wobble within a wide
        // confidence band: every detector gate rejects them, so a batch of noise
        // manufactures no findings.
        let series: Vec<Series> = (0..12)
            .map(|seed: i32| {
                let bump = f64::from(seed.rem_euclid(3));
                wall_series(
                    &[
                        100.0 + bump,
                        99.0,
                        101.0,
                        100.0,
                        98.0 + bump,
                        102.0,
                        100.0,
                        101.0 - bump,
                    ],
                    6.0,
                )
            })
            .collect();
        assert!(find_changes(&series, &AnalysisConfig::default()).is_empty());
    }

    #[test]
    fn a_strong_noisy_signal_survives_the_false_discovery_filter() {
        // One unmistakable step alongside many flat series: the real finding is not
        // washed out by the correction.
        let mut series = vec![wall_series(
            &[
                98.0, 100.0, 102.0, 99.0, 101.0, 148.0, 150.0, 152.0, 149.0, 151.0,
            ],
            2.0,
        )];
        for _ in 0..6 {
            series.push(wall_series(
                &[100.0, 101.0, 99.0, 100.0, 101.0, 99.0, 100.0, 101.0],
                3.0,
            ));
        }
        let findings = find_changes(&series, &AnalysisConfig::default());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].method, FindingMethod::ChangePoint);
        assert_eq!(findings[0].baseline, 100.0);
        assert_eq!(findings[0].latest, 150.0);
    }

    #[test]
    fn find_changes_ranks_major_before_minor() {
        let major = series_of(&[100.0, 100.0, 100.0, 200.0, 200.0, 200.0]);
        let minor = series_of(&[1000.0, 1000.0, 1000.0, 1020.0, 1020.0, 1020.0]);
        let findings = find_changes(&[minor, major], &AnalysisConfig::default());
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].severity, Severity::Major);
        assert_eq!(findings[1].severity, Severity::Minor);
    }

    #[test]
    fn find_changes_breaks_severity_ties_by_relative_move() {
        let larger = series_with(
            &[100.0, 100.0, 100.0, 200.0, 200.0, 200.0],
            MetricKind::InstructionCount,
            &[],
        );
        let mut smaller = series_with(
            &[100.0, 100.0, 100.0, 150.0, 150.0, 150.0],
            MetricKind::InstructionCount,
            &[],
        );
        // Distinguish the identity so both findings are retained.
        smaller.id = BenchmarkId::new(None, "other".to_owned(), Some("case".to_owned()), None);
        let findings = find_changes(&[smaller, larger], &AnalysisConfig::default());
        assert_eq!(findings.len(), 2);
        assert!(findings[0].relative_delta.abs() > findings[1].relative_delta.abs());
        assert_eq!(findings[0].latest, 200.0);
        assert_eq!(findings[1].latest, 150.0);
    }
}
