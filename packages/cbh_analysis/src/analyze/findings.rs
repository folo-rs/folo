//! The finding algorithms: locate *sustained* level shifts and slow drifts in
//! each series and flag only those that survive significance, practical-magnitude,
//! series-intrinsic-noise, and false-discovery gates.
//!
//! No benchmark engine is treated as noise-free. Callgrind instruction and event
//! counts jitter a few percent run to run, and `alloc_tracker`'s per-iteration
//! figures carry warmup and buffer-resize allocations amortized over a
//! Criterion-chosen iteration count; Criterion wall time and `all_the_time`
//! processor time jitter more visibly still.
//!
//! The jitter is easy to underestimate because a Callgrind run *repeated on one
//! unchanged machine* often reports the same count every time — the counter is
//! deterministic for a fixed binary and input. What is not fixed is everything
//! feeding it across the commits we compare: a different OS or CPU-microcode
//! patch level, a different compiler patch release, the compiler's own
//! run-to-run nondeterministic code-generation choices (inlining, ordering,
//! layout) even at the same version, and Criterion scheduling a different
//! iteration count when background load differs (which shifts how warmup and
//! buffer-resize costs are amortized). Any of these perturbs the measured count
//! without the code under test changing, so no metric can be assumed
//! reproducible commit to commit. Every series is therefore judged noise-aware:
//!
//! * A Pettitt change-point *locates* a candidate split (its analytic p-value is
//!   too conservative on short series to gate significance); both regimes must
//!   hold at least `min_regime` points (persistence).
//! * A Mann–Whitney rank test must then confirm the two regimes differ, the move
//!   must clear a practical-magnitude floor, and it must exceed the series' own
//!   between-commit residual scatter (the primary, series-intrinsic noise gate).
//! * Where the engine reports a per-point confidence interval (Criterion,
//!   `all_the_time`, `alloc_tracker`) the two regimes' intervals must also be
//!   disjoint; if they overlap this veto *withholds* the finding, treating the
//!   move as measurement noise. The veto direction is one-way: it can only
//!   suppress a candidate the other gates would have reported — it can never
//!   promote a move into a finding.
//! * Surviving candidates then pass a Benjamini–Hochberg false-discovery filter so
//!   a batch of series does not manufacture spurious findings.
//!
//! A separate slow-[`Drift`](FindingMethod::Drift) finding is raised from a
//! Mann–Kendall trend test plus a Theil–Sen slope, gated by the same practical
//! floor and residual-scatter check, and is suppressed when a single step on the
//! same series already explains at least as much movement.
//!
//! Polarity: most metrics are "lower is better" (instruction counts, cycle
//! estimates, branch counts, allocations, wall time), so a rise is a
//! [`Direction::Regression`] and a fall is a [`Direction::Improvement`]. L1 cache
//! *hits* invert that — more hits means fewer, costlier misses (see
//! [`MetricKind::higher_is_better`]).

use std::ops::Range;
use std::sync::Arc;

use anyspawn::Spawner;
use cbh_stats as stats;
use serde::Serialize;

use crate::analyze::parallel::{balanced_chunk_sizes, worker_count};
use crate::analyze::{Series, SeriesPoint};
use crate::model::{BenchmarkId, DiscriminantSet, MetricKind};

/// Tunable parameters of the engine-aware analysis.
#[derive(Clone, Copy, Debug)]
pub struct AnalysisConfig {
    /// Minimum points each side of a change must have for the step to be trusted
    /// (persistence): a one-off blip on the latest point cannot flag.
    pub min_regime: usize,
    /// Significance level a noisy change-point's Mann–Whitney rank test must clear
    /// (Pettitt only locates the split; its analytic p-value is too conservative on
    /// short series to gate significance).
    pub change_alpha: f64,
    /// Target false-discovery rate for the Benjamini–Hochberg filter over noisy
    /// candidates.
    pub fdr_q: f64,
    /// Minimum points a series needs before a slow-drift finding is considered.
    pub drift_min_points: usize,
    /// Significance level a noisy drift's Mann–Kendall trend must clear.
    pub drift_alpha: f64,
    /// Minimum relative magnitude (3%) a noisy move must reach to matter in
    /// practice, regardless of statistical significance.
    pub practical_relative: f64,
    /// How many recent base-side points form the level a branch's latest state is
    /// compared against (branch mode).
    pub compare_window: usize,
    /// Minimum relative magnitude a noisy *branch* move must reach. Raised above the
    /// history floor: a feature-branch signal must be high-confidence, since we
    /// would rather miss a small move than cry wolf on a pull request.
    pub branch_practical_relative: f64,
    /// Multiple of the per-measurement noise floor a noisy branch move with too
    /// few points to rank-test must exceed before it is trusted.
    pub branch_noise_multiple: f64,
    /// Multiple of a series' own between-commit residual scatter (median absolute
    /// residual of the fitted step or line model) that a move must exceed before it
    /// is trusted. This is the primary, series-intrinsic noise gate applied to every
    /// engine: a clean series has near-zero residual scatter, so any persistent move
    /// clears it, while a jittery series demands a move that stands out above its own
    /// run-to-run wobble. It composes with (and is independent of) the optional
    /// confidence-interval veto available on dispersion-reporting engines.
    pub residual_noise_multiple: f64,
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
            compare_window: 8,
            branch_practical_relative: 0.05,
            branch_noise_multiple: 2.0,
            residual_noise_multiple: 3.0,
        }
    }
}

/// Which analysis a [`find_changes_spawned`] pass performs.
///
/// The mode is auto-detected by the caller from git topology and the admitted data
/// set (a base branch whose tip is its own merge-base with no dirty run admitted on
/// that tip is [`History`](AnalysisMode::History); commits — or an admitted dirty run
/// — on top of the base make it [`Branch`](AnalysisMode::Branch)). The working tree
/// affects the choice only indirectly, through the exception that admits a base-tip
/// dirty run while the tree is dirty.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisMode {
    /// Long-range trend and change-point analysis over a base branch's history.
    History,
    /// Latest-state comparison of a feature branch against its base, ignoring the
    /// intermediate stages the branch passed through.
    Branch,
}

impl AnalysisMode {
    /// The lowercase wire name of the mode.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::Branch => "branch",
        }
    }
}

/// The context a [`find_changes_spawned`] pass runs in.
///
/// Carries which analysis to perform, the tuned parameters, where the branch forks
/// from its base (branch mode only), and whether improvements are reported
/// alongside regressions.
#[derive(Clone, Copy, Debug)]
pub struct AnalysisContext {
    /// The analysis to perform.
    pub mode: AnalysisMode,
    /// The tuned analysis parameters.
    pub config: AnalysisConfig,
    /// First-parent topological index of the merge-base commit, splitting base-side
    /// history from the branch. `None` means no split is known (every point is
    /// treated as branch-side). Consulted only in [`AnalysisMode::Branch`].
    pub merge_base_index: Option<usize>,
    /// Whether improvements are reported. History mode defaults to regressions only
    /// (scheduled drift watch); branch mode always reports both.
    pub include_improvements: bool,
    /// Whether *inactive* (recovered) findings are reported. History mode hides a
    /// change whose level has since returned to baseline unless this is set; branch
    /// mode only ever looks at the latest state, so it has no inactive findings.
    pub include_inactive: bool,
}

impl AnalysisContext {
    /// Whether a finding of the given `direction` is reported in this mode.
    fn keeps(&self, direction: Direction) -> bool {
        match self.mode {
            AnalysisMode::History => {
                direction == Direction::Regression || self.include_improvements
            }
            AnalysisMode::Branch => true,
        }
    }

    /// Whether this analysis reports improvements at all. `false` for the
    /// regressions-only case (history mode's default drift watch), where an
    /// always-zero improvement tally is noise the report omits.
    #[must_use]
    pub fn reports_improvements(&self) -> bool {
        self.keeps(Direction::Improvement)
    }
}

/// Which detector produced a finding.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingMethod {
    /// A sustained level shift located by the Pettitt change-point test.
    ChangePoint,
    /// A slow monotonic trend located by the Mann–Kendall / Theil–Sen pair.
    Drift,
}

/// The direction of a flagged change relative to the baseline.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// The latest value is worse than the baseline.
    Regression,
    /// The latest value is better than the baseline.
    Improvement,
}

/// One point of a finding's underlying series, retained for charting.
///
/// Carries, for charting and provenance, the commit it was measured against, the
/// value, and whether it came from a dirty (uncommitted-tree) snapshot.
#[derive(Clone, Debug)]
pub struct SeriesValue {
    /// Commit the point was measured against, if known.
    pub commit: Option<String>,
    /// The measured value.
    pub value: f64,
    /// Whether the point is a dirty (uncommitted-tree) snapshot.
    pub dirty: bool,
}

/// One flagged change: where it is, what moved, by how much, and how sure we are.
#[derive(Clone, Debug)]
pub struct Finding {
    /// The comparable discriminant set the series belongs to.
    pub set: DiscriminantSet,
    /// The benchmark identity.
    pub id: BenchmarkId,
    /// The category of the metric that moved (governs unit and polarity).
    pub kind: MetricKind,
    /// Which detector produced this finding.
    pub method: FindingMethod,
    /// Whether the move is a regression or an improvement.
    pub direction: Direction,
    /// The before-regime representative value the after regime was compared to.
    pub baseline: f64,
    /// The after-regime representative value.
    pub latest: f64,
    /// The absolute change (`latest - baseline`).
    pub delta: f64,
    /// The change relative to the baseline (`delta / baseline`).
    pub relative_delta: f64,
    /// How confident the detector is (`1 - p_value` of the significance test that
    /// confirmed the move).
    pub confidence: f64,
    /// Commit the change is attributed to, if known.
    pub commit: Option<String>,
    /// Where, within a feature branch, the latest regime began — set only in
    /// branch mode when a within-branch flip is located, naming the commit the
    /// move starts at, so a "got worse late in the branch" finding can point at it.
    /// In history mode, an inactive (recovered) finding sets this to the commit at
    /// which the level returned to baseline.
    pub flipped_at: Option<String>,
    /// Whether the change is still reflected in the latest measured state. An active
    /// finding's current level still differs from baseline; an inactive one has
    /// since recovered (history mode only — branch always looks at the latest
    /// state, so its findings are always active).
    pub active: bool,
    /// Index into `series` at which the active (post-blessing) window begins; points
    /// before it are pre-blessing history, retained for charting but excluded from
    /// detection. `0` when the series is unblessed.
    pub active_from: usize,
    /// Abbreviated commit of the blessing that re-baselined this series, if any.
    pub blessed_at: Option<String>,
    /// Effective (committer) time of the blessed commit, RFC 3339, if blessed.
    pub blessed_commit_time: Option<String>,
    /// The full underlying series, oldest-first. Retained internally so the text and
    /// Markdown reports can draw a chart; it is not part of the machine-readable JSON
    /// contract.
    pub series: Vec<SeriesValue>,
}

impl Finding {
    /// Whether this finding is a regression (as opposed to an improvement).
    #[must_use]
    pub fn is_regression(&self) -> bool {
        self.direction == Direction::Regression
    }
}

/// A finding before false-discovery filtering, carrying the p-value the
/// Benjamini–Hochberg pool needs and the fitted model parameters used to arbitrate
/// between the two detectors.
struct Candidate {
    /// The finding that will be emitted if it survives filtering.
    finding: Finding,
    /// Index of the source series in the analysed slice. The finding's charting
    /// points ([`Finding::series`]) are materialised from it only once the
    /// candidate survives filtering, so a dropped candidate never pays for them.
    source_index: usize,
    /// The p-value contributed to the false-discovery pool.
    bh_p: f64,
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

/// The full series, oldest-first, as compact [`SeriesValue`] points for the JSON
/// output (charting and provenance).
fn series_values(series: &Series) -> Vec<SeriesValue> {
    series
        .points
        .iter()
        .map(|point| SeriesValue {
            commit: owned_commit(point),
            value: point.value,
            dirty: point.dirty,
        })
        .collect()
}

/// The commit of a point as an owned `String`, for the JSON output.
///
/// Points intern their commit as a shared `Arc<str>`; the public finding fields are
/// plain owned strings, so a surviving finding pays one allocation here rather than
/// every point carrying its own copy.
fn owned_commit(point: &SeriesPoint) -> Option<String> {
    point.commit.as_deref().map(str::to_owned)
}

/// The direction of a change, given the signed delta from the baseline and the
/// metric's `kind`.
///
/// For a lower-is-better metric a positive delta is a regression; for a
/// higher-is-better metric (L1 cache hits) the polarity is inverted (see
/// [`MetricKind::higher_is_better`]). The caller only reaches this with a non-zero
/// delta, so the exact zero case never arises in practice; it is defined as an
/// improvement so the classification is total.
fn direction_of(delta: f64, kind: MetricKind) -> Direction {
    let worse = if kind.higher_is_better() {
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
    let mut lows: Vec<f64> = points
        .iter()
        .filter_map(|point| point.interval_low)
        .collect();
    let mut highs: Vec<f64> = points
        .iter()
        .filter_map(|point| point.interval_high)
        .collect();
    // `median_in_place` yields `None` for an empty side, so a regime missing either
    // bound short-circuits here without a separate emptiness guard.
    Some((
        stats::median_in_place(&mut lows)?,
        stats::median_in_place(&mut highs)?,
    ))
}

/// Whether two intervals are disjoint (the after regime sits wholly above or
/// wholly below the before regime).
fn intervals_disjoint(before: (f64, f64), after: (f64, f64)) -> bool {
    after.1 < before.0 || after.0 > before.1
}

/// The median confidence-interval half-width across `points`, when the engine
/// reports dispersion. Used as the per-measurement noise floor for noisy drift.
fn median_half_width(points: &[SeriesPoint]) -> Option<f64> {
    let mut halves: Vec<f64> = points
        .iter()
        .filter_map(|point| match (point.interval_low, point.interval_high) {
            (Some(low), Some(high)) => Some((high - low) / 2.0),
            _ => None,
        })
        .collect();
    if halves.is_empty() {
        return None;
    }
    stats::median_in_place(&mut halves)
}

/// The median absolute residual of the two-regime (step) model: each point's
/// distance from its own regime's median, split at `tau`.
fn step_model_residual(values: &[f64], tau: usize) -> Option<f64> {
    let before = values.get(..tau)?;
    let after = values.get(tau..)?;
    let before_median = stats::median(before)?;
    let after_median = stats::median(after)?;
    let mut residuals: Vec<f64> = before
        .iter()
        .map(|value| (value - before_median).abs())
        .chain(after.iter().map(|value| (value - after_median).abs()))
        .collect();
    stats::median_in_place(&mut residuals)
}

/// The median absolute residual of the linear (drift) model `intercept + slope·i`.
fn line_model_residual(values: &[f64], slope: f64, intercept: f64) -> Option<f64> {
    let mut residuals: Vec<f64> = values
        .iter()
        .enumerate()
        .map(|(index, value)| (value - (intercept + slope * count_to_f64(index))).abs())
        .collect();
    stats::median_in_place(&mut residuals)
}

/// The median absolute residual of a two-sample step model: each sample's points'
/// distance from their own sample median.
fn sample_step_residual(before: &[f64], after: &[f64]) -> Option<f64> {
    let before_median = stats::median(before)?;
    let after_median = stats::median(after)?;
    let mut residuals: Vec<f64> = before
        .iter()
        .map(|value| (value - before_median).abs())
        .chain(after.iter().map(|value| (value - after_median).abs()))
        .collect();
    stats::median_in_place(&mut residuals)
}

/// Whether `delta` stands clear of a series' own between-commit scatter: it must
/// exceed `config.residual_noise_multiple` times the model's median absolute
/// residual. A clean series has a near-zero residual, so any persistent move
/// passes; a jittery one demands a move that stands out above its wobble. A missing
/// residual (an empty model) is treated as no evidence of noise, so the move is
/// trusted.
fn exceeds_residual_noise(delta: f64, residual: Option<f64>, config: &AnalysisConfig) -> bool {
    match residual {
        Some(residual) => delta.abs() > config.residual_noise_multiple * residual,
        None => true,
    }
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
/// noise-aware gates pass.
///
/// The Pettitt test *locates* the split (its analytic p-value is conservative for
/// short series, so it is not used as a significance gate); both regimes must hold
/// at least `min_regime` points (persistence). The move must then be confirmed by a
/// significant Mann–Whitney rank-sum difference between the regimes, clear the
/// practical-magnitude floor, stand above the series' own between-commit residual
/// scatter, and — when the engine reports per-point confidence intervals — separate
/// the two regimes' intervals.
fn evaluate_change_point(
    series: &Series,
    values: &[f64],
    config: &AnalysisConfig,
) -> Option<Candidate> {
    let points = &series.points;
    let n = points.len();

    let change = stats::pettitt(values)?;
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

    let mann_whitney = stats::mann_whitney_u_pvalue(before, after);
    if mann_whitney >= config.change_alpha {
        return None;
    }
    if relative_delta.abs() < config.practical_relative {
        return None;
    }
    if !exceeds_residual_noise(delta, step_model_residual(values, tau), config) {
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
    let effective_p = mann_whitney;

    let commit = points.get(tau).and_then(owned_commit);
    Some(Candidate {
        finding: Finding {
            set: series.set.clone(),
            id: series.id.clone(),
            kind: series.kind,
            method: FindingMethod::ChangePoint,
            direction: direction_of(delta, series.kind),
            baseline,
            latest,
            delta,
            relative_delta,
            confidence: (1.0 - effective_p).clamp(0.0, 1.0),
            commit,
            flipped_at: None,
            active: true,
            active_from: 0,
            blessed_at: None,
            blessed_commit_time: None,
            series: Vec::new(),
        },
        source_index: 0,
        bh_p: effective_p,
        split: Some(tau),
        line: None,
    })
}

/// Locates a slow monotonic drift in `series`, returning a [`Candidate`] when the
/// trend is significant and practically meaningful.
///
/// The trend is established by the Mann–Kendall test and quantified by the
/// Theil–Sen line, so a single outlier cannot manufacture a drift. The total
/// movement must clear the practical-magnitude floor and stand above the series'
/// own residual scatter about the fitted line; where the engine reports confidence
/// intervals it must additionally exceed the per-measurement noise floor (twice the
/// median half-width), so jitter does not read as a trend.
fn evaluate_drift(series: &Series, values: &[f64], config: &AnalysisConfig) -> Option<Candidate> {
    let points = &series.points;
    let n = points.len();
    if n < config.drift_min_points {
        return None;
    }

    let trend = stats::mann_kendall(values);
    if trend.p_value >= config.drift_alpha {
        return None;
    }
    let (slope, intercept) = stats::theil_sen_line(values)?;
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
    if !exceeds_residual_noise(delta, line_model_residual(values, slope, intercept), config) {
        return None;
    }
    // Where the engine reports dispersion, a trend must also clear the measurement
    // noise floor: the endpoints have to separate by more than the run-to-run
    // dispersion, or it is just jitter.
    if let Some(half_width) = median_half_width(points)
        && delta.abs() <= 2.0 * half_width
    {
        return None;
    }

    let commit = points.last().and_then(owned_commit);
    Some(Candidate {
        finding: Finding {
            set: series.set.clone(),
            id: series.id.clone(),
            kind: series.kind,
            method: FindingMethod::Drift,
            direction: direction_of(delta, series.kind),
            baseline,
            latest,
            delta,
            relative_delta,
            confidence: (1.0 - trend.p_value).clamp(0.0, 1.0),
            commit,
            flipped_at: None,
            active: true,
            active_from: 0,
            blessed_at: None,
            blessed_commit_time: None,
            series: Vec::new(),
        },
        source_index: 0,
        bh_p: trend.p_value,
        split: None,
        line: Some((slope, intercept)),
    })
}

/// The last `window` entries of `points` (all of them when shorter).
fn recent<'a>(points: &[&'a SeriesPoint], window: usize) -> Vec<&'a SeriesPoint> {
    let start = points.len().saturating_sub(window);
    points
        .get(start..)
        .map(<[&SeriesPoint]>::to_vec)
        .unwrap_or_default()
}

/// Splits a series' points into `(base_side, branch_side)` at the merge-base.
///
/// A point is branch-side when its commit sits past the merge-base, or when it is
/// a dirty snapshot exactly at the merge-base (the dirty-base-tip exception, where
/// the merge-base *is* the tip). With no merge-base every point is branch-side.
fn split_at_merge_base(
    points: &[SeriesPoint],
    merge_base_index: Option<usize>,
) -> (Vec<&SeriesPoint>, Vec<&SeriesPoint>) {
    let Some(merge_base) = merge_base_index else {
        return (Vec::new(), points.iter().collect());
    };
    let mut base = Vec::new();
    let mut branch = Vec::new();
    for point in points {
        if point.topo_index > merge_base || (point.topo_index == merge_base && point.dirty) {
            branch.push(point);
        } else {
            base.push(point);
        }
    }
    (base, branch)
}

/// The branch's *latest regime* and where it began.
///
/// A feature branch may have changed direction partway through (improved, then
/// regressed). We care only about its current state, so a Pettitt split on the
/// branch values isolates the most recent regime; the split is accepted only when
/// the within-branch move is practically meaningful, otherwise the whole branch is
/// one regime with no flip point. Too few points to split leaves the branch whole.
fn latest_regime<'a>(
    branch: &[&'a SeriesPoint],
    config: &AnalysisConfig,
) -> (Vec<&'a SeriesPoint>, Option<String>) {
    let whole = || (branch.to_vec(), None);
    if branch.len() < 3 {
        return whole();
    }
    let values: Vec<f64> = branch.iter().map(|point| point.value).collect();
    let Some(change) = stats::pettitt(&values) else {
        return whole();
    };
    let tau = change.index;
    let (Some(before), Some(after)) = (values.get(..tau), values.get(tau..)) else {
        return whole();
    };
    // `pettitt` reports a split index in `1..=n-1`, so both sides are non-empty;
    // an empty side would short-circuit on the `median` guard below regardless.
    let (Some(before_median), Some(after_median)) = (stats::median(before), stats::median(after))
    else {
        return whole();
    };
    // Only treat the split as a real direction change when the within-branch move
    // clears the practical floor; otherwise the branch is a single regime.
    if relative_delta_of(after_median - before_median, before_median).abs()
        < config.practical_relative
    {
        return whole();
    }
    let after_points = branch
        .get(tau..)
        .map(<[&SeriesPoint]>::to_vec)
        .unwrap_or_default();
    let flipped_at = branch.get(tau).and_then(|&point| owned_commit(point));
    (after_points, flipped_at)
}

/// Compares a `before` sample against an `after` sample on the same series and, if
/// the noise-aware gates pass, returns a change-point [`Candidate`].
///
/// The relative move must clear `practical_floor` and stand above the two samples'
/// own between-commit residual scatter (the primary, series-intrinsic noise gate,
/// which for a single-run engine like Callgrind is the only dispersion available).
/// It must then either — when both samples have at least two points — pass a
/// significant Mann–Whitney difference, or — when a sample is too small to
/// rank-test — rest on that residual gate alone. Where the engine additionally
/// reports per-point confidence intervals, the two samples' intervals must also be
/// disjoint; this is an extra veto that can only *suppress* a candidate the other
/// gates would have reported (treating the move as noise when the intervals
/// overlap) — it never turns a non-finding into a finding.
fn compare_samples(
    series: &Series,
    before: &[&SeriesPoint],
    after: &[&SeriesPoint],
    config: &AnalysisConfig,
    practical_floor: f64,
    commit: Option<String>,
    flipped_at: Option<String>,
) -> Option<Candidate> {
    let before_values: Vec<f64> = before.iter().map(|point| point.value).collect();
    let after_values: Vec<f64> = after.iter().map(|point| point.value).collect();
    let baseline = stats::median(&before_values)?;
    let latest = stats::median(&after_values)?;
    let delta = latest - baseline;
    if delta.abs() <= 0.0 {
        return None;
    }
    let relative_delta = relative_delta_of(delta, baseline);

    if relative_delta.abs() < practical_floor {
        return None;
    }
    if !exceeds_residual_noise(
        delta,
        sample_step_residual(&before_values, &after_values),
        config,
    ) {
        return None;
    }
    let effective_p = if before_values.len() >= 2 && after_values.len() >= 2 {
        let mann_whitney = stats::mann_whitney_u_pvalue(&before_values, &after_values);
        if mann_whitney >= config.change_alpha {
            return None;
        }
        if let (Some(before_ci), Some(after_ci)) = (regime_interval(before), regime_interval(after))
            && !intervals_disjoint(before_ci, after_ci)
        {
            return None;
        }
        mann_whitney
    } else {
        // Too few points to rank-test (typically a single fresh tip or branch run):
        // the residual gate above is the significance proxy. Where per-point
        // confidence intervals exist, require the move to also clear the measurement
        // noise band as an additional veto that can only suppress this candidate
        // (never create one).
        let points: Vec<SeriesPoint> = before
            .iter()
            .chain(after.iter())
            .map(|point| (*point).clone())
            .collect();
        if let Some(half_width) = median_half_width(&points)
            && delta.abs() <= config.branch_noise_multiple * half_width
        {
            return None;
        }
        config.change_alpha
    };

    Some(Candidate {
        finding: Finding {
            set: series.set.clone(),
            id: series.id.clone(),
            kind: series.kind,
            method: FindingMethod::ChangePoint,
            direction: direction_of(delta, series.kind),
            baseline,
            latest,
            delta,
            relative_delta,
            confidence: (1.0 - effective_p).clamp(0.0, 1.0),
            commit,
            flipped_at,
            active: true,
            active_from: 0,
            blessed_at: None,
            blessed_commit_time: None,
            series: Vec::new(),
        },
        source_index: 0,
        bh_p: effective_p,
        split: None,
        line: None,
    })
}

/// Evaluates a series in *branch* mode: compares the branch's latest state against
/// the recent base level, in either direction.
///
/// The branch's intermediate stages are ignored — only its latest regime matters
/// (see [`latest_regime`]). A new benchmark introduced on the branch (no base-side
/// points) or an empty branch yields nothing, since there is no baseline to compare.
fn evaluate_branch(
    series: &Series,
    config: &AnalysisConfig,
    merge_base_index: Option<usize>,
) -> Option<Candidate> {
    let (base, branch) = split_at_merge_base(&series.points, merge_base_index);
    // An empty base or branch yields nothing: `compare_samples` returns `None` once
    // either sample's median is absent, so no explicit emptiness guard is needed.
    let base_window = recent(&base, config.compare_window);
    let (latest_points, flipped_at) = latest_regime(&branch, config);
    let commit = branch.last().and_then(|&point| owned_commit(point));
    compare_samples(
        series,
        &base_window,
        &latest_points,
        config,
        config.branch_practical_relative,
        commit,
        flipped_at,
    )
}

/// The post-blessing window of `series` as a standalone series for detection.
///
/// History-mode detection runs on this view so a blessed (re-baselined) series is
/// only judged from the blessed commit onward; the full series is restored on the
/// finding afterwards for charting. An unblessed series (`active_start == 0`) yields
/// an equivalent copy.
fn active_view(series: &Series) -> Series {
    if series.active_start == 0 {
        return series.clone();
    }
    let points = series
        .points
        .get(series.active_start..)
        .map(<[SeriesPoint]>::to_vec)
        .unwrap_or_default();
    Series {
        set: series.set.clone(),
        id: series.id.clone(),
        kind: series.kind,
        points,
        active_start: 0,
        blessing: None,
    }
}

/// Records a history-mode finding's re-baseline provenance, so the chart can grey
/// the pre-blessing prefix and the report can name the blessing.
///
/// The finding's charting points ([`Finding::series`]) are filled in later, when
/// the candidate survives filtering (see [`find_changes_spawned`]); a dropped candidate
/// never builds them.
fn stamp_history(finding: &mut Finding, series: &Series) {
    finding.active_from = series.active_start;
    if let Some(blessing) = &series.blessing {
        finding.blessed_at = Some(short_commit(&blessing.commit));
        finding.blessed_commit_time = blessing.commit_time.map(|time| time.to_string());
    }
}

/// Abbreviates a commit ID for display (first 12 hex digits).
#[must_use]
pub fn short_commit(commit: &str) -> String {
    commit.get(..12).unwrap_or(commit).to_owned()
}

/// Largest interior window size resolved-spike search will scan; longer histories
/// skip the (quadratic) search rather than stall.
const RESOLVED_SPIKE_MAX_POINTS: usize = 200;

/// Locates a *recovered* spike in a (re-baselined) history series: a sustained
/// interior regime that deviated from baseline and has since returned to it.
///
/// Such a change is no longer reflected in the latest state, so it is emitted as an
/// *inactive* finding (only surfaced with `--include-inactive`): `commit` names where
/// the level rose, `flipped_at` where it recovered, `baseline` the pre-spike level,
/// and `latest` the spike's own level (its magnitude is what is notable). Both the
/// rise and the recovery must be Mann–Whitney significant, the plateau must clear
/// the practical-magnitude floor, and the deviation must stand above the rise's own
/// residual scatter.
fn evaluate_resolved_spike(
    series: &Series,
    values: &[f64],
    config: &AnalysisConfig,
) -> Option<Candidate> {
    let points = &series.points;
    let n = points.len();
    if n > RESOLVED_SPIKE_MAX_POINTS {
        return None;
    }
    let min = config.min_regime.max(1);
    // Baseline, elevated middle, and recovery each need at least `min` points.
    if n < min.checked_mul(3)? {
        return None;
    }
    let baseline = stats::median(values.get(..min)?)?;
    let current = stats::median(values.get(n.checked_sub(min)?..)?)?;
    // Only a spike that has recovered qualifies; a still-elevated tail is an active
    // change-point, handled by `evaluate_change_point`.
    if relative_delta_of(current - baseline, baseline).abs() >= config.practical_relative {
        return None;
    }

    // Find the most-deviated sustained plateau [start, end) with a baseline segment
    // [0, start) and a recovery segment [end, n) each at least `min` points long.
    let mut best: Option<(usize, usize, f64, f64)> = None;
    let mut start = min;
    while start <= n.saturating_sub(min.saturating_mul(2)) {
        let mut end = start.saturating_add(min);
        while end <= n.saturating_sub(min) {
            if let Some(segment) = values.get(start..end)
                && let Some(level) = stats::median(segment)
            {
                let deviation = level - baseline;
                if best.is_none_or(|(_, _, _, best_dev): (usize, usize, f64, f64)| {
                    deviation.abs() > best_dev.abs()
                }) {
                    best = Some((start, end, level, deviation));
                }
            }
            end = end.saturating_add(1);
        }
        start = start.saturating_add(1);
    }

    let (rise, recovery, level, deviation) = best?;
    if deviation.abs() <= 0.0
        || relative_delta_of(deviation, baseline).abs() < config.practical_relative
    {
        return None;
    }

    let before = values.get(..rise)?;
    let segment = values.get(rise..recovery)?;
    let after = values.get(recovery..)?;
    if !exceeds_residual_noise(deviation, sample_step_residual(before, segment), config) {
        return None;
    }
    let rise_p = stats::mann_whitney_u_pvalue(before, segment);
    let recovery_p = stats::mann_whitney_u_pvalue(segment, after);
    if rise_p >= config.change_alpha || recovery_p >= config.change_alpha {
        return None;
    }
    let effective_p = rise_p.max(recovery_p);

    let relative_delta = relative_delta_of(deviation, baseline);
    Some(Candidate {
        finding: Finding {
            set: series.set.clone(),
            id: series.id.clone(),
            kind: series.kind,
            method: FindingMethod::ChangePoint,
            direction: direction_of(deviation, series.kind),
            baseline,
            latest: level,
            delta: deviation,
            relative_delta,
            confidence: (1.0 - effective_p).clamp(0.0, 1.0),
            commit: points.get(rise).and_then(owned_commit),
            flipped_at: points.get(recovery).and_then(owned_commit),
            active: false,
            active_from: 0,
            blessed_at: None,
            blessed_commit_time: None,
            series: Vec::new(),
        },
        source_index: 0,
        bh_p: effective_p,
        split: Some(rise),
        line: None,
    })
}

/// Serial reference for the spawner-distributed [`find_changes_spawned`]: detects
/// every series in one contiguous scan, then runs the shared finalize tail.
///
/// Exists only as test scaffolding — the independent oracle for
/// `find_changes_spawned_matches_the_serial_pass` (the spawned path chunks and
/// recombines; this one never chunks) and a spawner-free convenience for the crate's
/// unit tests (the tests below and the `signal_validation` suite). Production
/// detection goes through [`find_changes_spawned`].
#[cfg(test)]
#[must_use]
pub(super) fn find_changes(series: &[Series], context: &AnalysisContext) -> Vec<Finding> {
    let candidates = detect_all(series, context);
    finalize_findings(candidates, series, context)
}

/// Evaluates every series and returns the surviving findings, ranked
/// most-notable first — the analysis's detection entry point.
///
/// The [`AnalysisContext`] selects the per-series detector: history mode locates a
/// change-point and a drift and keeps the better-fitting one; branch mode compares
/// the branch's latest state against its base.
/// Surviving candidates pass a Benjamini–Hochberg false-discovery filter at
/// `config.fdr_q`. Findings are then filtered to the directions the mode reports and
/// ordered by descending relative move, then method, then a stable identity
/// tie-break.
///
/// Detection is per-series independent, so the series are split into one balanced
/// contiguous chunk per worker and each chunk runs on its own blocking task via
/// `spawner`, then recombined in series order; the result is identical to a plain
/// serial scan but spread across cores. A single available CPU (which is what Miri
/// reports) yields a single worker — one chunk, one task over every series. The
/// false-discovery filtering and final ranking that follow are cheap and stay on the
/// calling thread.
///
/// The series are taken as an `Arc<[Series]>` so each blocking task can share them
/// without copying. Production passes a Tokio-backed spawner; tests and Miri pass an
/// inline spawner that runs each task on the calling thread.
pub async fn find_changes_spawned(
    series: Arc<[Series]>,
    context: AnalysisContext,
    spawner: &Spawner,
) -> Vec<Finding> {
    let candidates = detect_all_spawned(&series, context, spawner).await;
    finalize_findings(candidates, &series, &context)
}

/// Applies the false-discovery filter, materialises the surviving findings' charting
/// points, and ranks them — the cross-series tail shared by the serial and
/// spawner-distributed detection passes.
///
/// `candidates` must be in series order (the order both detection paths produce) so
/// the Benjamini–Hochberg mask stays aligned.
fn finalize_findings(
    candidates: Vec<Candidate>,
    series: &[Series],
    context: &AnalysisContext,
) -> Vec<Finding> {
    let config = &context.config;

    // Control the false-discovery rate across every candidate: no engine is exact, so
    // each contributes its significance-test p-value to the shared pool.
    let candidate_p: Vec<f64> = candidates.iter().map(|candidate| candidate.bh_p).collect();
    let keep = stats::benjamini_hochberg(&candidate_p, config.fdr_q);
    let mut keep_iter = keep.into_iter();

    // `candidates` and `candidate_p` were built in the same order, so advancing
    // `keep_iter` for each candidate keeps the mask aligned. A surviving finding that
    // the mode keeps materialises its charting points here — a dropped candidate never
    // pays for them.
    let mut findings: Vec<Finding> = candidates
        .into_iter()
        .filter_map(|candidate| {
            if !keep_iter.next().unwrap_or(false) {
                return None;
            }
            let Candidate {
                mut finding,
                source_index,
                ..
            } = candidate;
            if !context.keeps(finding.direction) {
                return None;
            }
            let source = series
                .get(source_index)
                .expect("the source index was assigned from this series slice");
            finding.series = series_values(source);
            Some(finding)
        })
        .collect();

    findings.sort_by(|left, right| {
        right
            .relative_delta
            .abs()
            .total_cmp(&left.relative_delta.abs())
            .then_with(|| left.method.cmp(&right.method))
            .then_with(|| left.set.cmp(&right.set))
            .then_with(|| left.id.cmp(&right.id))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    findings
}

/// Detects every series sequentially, returning the raised candidates in series
/// order — the order [`finalize_findings`] relies on.
#[cfg(test)]
fn detect_all(series: &[Series], context: &AnalysisContext) -> Vec<Candidate> {
    detect_range(series, 0..series.len(), context)
}

/// Detects every series, distributed across workers: splits the series into one
/// balanced contiguous chunk per worker (the worker count is the available
/// parallelism capped at the series count), runs each chunk on its own blocking task
/// via `spawner`, and recombines the candidates in series order.
///
/// A single available CPU (which is what Miri reports) yields a single worker — one
/// chunk, one task covering every series — so the one-worker case is just the
/// degenerate partition rather than a separate serial branch. An empty slice yields no
/// workers and dispatches no task.
async fn detect_all_spawned(
    series: &Arc<[Series]>,
    context: AnalysisContext,
    spawner: &Spawner,
) -> Vec<Candidate> {
    let len = series.len();
    let workers = worker_count(len);

    // Spawn every chunk before awaiting any, so the blocking tasks run concurrently;
    // each owns a shared `Arc` handle to the series and a `Copy` of the context.
    let mut handles = Vec::with_capacity(workers);
    let mut start: usize = 0;
    for size in balanced_chunk_sizes(len, workers) {
        let end = start.saturating_add(size);
        let chunk = Arc::clone(series);
        handles.push(spawner.spawn_blocking(move || detect_range(&chunk, start..end, &context)));
        start = end;
    }

    // Concatenate in spawn order, which is series order, so the candidate sequence is
    // identical to the serial pass.
    let mut candidates = Vec::new();
    for handle in handles {
        candidates.extend(handle.await);
    }
    candidates
}

/// Detects the series in `range`, returning the raised candidates in index order.
fn detect_range(
    series: &[Series],
    range: Range<usize>,
    context: &AnalysisContext,
) -> Vec<Candidate> {
    range
        .filter_map(|index| {
            let one = series
                .get(index)
                .expect("the range is within the series slice");
            detect_one(index, one, context)
        })
        .collect()
}

/// Runs the mode-appropriate detector on the series at `index` and returns its
/// candidate finding, if one is raised.
///
/// This is pure and depends on no other series, which is what lets
/// [`find_changes_spawned`] evaluate the series across workers. History mode locates a
/// change-point and a drift and keeps the better-fitting one (optionally surfacing a
/// recovered spike); branch mode delegates to its dedicated detector.
/// `index` is the series' position in the analysed slice, stamped onto the candidate so
/// the finalize tail can materialise its charting points only if it survives filtering.
fn detect_one(index: usize, one: &Series, context: &AnalysisContext) -> Option<Candidate> {
    let config = &context.config;
    let candidate = match context.mode {
        AnalysisMode::History => {
            let active = active_view(one);
            // The point values are projected once here and shared by every history
            // detector, rather than each rebuilding the same `Vec<f64>`.
            let values: Vec<f64> = active.points.iter().map(|point| point.value).collect();
            let change = evaluate_change_point(&active, &values, config);
            let drift = evaluate_drift(&active, &values, config);
            let mut chosen = arbitrate(&values, change, drift);
            // A series with no active change may instead carry a recovered spike;
            // surface it only when inactive findings are requested.
            if chosen.is_none() && context.include_inactive {
                chosen = evaluate_resolved_spike(&active, &values, config);
            }
            chosen.map(|mut candidate| {
                stamp_history(&mut candidate.finding, one);
                candidate
            })
        }
        AnalysisMode::Branch => evaluate_branch(one, config, context.merge_base_index),
    };
    candidate.map(|mut candidate| {
        candidate.source_index = index;
        candidate
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "metric values are exact integer-derived counts"
    )]
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use std::sync::Arc;

    use jiff::Timestamp;
    use nonempty::nonempty;

    use super::*;
    use crate::analyze::{Blessing, SeriesPoint};
    use crate::model::{DiscriminantSet, MetricKind};

    /// Builds a Callgrind-style series carrying `values` in topological order, with
    /// no dispersion (no confidence interval).
    fn series_of(values: &[f64]) -> Series {
        series_with(values, MetricKind::InstructionCount, &[])
    }

    /// Builds a series whose benchmark id carries a distinct `name`, so a batch of
    /// series stays individually identifiable in the findings.
    fn named_series(name: &str, values: &[f64]) -> Series {
        let mut series = series_of(values);
        series.id = BenchmarkId::new(nonempty![name.to_owned(), "case".to_owned()]);
        series
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
                    object_ordinal: u32::try_from(index).unwrap(),
                    commit: Some(Arc::from(format!("commit{index}"))),
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
                machine_key: "synthetic".to_owned(),
            },
            id: BenchmarkId::new(nonempty!["group".to_owned(), "case".to_owned()]),
            kind,
            points,
            active_start: 0,
            blessing: None,
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
        findings.into_iter().next().unwrap()
    }

    /// The point values of a series, projected as the history detectors receive
    /// them (the production path shares one such projection across detectors).
    fn values_of(series: &Series) -> Vec<f64> {
        series.points.iter().map(|point| point.value).collect()
    }

    /// Builds a Callgrind-style history with a two-point plateau at `peak`
    /// bracketed by `shoulder`-length baseline and recovery regimes at `base`: a
    /// spike that rose and has since fully recovered.
    ///
    /// Every engine is now treated as noisy, so a recovered spike is only
    /// significant once each side is long enough for its Mann-Whitney gate; a
    /// two-point plateau needs eight-point shoulders to clear both rank tests.
    fn recovered_spike(base: f64, peak: f64, shoulder: usize) -> Series {
        let mut values = vec![base; shoulder];
        values.push(peak);
        values.push(peak);
        values.extend(std::iter::repeat_n(base, shoulder));
        series_of(&values)
    }

    /// Builds a minimal [`Candidate`] carrying only the fields [`arbitrate`]
    /// inspects (`method`, `split`, `line`); every other field is a placeholder.
    fn candidate(
        method: FindingMethod,
        split: Option<usize>,
        line: Option<(f64, f64)>,
    ) -> Candidate {
        Candidate {
            finding: Finding {
                set: DiscriminantSet {
                    engine: "callgrind".to_owned(),
                    target_triple: "t".to_owned(),
                    machine_key: "synthetic".to_owned(),
                },
                id: BenchmarkId::new(nonempty!["group".to_owned(), "case".to_owned()]),
                kind: MetricKind::InstructionCount,
                method,
                direction: Direction::Regression,
                baseline: 0.0,
                latest: 0.0,
                delta: 0.0,
                relative_delta: 0.0,
                confidence: 1.0,
                commit: None,
                flipped_at: None,
                active: true,
                active_from: 0,
                blessed_at: None,
                blessed_commit_time: None,
                series: Vec::new(),
            },
            source_index: 0,
            bh_p: 0.0,
            split,
            line,
        }
    }

    /// Runs the history-mode detector with default config, reporting both
    /// directions.
    fn changes(series: &[Series]) -> Vec<Finding> {
        find_changes(
            series,
            &AnalysisContext {
                mode: AnalysisMode::History,
                config: AnalysisConfig::default(),
                merge_base_index: None,
                include_improvements: true,
                include_inactive: false,
            },
        )
    }

    #[test]
    fn change_point_method_sorts_before_drift() {
        assert!(FindingMethod::ChangePoint < FindingMethod::Drift);
    }

    /// The spawner-distributed [`find_changes_spawned`] must produce exactly the same
    /// findings as the serial [`find_changes`] oracle. On a multi-core host this
    /// exercises the chunked spawn-and-recombine path across several chunks; under
    /// Miri, which reports one CPU, it exercises the single-worker chunk. Either way
    /// the synchronous spawner runs each chunk inline on the calling thread.
    #[cfg(feature = "private-test-util")]
    #[test]
    fn find_changes_spawned_matches_the_serial_pass() {
        use crate::testing::synchronous_spawner;

        // A batch large enough to span several worker chunks, mixing series that raise
        // a finding with flat ones that do not, so the spawned path must detect across
        // chunks and preserve series order when recombining.
        let step_up = [100.0, 100.0, 100.0, 100.0, 130.0, 130.0, 130.0, 130.0];
        let step_down = [130.0, 130.0, 130.0, 130.0, 100.0, 100.0, 100.0, 100.0];
        let flat = [100.0; 8];
        let shapes: [&[f64]; 3] = [&step_up, &step_down, &flat];
        let series: Vec<Series> = shapes
            .iter()
            .cycle()
            .take(24)
            .enumerate()
            .map(|(index, &values)| named_series(&format!("bench{index:02}"), values))
            .collect();

        let context = AnalysisContext {
            mode: AnalysisMode::History,
            config: AnalysisConfig::default(),
            merge_base_index: None,
            include_improvements: true,
            include_inactive: false,
        };

        let serial = find_changes(&series, &context);
        let spawned = futures::executor::block_on(find_changes_spawned(
            Arc::from(series.as_slice()),
            context,
            &synchronous_spawner(),
        ));

        // `Finding` is not `PartialEq`; its `Debug` projection is a faithful, total
        // rendering of every field, so equal debug output means equal findings.
        assert!(!serial.is_empty(), "the fixture must raise some findings");
        assert_eq!(format!("{serial:#?}"), format!("{spawned:#?}"));
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
        // L1 hits invert: more cheap L1 accesses improve, fewer regress.
        assert_eq!(
            direction_of(1.0, MetricKind::L1CacheHits),
            Direction::Improvement
        );
        assert_eq!(
            direction_of(-1.0, MetricKind::L1CacheHits),
            Direction::Regression
        );
        // The slower cache tiers are expensive: more LL/RAM hits regress, like any
        // lower-is-better metric.
        assert_eq!(
            direction_of(1.0, MetricKind::LastLevelCacheHits),
            Direction::Regression
        );
        assert_eq!(
            direction_of(1.0, MetricKind::RamHits),
            Direction::Regression
        );
        assert_eq!(
            direction_of(-1.0, MetricKind::RamHits),
            Direction::Improvement
        );
    }

    #[test]
    fn allocation_and_processor_time_metrics_are_lower_is_better() {
        // The `alloc_tracker` and `all_the_time` metrics all improve as they
        // shrink, so more bytes / allocations / processor time is a regression.
        for kind in [
            MetricKind::AllocatedBytes,
            MetricKind::AllocationCount,
            MetricKind::ProcessorTime,
        ] {
            assert!(!kind.higher_is_better());
            assert_eq!(direction_of(1.0, kind), Direction::Regression);
            assert_eq!(direction_of(-1.0, kind), Direction::Improvement);
        }
    }

    #[test]
    fn direction_of_classifies_a_zero_delta_as_an_improvement() {
        // The classification is total: a zero delta (never reached in practice) is
        // defined as an improvement for every polarity.
        assert_eq!(
            direction_of(0.0, MetricKind::InstructionCount),
            Direction::Improvement
        );
        assert_eq!(
            direction_of(0.0, MetricKind::L1CacheHits),
            Direction::Improvement
        );
        assert_eq!(
            direction_of(0.0, MetricKind::LastLevelCacheHits),
            Direction::Improvement
        );
    }

    #[test]
    fn regime_interval_takes_the_median_of_each_bound() {
        // Half-width 4 around [10,20,30] gives lows [6,16,26] and highs [14,24,34];
        // their medians are 16 and 24.
        let series = wall_series(&[10.0, 20.0, 30.0], 4.0);
        let refs: Vec<&SeriesPoint> = series.points.iter().collect();
        assert_eq!(regime_interval(&refs), Some((16.0, 24.0)));
    }

    #[test]
    fn regime_interval_without_dispersion_is_none() {
        let series = series_of(&[10.0, 20.0, 30.0]);
        let refs: Vec<&SeriesPoint> = series.points.iter().collect();
        assert_eq!(regime_interval(&refs), None);
    }

    #[test]
    fn intervals_disjoint_detects_separation_in_both_orders() {
        // The after regime sits wholly above the before regime.
        assert!(intervals_disjoint((10.0, 20.0), (30.0, 40.0)));
        // ...and wholly below it.
        assert!(intervals_disjoint((30.0, 40.0), (10.0, 20.0)));
        // Overlapping ranges are not disjoint.
        assert!(!intervals_disjoint((10.0, 20.0), (15.0, 25.0)));
        // Touching at a single boundary counts as overlapping, pinning the strict
        // `<`/`>` comparisons against `<=`/`>=` slips.
        assert!(!intervals_disjoint((10.0, 20.0), (20.0, 30.0)));
        assert!(!intervals_disjoint((20.0, 30.0), (10.0, 20.0)));
    }

    #[test]
    fn median_half_width_is_the_median_interval_half() {
        // A uniform half-width of 4 yields a median half-width of 4 (a `+`/`*` slip
        // in `(high - low) / 2` would instead give the point value or twice the
        // width).
        let series = wall_series(&[10.0, 20.0, 30.0], 4.0);
        assert_eq!(median_half_width(&series.points), Some(4.0));
    }

    #[test]
    fn median_half_width_without_dispersion_is_none() {
        let series = series_of(&[10.0, 20.0, 30.0]);
        assert_eq!(median_half_width(&series.points), None);
    }

    #[test]
    fn step_model_residual_is_the_median_absolute_deviation_per_regime() {
        // before [1,7] -> median 4 -> residuals 3,3; after [40,40] -> median 40 ->
        // residuals 0,0; the median of [3,3,0,0] is 1.5.
        assert_eq!(step_model_residual(&[1.0, 7.0, 40.0, 40.0], 2), Some(1.5));
    }

    #[test]
    fn step_model_residual_out_of_range_tau_is_none() {
        assert_eq!(step_model_residual(&[1.0, 2.0], 5), None);
    }

    #[test]
    fn line_model_residual_measures_distance_from_the_fitted_line() {
        // The line 10 + 2*i predicts [10,12,14,16]; the values deviate by [0,1,0,2],
        // whose median absolute residual is 0.5.
        assert_eq!(
            line_model_residual(&[10.0, 13.0, 14.0, 18.0], 2.0, 10.0),
            Some(0.5)
        );
    }

    #[test]
    fn sample_step_residual_is_the_median_absolute_deviation_across_samples() {
        // before [10,12,20] -> median 12 -> residuals 2,0,8; after [30,33,40] ->
        // median 33 -> residuals 3,0,7; the median of [2,0,8,3,0,7] is 2.5.
        assert_eq!(
            sample_step_residual(&[10.0, 12.0, 20.0], &[30.0, 33.0, 40.0]),
            Some(2.5)
        );
    }

    #[test]
    fn sample_step_residual_of_an_empty_sample_is_none() {
        assert_eq!(sample_step_residual(&[], &[1.0, 2.0]), None);
    }

    #[test]
    fn exceeds_residual_noise_requires_the_move_to_clear_the_scatter_band() {
        let config = AnalysisConfig::default();
        // A residual of 1.0 puts the band at 3x = 3.0. A move inside the band is
        // not clear of it, a move exactly at the band is still not (the comparison
        // is strict), a move above it is, and a missing residual trusts the move.
        assert!(!exceeds_residual_noise(1.0, Some(1.0), &config));
        assert!(!exceeds_residual_noise(3.0, Some(1.0), &config));
        assert!(exceeds_residual_noise(3.5, Some(1.0), &config));
        assert!(exceeds_residual_noise(0.0, None, &config));
    }

    #[test]
    fn arbitrate_breaks_a_residual_tie_in_favour_of_the_change_point() {
        // Both models fit a flat series perfectly (residual 0): the tie favours the
        // more specific change-point, so a `line < step` -> `line <= step` slip that
        // would pick the drift is caught.
        let values = [0.0, 0.0, 0.0, 0.0];
        let change = candidate(FindingMethod::ChangePoint, Some(2), None);
        let drift = candidate(FindingMethod::Drift, None, Some((0.0, 0.0)));
        let chosen = arbitrate(&values, Some(change), Some(drift)).unwrap();
        assert_eq!(chosen.finding.method, FindingMethod::ChangePoint);
    }

    #[test]
    fn arbitrate_prefers_the_better_fitting_line() {
        // A pure ramp: the line fits with zero residual while the two-regime split
        // leaves a positive residual, so the drift candidate wins.
        let values = [0.0, 1.0, 2.0, 3.0];
        let change = candidate(FindingMethod::ChangePoint, Some(2), None);
        let drift = candidate(FindingMethod::Drift, None, Some((1.0, 0.0)));
        let chosen = arbitrate(&values, Some(change), Some(drift)).unwrap();
        assert_eq!(chosen.finding.method, FindingMethod::Drift);
    }

    #[test]
    fn arbitrate_keeps_the_sole_candidate_that_fires() {
        let values = [0.0, 0.0, 5.0, 5.0];
        let change = candidate(FindingMethod::ChangePoint, Some(2), None);
        let only_change = arbitrate(&values, Some(change), None).unwrap();
        assert_eq!(only_change.finding.method, FindingMethod::ChangePoint);

        let drift = candidate(FindingMethod::Drift, None, Some((1.0, 0.0)));
        let only_drift = arbitrate(&values, None, Some(drift)).unwrap();
        assert_eq!(only_drift.finding.method, FindingMethod::Drift);

        assert!(arbitrate(&values, None, None).is_none());
    }

    #[test]
    fn change_point_accepts_a_minimal_before_regime() {
        // Pettitt splits at tau=2, so the before regime holds exactly `min_regime`
        // points: a `<=`/`==` slip on the before-regime bound would reject the step.
        // The after regime is padded so the rank test has enough points to confirm
        // the move (a 2-vs-5 clean step is Mann–Whitney significant).
        let finding = only(changes(&[series_of(&[
            100.0, 100.0, 130.0, 130.0, 130.0, 130.0, 130.0,
        ])]));
        assert_eq!(finding.method, FindingMethod::ChangePoint);
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.latest, 130.0);
    }

    #[test]
    fn change_point_accepts_a_minimal_after_regime() {
        // Pettitt splits at tau=5, so the after regime holds exactly `min_regime`
        // points: a `<=` slip on the after-regime bound would reject the step. The
        // before regime is padded so the 5-vs-2 clean step is rank-test significant.
        let finding = only(changes(&[series_of(&[
            100.0, 100.0, 100.0, 100.0, 100.0, 130.0, 130.0,
        ])]));
        assert_eq!(finding.method, FindingMethod::ChangePoint);
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.latest, 130.0);
    }

    #[test]
    fn change_point_rejects_a_single_point_regime() {
        // Pettitt splits at tau=1, leaving a one-point before regime (below
        // min_regime). The size guard rejects when *either* regime is too small, so
        // a `||`->`&&` slip would wrongly admit this lopsided split. A permissive
        // rank-test threshold isolates the guard: only the size check keeps it out.
        let config = AnalysisConfig {
            change_alpha: 0.5,
            ..AnalysisConfig::default()
        };
        let series = series_of(&[100.0, 130.0, 130.0, 130.0, 130.0, 130.0]);
        assert!(evaluate_change_point(&series, &values_of(&series), &config).is_none());
    }

    #[test]
    fn change_point_within_its_own_residual_scatter_is_suppressed() {
        // A rank-significant step (medians 102 -> 132, delta 30) whose regimes each
        // wobble by 2. Under the default residual multiple the move stands clear of
        // that scatter and is flagged; a deliberately high multiple pushes the noise
        // band above the move, so only the residual gate rejects it (every earlier
        // gate — persistence, Mann-Whitney, practical floor — still passes).
        let series = series_of(&[100.0, 104.0, 100.0, 104.0, 130.0, 134.0, 130.0, 134.0]);
        assert!(
            evaluate_change_point(&series, &values_of(&series), &AnalysisConfig::default())
                .is_some()
        );
        let config = AnalysisConfig {
            residual_noise_multiple: 20.0,
            ..AnalysisConfig::default()
        };
        assert!(evaluate_change_point(&series, &values_of(&series), &config).is_none());
    }

    #[test]
    fn sustained_step_is_flagged_as_a_change_point() {
        // A clean step from 100 to 130 with three points each side: a 3-vs-3 clean
        // step is Mann–Whitney significant.
        let series = series_of(&[100.0, 100.0, 100.0, 130.0, 130.0, 130.0]);
        let finding = only(changes(&[series]));
        assert_eq!(finding.method, FindingMethod::ChangePoint);
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.latest, 130.0);
        assert_eq!(finding.delta, 30.0);
        assert!((finding.relative_delta - 0.30).abs() <= 1e-9);
        // Confidence derives from the rank-test p-value (below 1) and the change is
        // attributed to the first commit of the after regime.
        assert!(finding.confidence > 0.9 && finding.confidence < 1.0);
        assert_eq!(finding.commit.as_deref(), Some("commit3"));
    }

    #[test]
    fn step_below_the_practical_floor_is_suppressed() {
        // A sub-3% move is treated as measurement noise even when it looks clean, so
        // the practical-magnitude floor suppresses it: 1000 -> 1001 is a 0.1% move.
        let series = series_of(&[1000.0, 1000.0, 1000.0, 1001.0, 1001.0, 1001.0]);
        assert!(changes(&[series]).is_empty());
    }

    #[test]
    fn step_at_the_practical_floor_is_flagged() {
        // The practical floor is a strict `<` rejection, so a step whose relative move
        // EQUALS the 3% floor is still reported: 1000 -> 1030 is exactly a 3% move.
        let series = series_of(&[1000.0, 1000.0, 1000.0, 1030.0, 1030.0, 1030.0]);
        let finding = only(changes(&[series]));
        assert_eq!(finding.method, FindingMethod::ChangePoint);
        assert_eq!(finding.delta, 30.0);
        assert!((finding.relative_delta - 0.03).abs() <= 1e-9);
        assert!(finding.confidence > 0.9 && finding.confidence < 1.0);
    }

    #[test]
    fn sub_practical_floor_improvement_is_also_suppressed() {
        // The floor applies in any direction: a sub-3% improvement is just as
        // meaningless as a sub-3% regression, so 1000 -> 999 (a 0.1% drop) raises
        // nothing.
        let series = series_of(&[1000.0, 1000.0, 1000.0, 999.0, 999.0, 999.0]);
        assert!(changes(&[series]).is_empty());
    }

    #[test]
    fn l1_hit_drop_is_a_regression() {
        // L1 hits are higher-is-better, so a drop in L1 hits is a regression
        // (the access shifted to a slower tier).
        let series = series_with(
            &[100.0, 100.0, 100.0, 70.0, 70.0, 70.0],
            MetricKind::L1CacheHits,
            &[],
        );
        let finding = only(changes(&[series]));
        assert!(finding.is_regression());
        assert_eq!(finding.delta, -30.0);
    }

    #[test]
    fn ram_hit_rise_is_a_regression() {
        // RAM hits are the expensive tier (lower-is-better), so a rise in RAM
        // hits is a regression.
        let series = series_with(
            &[70.0, 70.0, 70.0, 100.0, 100.0, 100.0],
            MetricKind::RamHits,
            &[],
        );
        let finding = only(changes(&[series]));
        assert!(finding.is_regression());
        assert_eq!(finding.delta, 30.0);
    }

    #[test]
    fn flat_series_never_flags() {
        let series = series_of(&[100.0, 100.0, 100.0, 100.0, 100.0, 100.0]);
        assert!(changes(&[series]).is_empty());
    }

    #[test]
    fn many_independent_series_are_detected_in_a_stable_order() {
        // `find_changes` runs the per-series detection sequentially. The work is
        // embarrassingly parallel — no series depends on another — so this guards
        // the properties any detection pass must preserve: every independent
        // finding is produced exactly once (the `filter_map`/`collect` neither
        // drops nor duplicates a candidate), flat series stay silent, and the
        // ranking is deterministic across runs (the order-preserving collect plus
        // the final sort fix the output). `find_changes_spawned_matches_the_serial_pass`
        // pins the spawner-distributed pass to this same output.
        let mut series = Vec::new();
        let mut stepped_ids = Vec::new();
        for raw in 0_i32..32 {
            // A clean step of a distinct magnitude: flags as a regression with its own
            // `|relative_delta|`, so the final ranking is a total order.
            let name = format!("step{raw:03}");
            let raised = 130.0 + f64::from(raw);
            series.push(named_series(
                &name,
                &[100.0, 100.0, 100.0, raised, raised, raised],
            ));
            stepped_ids.push(BenchmarkId::new(nonempty![name, "case".to_owned()]).qualified());
            // A flat companion never flags, so it must be absent from the output.
            series.push(named_series(
                &format!("flat{raw:03}"),
                &[100.0, 100.0, 100.0, 100.0, 100.0, 100.0],
            ));
        }

        let findings = changes(&series);

        // Exactly the stepped series flag, each exactly once.
        let mut flagged: Vec<String> = findings
            .iter()
            .map(|finding| finding.id.qualified())
            .collect();
        flagged.sort();
        stepped_ids.sort();
        assert_eq!(flagged, stepped_ids);

        // The ranking is byte-stable across repeated parallel passes.
        let ranking = |list: &[Finding]| -> Vec<(String, f64)> {
            list.iter()
                .map(|finding| (finding.id.qualified(), finding.relative_delta))
                .collect()
        };
        assert_eq!(ranking(&findings), ranking(&changes(&series)));
    }

    #[test]
    fn a_lone_blip_does_not_flag_a_change_point() {
        // A single spike returns to baseline: the after regime is one point, which
        // fails the persistence requirement.
        let series = series_of(&[100.0, 100.0, 100.0, 100.0, 100.0, 175.0]);
        assert!(changes(&[series]).is_empty());
    }

    #[test]
    fn step_in_the_final_point_fails_persistence() {
        // The shift only has one point after it (< min_regime), so it is rejected
        // even though the levels differ.
        let series = series_of(&[100.0, 100.0, 100.0, 100.0, 130.0]);
        assert!(changes(&[series]).is_empty());
    }

    #[test]
    fn noisy_jitter_around_a_stable_mean_is_not_flagged() {
        // Pure measurement jitter with no real shift must stay silent.
        let series = wall_series(&[100.0, 103.0, 98.0, 101.0, 99.0, 102.0, 97.0, 100.0], 5.0);
        assert!(changes(&[series]).is_empty());
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
        let finding = only(changes(&[series]));
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
        assert!(changes(&[series]).is_empty());
    }

    #[test]
    fn noisy_step_exactly_at_the_practical_floor_is_reported() {
        // The practical-magnitude floor is a strict `<` rejection, so a step whose
        // relative move EQUALS the floor must still be reported. Pin the floor to
        // exactly this series' relative delta (30/100) to exercise that boundary: a
        // `<=` slip would suppress an at-floor regression.
        let series = wall_series(
            &[
                98.0, 100.0, 102.0, 99.0, 101.0, 128.0, 130.0, 132.0, 129.0, 131.0,
            ],
            2.0,
        );
        let config = AnalysisConfig {
            practical_relative: 30.0_f64 / 100.0,
            ..AnalysisConfig::default()
        };
        let candidate = evaluate_change_point(&series, &values_of(&series), &config).unwrap();
        assert_eq!(candidate.finding.baseline, 100.0);
        assert_eq!(candidate.finding.latest, 130.0);
        assert_eq!(candidate.finding.relative_delta, config.practical_relative);
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
        assert!(changes(&[series]).is_empty());
    }

    #[test]
    fn monotonic_drift_is_flagged() {
        // A steady climb with no single dominant step surfaces as a drift finding.
        let series = series_of(&[100.0, 104.0, 108.0, 112.0, 116.0, 120.0]);
        let finding = only(changes(&[series]));
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
        // jump better than a line, so it is reported once, as a change-point. Four
        // distinct points each side make the rank test significant despite the
        // within-regime spread.
        let series = series_of(&[100.0, 101.0, 102.0, 103.0, 160.0, 161.0, 162.0, 163.0]);
        let findings = changes(&[series]);
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
        assert!(changes(&series).is_empty());
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
        let findings = changes(&series);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].method, FindingMethod::ChangePoint);
        assert_eq!(findings[0].baseline, 100.0);
        assert_eq!(findings[0].latest, 150.0);
    }

    #[test]
    fn find_changes_ranks_larger_relative_move_first() {
        let larger = series_of(&[100.0, 100.0, 100.0, 200.0, 200.0, 200.0]);
        let smaller = series_of(&[1000.0, 1000.0, 1000.0, 1050.0, 1050.0, 1050.0]);
        let findings = changes(&[smaller, larger]);
        assert_eq!(findings.len(), 2);
        assert!(findings[0].relative_delta.abs() > findings[1].relative_delta.abs());
        assert_eq!(findings[0].latest, 200.0);
        assert_eq!(findings[1].latest, 1050.0);
    }

    #[test]
    fn find_changes_retains_distinct_identities_ordered_by_move() {
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
        smaller.id = BenchmarkId::new(nonempty!["other".to_owned(), "case".to_owned()]);
        let findings = changes(&[smaller, larger]);
        assert_eq!(findings.len(), 2);
        assert!(findings[0].relative_delta.abs() > findings[1].relative_delta.abs());
        assert_eq!(findings[0].latest, 200.0);
        assert_eq!(findings[1].latest, 150.0);
    }

    // -- Branch mode ----------------------------------------------------------

    /// Builds a Callgrind-style series from explicit `(topo_index, value, dirty)`
    /// points, so branch splits can be modelled precisely. Points are taken in
    /// the given order (already topological).
    fn placed_series(points: &[(usize, f64, bool)]) -> Series {
        let points = points
            .iter()
            .map(|&(topo_index, value, dirty)| SeriesPoint {
                topo_index,
                dirty,
                object_ordinal: u32::try_from(topo_index).unwrap(),
                commit: Some(Arc::from(format!("commit{topo_index}"))),
                value,
                interval_low: None,
                interval_high: None,
            })
            .collect();
        Series {
            set: DiscriminantSet {
                engine: "callgrind".to_owned(),
                target_triple: "t".to_owned(),
                machine_key: "synthetic".to_owned(),
            },
            id: BenchmarkId::new(nonempty!["group".to_owned(), "case".to_owned()]),
            kind: MetricKind::InstructionCount,
            points,
            active_start: 0,
            blessing: None,
        }
    }

    /// Runs the branch-mode detector with default config and the given merge-base.
    fn branch_changes(series: &[Series], merge_base_index: Option<usize>) -> Vec<Finding> {
        find_changes(
            series,
            &AnalysisContext {
                mode: AnalysisMode::Branch,
                config: AnalysisConfig::default(),
                merge_base_index,
                include_improvements: false,
                include_inactive: false,
            },
        )
    }

    #[test]
    fn branch_mode_flags_a_late_regression_against_the_base() {
        // Base-side flat at 100 (topo 0..2), branch-side flat at 130 (topo 3..5).
        let series = placed_series(&[
            (0, 100.0, false),
            (1, 100.0, false),
            (2, 100.0, false),
            (3, 130.0, false),
            (4, 130.0, false),
            (5, 130.0, false),
        ]);
        let finding = only(branch_changes(&[series], Some(2)));
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.latest, 130.0);
        // A single sustained regime: no within-branch flip is reported.
        assert_eq!(finding.flipped_at, None);
    }

    #[test]
    fn branch_mode_reports_the_latest_state_after_an_intermediate_flip() {
        // The branch first improved (80) then regressed (130): we report the latest
        // state (worse than the 100 base) and point at where it flipped.
        let series = placed_series(&[
            (0, 100.0, false),
            (1, 100.0, false),
            (2, 100.0, false),
            (3, 80.0, false),
            (4, 80.0, false),
            (5, 80.0, false),
            (6, 130.0, false),
            (7, 130.0, false),
            (8, 130.0, false),
        ]);
        let finding = only(branch_changes(&[series], Some(2)));
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.latest, 130.0);
        // The flip is attributed to the first commit of the worsened regime.
        assert_eq!(finding.flipped_at.as_deref(), Some("commit6"));
    }

    #[test]
    fn branch_mode_is_silent_when_the_branch_matches_the_base() {
        let series = placed_series(&[
            (0, 100.0, false),
            (1, 100.0, false),
            (2, 100.0, false),
            (3, 100.0, false),
            (4, 100.0, false),
        ]);
        assert!(branch_changes(&[series], Some(2)).is_empty());
    }

    #[test]
    fn branch_mode_reports_an_improvement_over_the_base() {
        // Branch mode always reports both directions, regardless of
        // `include_improvements` (which only governs history mode).
        let series = placed_series(&[
            (0, 100.0, false),
            (1, 100.0, false),
            (2, 100.0, false),
            (3, 70.0, false),
            (4, 70.0, false),
            (5, 70.0, false),
        ]);
        let finding = only(branch_changes(&[series], Some(2)));
        assert_eq!(finding.direction, Direction::Improvement);
        assert!(!finding.is_regression());
        assert_eq!(finding.latest, 70.0);
    }

    #[test]
    fn branch_mode_is_silent_for_a_benchmark_new_on_the_branch() {
        // Every point is past the merge-base: no base-side baseline to compare to.
        let series = placed_series(&[(3, 130.0, false), (4, 130.0, false), (5, 130.0, false)]);
        assert!(branch_changes(&[series], Some(2)).is_empty());
    }

    #[test]
    fn branch_mode_admits_a_dirty_snapshot_at_the_merge_base_tip() {
        // The merge-base is the branch tip (topo 2); a dirty snapshot there is the
        // branch side, the clean runs at the same/earlier commits are the base. Three
        // dirty runs give the rank test enough points to confirm the regression.
        let series = placed_series(&[
            (0, 100.0, false),
            (1, 100.0, false),
            (2, 100.0, false),
            (2, 130.0, true),
            (2, 130.0, true),
            (2, 130.0, true),
        ]);
        let finding = only(branch_changes(&[series], Some(2)));
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.latest, 130.0);
    }

    // -- Blessing (re-baselining) and recovered spikes ------------------------

    /// Runs the history-mode detector reporting both directions *and* inactive
    /// findings, so a recovered spike surfaces.
    fn changes_with_inactive(series: &[Series]) -> Vec<Finding> {
        find_changes(
            series,
            &AnalysisContext {
                mode: AnalysisMode::History,
                config: AnalysisConfig::default(),
                merge_base_index: None,
                include_improvements: true,
                include_inactive: true,
            },
        )
    }

    #[test]
    fn history_does_not_reflag_a_blessed_step() {
        // The unblessed step from 100 to 130 is a change point.
        let series = series_of(&[100.0, 100.0, 100.0, 130.0, 130.0, 130.0]);
        assert_eq!(only(changes(std::slice::from_ref(&series))).latest, 130.0);

        // Blessing the post-step level re-baselines the series: the active window
        // begins at the first elevated point, leaving only the flat 130 regime to
        // judge, which no longer moves.
        let mut blessed = series;
        blessed.active_start = 3;
        blessed.blessing = Some(Blessing {
            commit: "abcdef0123456789".to_owned(),
            commit_time: Some(Timestamp::from_second(3).unwrap()),
        });
        assert!(changes(&[blessed]).is_empty());
    }

    #[test]
    fn history_stamps_blessing_provenance_on_an_active_finding() {
        // Pre-blessing history (100) is retained for charting but excluded from
        // detection; a real step *after* the blessed baseline (130 -> 160) still
        // flags, and the finding carries the blessing provenance and full series.
        let mut series = series_of(&[
            100.0, 100.0, 100.0, // pre-blessing prefix (charted, not judged)
            130.0, 130.0, 130.0, // blessed baseline
            160.0, 160.0, 160.0, // regression within the active window
        ]);
        series.active_start = 3;
        series.blessing = Some(Blessing {
            commit: "abcdef0123456789cafe".to_owned(),
            commit_time: Some(Timestamp::from_second(3).unwrap()),
        });
        let finding = only(changes(&[series]));
        assert!(finding.active);
        assert_eq!(finding.baseline, 130.0);
        assert_eq!(finding.latest, 160.0);
        // The full nine-point series is restored for charting...
        assert_eq!(finding.series.len(), 9);
        // ...and the active window plus blessing provenance are recorded.
        assert_eq!(finding.active_from, 3);
        assert_eq!(finding.blessed_at.as_deref(), Some("abcdef012345"));
        assert_eq!(
            finding.blessed_commit_time.as_deref(),
            Some("1970-01-01T00:00:03Z")
        );
        // An unblessed finding has a zero active window.
        let unblessed = only(changes(&[series_of(&[
            10.0, 10.0, 10.0, 10.0, 20.0, 20.0, 20.0, 20.0,
        ])]));
        assert_eq!(unblessed.active_from, 0);
    }

    #[test]
    fn resolved_spike_is_detected_and_marked_inactive() {
        // A two-point plateau (20) between baseline regimes (10) that has since
        // recovered. Every engine is now treated as noisy, so the elevated span
        // must clear a Mann-Whitney gate on both sides; the baseline and recovery
        // shoulders are long enough to make the rise and the fall significant.
        let spike = recovered_spike(10.0, 20.0, 8);
        let candidate =
            evaluate_resolved_spike(&spike, &values_of(&spike), &AnalysisConfig::default())
                .unwrap();
        assert!(!candidate.finding.active);
        assert_eq!(candidate.finding.baseline, 10.0);
        assert_eq!(candidate.finding.latest, 20.0);
        assert_eq!(candidate.finding.direction, Direction::Regression);
        // `commit` names where the median-plateau search brackets the rise,
        // `flipped_at` where it recovered.
        assert_eq!(candidate.finding.commit.as_deref(), Some("commit7"));
        assert_eq!(candidate.finding.flipped_at.as_deref(), Some("commit10"));
    }

    #[test]
    fn history_surfaces_a_resolved_spike_only_with_include_inactive() {
        // The spike rose and recovered, so no active change remains: the default
        // history pass is silent.
        let spike = recovered_spike(10.0, 20.0, 8);
        assert!(changes(std::slice::from_ref(&spike)).is_empty());

        // Requesting inactive findings surfaces it as a recovered spike that is no
        // longer reflected in the latest state.
        let finding = only(changes_with_inactive(&[spike]));
        assert!(!finding.active);
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.baseline, 10.0);
        assert_eq!(finding.latest, 20.0);
        assert!(finding.flipped_at.is_some());
    }

    // -- Noisy sample-comparison gates and statistical boundaries -------------

    /// Builds standalone `(value, confidence-half-width)` points for exercising the
    /// sample-comparison gates directly, independent of any series ordering.
    fn pts(specs: &[(f64, f64)]) -> Vec<SeriesPoint> {
        specs
            .iter()
            .enumerate()
            .map(|(index, &(value, half))| SeriesPoint {
                topo_index: index,
                dirty: false,
                object_ordinal: u32::try_from(index).unwrap(),
                commit: Some(Arc::from(format!("commit{index}"))),
                value,
                interval_low: Some(value - half),
                interval_high: Some(value + half),
            })
            .collect()
    }

    /// Compares the `before` and `after` samples on a wall-time (noisy) series,
    /// passing `floor` as the practical relative floor.
    fn compare(before: &[SeriesPoint], after: &[SeriesPoint], floor: f64) -> Option<Candidate> {
        let series = wall_series(&[100.0], 1.0);
        let before_refs: Vec<&SeriesPoint> = before.iter().collect();
        let after_refs: Vec<&SeriesPoint> = after.iter().collect();
        compare_samples(
            &series,
            &before_refs,
            &after_refs,
            &AnalysisConfig::default(),
            floor,
            None,
            None,
        )
    }

    #[test]
    fn compare_samples_at_the_practical_floor_is_not_suppressed() {
        // The relative move (0.03) is exactly the floor: the `relative < floor` gate
        // must be a strict `<` (a `<=`/`==` mutant would suppress it). The 1-vs-1
        // sample then clears the noise floor (delta 3 > 2 * 0.5).
        let before = pts(&[(100.0, 0.5)]);
        let after = pts(&[(103.0, 0.5)]);
        assert!(compare(&before, &after, 3.0 / 100.0).is_some());
    }

    #[test]
    fn compare_samples_prefers_the_small_sample_path_when_one_side_is_tiny() {
        // Five before-points, one after-point: the `len >= 2 && len >= 2` selects the
        // small-sample path, where delta 30 clears the floor and flags. An `||`
        // mutant would rank-test a 5-vs-1 sample, whose Mann-Whitney p stays above
        // alpha, flagging nothing.
        let before = pts(&[(100.0, 0.5); 5]);
        let after = pts(&[(130.0, 0.5)]);
        assert!(compare(&before, &after, 0.05).is_some());
    }

    #[test]
    fn compare_samples_suppresses_a_significant_move_with_overlapping_intervals() {
        // 5-vs-5 complete separation is Mann-Whitney significant, but the wide
        // confidence intervals overlap, so the change is rejected. Deleting the `!`
        // in the interval-overlap guard would let it through.
        let before = pts(&[(100.0, 2.0); 5]);
        let after = pts(&[(130.0, 60.0); 5]);
        assert!(compare(&before, &after, 0.05).is_none());
    }

    #[test]
    fn compare_samples_small_sample_clearing_the_noise_floor_has_real_confidence() {
        // 1-vs-1, delta 30 > 2 * 0.5: flagged. The small-sample path uses
        // change_alpha as its effective p, so the `1 - p` confidence is below 1 (a
        // mutated `1 + p` / `1 / p` would clamp to 1). An always-false floor guard or
        // a `>`->`<` floor comparison would instead suppress it.
        let before = pts(&[(100.0, 0.5)]);
        let after = pts(&[(130.0, 0.5)]);
        let candidate = compare(&before, &after, 0.05).unwrap();
        assert!(candidate.finding.confidence < 1.0);
    }

    #[test]
    fn compare_samples_small_sample_at_the_noise_floor_is_suppressed() {
        // 1-vs-1, delta 8 == 2 * 4: the strict `>` noise-floor gate rejects it. A
        // `>`->`>=`/`==`, the `*`->`+`/`/` arithmetic, or an always-true guard would
        // each flag it instead.
        let before = pts(&[(100.0, 4.0)]);
        let after = pts(&[(108.0, 4.0)]);
        assert!(compare(&before, &after, 0.05).is_none());
    }

    #[test]
    fn latest_regime_splits_a_three_point_branch_at_a_real_flip() {
        // Three points is the minimum to split; `branch.len() < 3` must be a strict
        // `<` (a `<=`/`==` mutant would keep the branch whole). The 100 -> 130 jump
        // is a real within-branch flip, so the latest regime is the two 130 points.
        let series = series_of(&[100.0, 130.0, 130.0]);
        let branch: Vec<&SeriesPoint> = series.points.iter().collect();
        let (points, flipped) = latest_regime(&branch, &AnalysisConfig::default());
        assert_eq!(points.len(), 2);
        assert!(flipped.is_some());
    }

    #[test]
    fn latest_regime_splits_when_the_within_branch_move_is_exactly_at_the_floor() {
        // The within-branch move (100 -> 103) is exactly the practical floor, so the
        // split stands: the floor gate must be a strict `<`, not a `<=`.
        let series = series_of(&[100.0, 103.0, 103.0]);
        let branch: Vec<&SeriesPoint> = series.points.iter().collect();
        let config = AnalysisConfig {
            practical_relative: 3.0 / 100.0,
            ..AnalysisConfig::default()
        };
        let (points, flipped) = latest_regime(&branch, &config);
        assert_eq!(points.len(), 2);
        assert!(flipped.is_some());
    }

    #[test]
    fn drift_at_the_practical_floor_is_flagged_with_real_confidence() {
        // A steady climb whose relative drift (0.20) is exactly the floor: the
        // floor gate must be a strict `<`, not a `<=`. Its confidence is 1 - p with
        // p > 0, so a mutated `1 + p` / `1 / p` would clamp to 1.
        let series = series_of(&[100.0, 104.0, 108.0, 112.0, 116.0, 120.0]);
        let config = AnalysisConfig {
            practical_relative: 20.0 / 100.0,
            ..AnalysisConfig::default()
        };
        let candidate = evaluate_drift(&series, &values_of(&series), &config).unwrap();
        assert_eq!(candidate.finding.method, FindingMethod::Drift);
        assert!(candidate.finding.confidence < 1.0);
    }

    #[test]
    fn noisy_drift_within_the_measurement_noise_floor_is_suppressed() {
        // The same climb on a noisy engine, but the endpoints (delta 20) do not
        // separate by more than twice the confidence half-width (12): jitter, not a
        // trend. The `2.0 * half_width` floor must be a product (a `+` mutant lowers
        // the floor to 14 and would flag it).
        let series = wall_series(&[100.0, 104.0, 108.0, 112.0, 116.0, 120.0], 12.0);
        assert!(evaluate_drift(&series, &values_of(&series), &AnalysisConfig::default()).is_none());
    }

    #[test]
    fn drift_within_its_own_residual_scatter_is_suppressed() {
        // A significant upward trend (100 -> 140) that scatters about its Theil-Sen
        // line. Under the default residual multiple the total move dwarfs that
        // scatter and is flagged as drift; a deliberately high multiple lifts the
        // noise band above the move, so only the residual gate rejects it (the
        // length, Mann-Kendall, and practical-floor gates still pass).
        let series = series_of(&[100.0, 110.0, 109.0, 120.0, 130.0, 140.0]);
        assert!(evaluate_drift(&series, &values_of(&series), &AnalysisConfig::default()).is_some());
        let config = AnalysisConfig {
            residual_noise_multiple: 1000.0,
            ..AnalysisConfig::default()
        };
        assert!(evaluate_drift(&series, &values_of(&series), &config).is_none());
    }

    #[test]
    fn drift_needs_at_least_the_minimum_points() {
        // The length gate is `n < drift_min_points`: a series one point short is
        // rejected outright, while a longer series is still evaluated (so a gate
        // mutated to reject the longer series instead is caught).
        let config = AnalysisConfig {
            practical_relative: 20.0 / 100.0,
            ..AnalysisConfig::default()
        };
        let short = series_of(&[100.0, 104.0, 108.0, 112.0, 116.0]);
        assert!(evaluate_drift(&short, &values_of(&short), &config).is_none());
        let long = series_of(&[100.0, 104.0, 108.0, 112.0, 116.0, 120.0, 124.0]);
        assert!(evaluate_drift(&long, &values_of(&long), &config).is_some());
    }

    #[test]
    fn analysis_mode_wire_names() {
        assert_eq!(AnalysisMode::History.as_str(), "history");
        assert_eq!(AnalysisMode::Branch.as_str(), "branch");
    }

    #[test]
    fn history_keeps_regressions_and_optionally_improvements() {
        let context = |include_improvements| AnalysisContext {
            mode: AnalysisMode::History,
            config: AnalysisConfig::default(),
            merge_base_index: None,
            include_improvements,
            include_inactive: false,
        };
        // Regressions are always reported; improvements only when opted in.
        assert!(context(false).keeps(Direction::Regression));
        assert!(!context(false).keeps(Direction::Improvement));
        assert!(context(true).keeps(Direction::Improvement));
    }

    #[test]
    fn reports_improvements_reflects_the_mode() {
        let context = |mode, include_improvements| AnalysisContext {
            mode,
            config: AnalysisConfig::default(),
            merge_base_index: None,
            include_improvements,
            include_inactive: false,
        };
        // History reports improvements only when opted in; branch always compares
        // both directions. Pinning both a true and a false case keeps the flag from
        // collapsing to a constant.
        assert!(!context(AnalysisMode::History, false).reports_improvements());
        assert!(context(AnalysisMode::History, true).reports_improvements());
        assert!(context(AnalysisMode::Branch, false).reports_improvements());
    }

    #[test]
    fn resolved_spike_reports_the_level_minus_baseline_deviation() {
        // The reported deviation is the plateau level (20) minus the baseline (10) --
        // the `level - baseline` difference, not a sum or a quotient.
        let series = recovered_spike(10.0, 20.0, 8);
        let candidate =
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .unwrap();
        assert_eq!(candidate.finding.delta, 10.0);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "the 200-point quadratic spike search is slow under Miri"
    )]
    fn resolved_spike_at_the_search_size_limit_is_flagged() {
        // A 200-point history (the inclusive search ceiling) with a recovered plateau
        // still analyses: the `n > RESOLVED_SPIKE_MAX_POINTS` guard must be a strict
        // `>`.
        let mut values = vec![10.0_f64; RESOLVED_SPIKE_MAX_POINTS];
        for value in values.get_mut(90..110).unwrap() {
            *value = 20.0;
        }
        let series = series_with(&values, MetricKind::InstructionCount, &[]);
        assert!(
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .is_some()
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "the 200-point quadratic spike search is slow under Miri"
    )]
    fn resolved_spike_beyond_the_search_size_limit_is_skipped() {
        // One point past the inclusive search ceiling is rejected outright: the
        // `n > RESOLVED_SPIKE_MAX_POINTS` guard caps the quadratic plateau search.
        let mut values = vec![10.0_f64; RESOLVED_SPIKE_MAX_POINTS + 1];
        for value in values.get_mut(90..110).unwrap() {
            *value = 20.0;
        }
        let series = series_with(&values, MetricKind::InstructionCount, &[]);
        assert!(
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .is_none()
        );
    }

    #[test]
    fn resolved_spike_below_the_practical_floor_is_not_a_spike() {
        // A plateau (1010) only 1% above baseline (1000) is below the 3% practical
        // floor. The reject gate is `deviation <= 0 || relative < floor`; an `&&`
        // mutant (needing BOTH) would wrongly surface it.
        let series = recovered_spike(1000.0, 1010.0, 8);
        assert!(
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .is_none()
        );
    }

    #[test]
    fn resolved_spike_exactly_at_the_practical_floor_is_a_spike() {
        // A plateau (103) exactly 3% above baseline (100) meets the floor; the
        // `relative < floor` gate must be a strict `<` (a `<=`/`==` mutant suppresses
        // it).
        let series = recovered_spike(100.0, 103.0, 8);
        let config = AnalysisConfig {
            practical_relative: 3.0 / 100.0,
            ..AnalysisConfig::default()
        };
        assert!(evaluate_resolved_spike(&series, &values_of(&series), &config).is_some());
    }

    #[test]
    fn noisy_resolved_spike_with_significant_rise_and_recovery_is_flagged() {
        // A noisy plateau (200) between long baseline/recovery regimes (100): both
        // the rise and the recovery are Mann-Whitney significant, so the recovered
        // spike is flagged, with confidence below 1.
        let values: Vec<f64> = std::iter::repeat_n(100.0_f64, 8)
            .chain([200.0, 200.0])
            .chain(std::iter::repeat_n(100.0, 8))
            .collect();
        let series = wall_series(&values, 1.0);
        let candidate =
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .unwrap();
        assert!(candidate.finding.confidence < 1.0);
    }

    #[test]
    fn noisy_resolved_spike_needs_both_gates_significant() {
        // The rise is Mann-Whitney significant, but the short recovery tail (two
        // points) is not: `rise_p >= alpha || recovery_p >= alpha` rejects it. An
        // `&&` mutant (needing both insignificant to reject) would wrongly flag it.
        let values: Vec<f64> = std::iter::repeat_n(100.0_f64, 8)
            .chain([200.0, 200.0])
            .chain([100.0, 100.0])
            .collect();
        let series = wall_series(&values, 1.0);
        assert!(
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .is_none()
        );
    }

    #[test]
    fn resolved_spike_within_its_own_residual_scatter_is_suppressed() {
        // A recovered plateau (200) far above a baseline (100) that itself wobbles by
        // 2. Under the default residual multiple the deviation stands clear and the
        // spike is flagged; a deliberately high multiple lifts the noise band above
        // the deviation, so only the residual gate rejects it (the recovery,
        // practical-floor, and both rank gates still pass).
        let values: Vec<f64> = [98.0, 102.0]
            .into_iter()
            .cycle()
            .take(8)
            .chain([198.0, 202.0])
            .chain([98.0, 102.0].into_iter().cycle().take(8))
            .collect();
        let series = series_of(&values);
        assert!(
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .is_some()
        );
        let config = AnalysisConfig {
            residual_noise_multiple: 60.0,
            ..AnalysisConfig::default()
        };
        assert!(evaluate_resolved_spike(&series, &values_of(&series), &config).is_none());
    }

    #[test]
    fn relative_delta_against_a_zero_baseline_is_a_full_magnitude_move() {
        // A move away from a (near-)zero baseline is proportionally unbounded, so its
        // sign is returned at full magnitude to rank as a major change.
        assert_eq!(relative_delta_of(5.0, 0.0), 1.0);
        assert_eq!(relative_delta_of(-5.0, 0.0), -1.0);
    }

    #[test]
    fn compare_samples_below_the_practical_floor_is_suppressed() {
        // A 1% relative move sits below the 5% floor on a noisy series, so the
        // comparison is dropped before any rank test.
        let before = pts(&[(100.0, 0.5)]);
        let after = pts(&[(101.0, 0.5)]);
        assert!(compare(&before, &after, 0.05).is_none());
    }

    #[test]
    fn compare_samples_suppresses_a_clear_move_the_rank_test_cannot_confirm() {
        // Two-versus-two complete separation clears the practical floor but its
        // Mann-Whitney p-value (0.33 for n1 = n2 = 2) stays above alpha, so the
        // change is not significant and is suppressed.
        let before = pts(&[(100.0, 0.5), (100.0, 0.5)]);
        let after = pts(&[(106.0, 0.5), (106.0, 0.5)]);
        assert!(compare(&before, &after, 0.05).is_none());
    }

    #[test]
    fn resolved_spike_shorter_than_three_regimes_is_not_a_spike() {
        // Five points cannot hold a baseline, an elevated middle, and a recovery of
        // at least `min_regime` (2) each, so the `n < min * 3` gate rejects it.
        let series = series_of(&[10.0, 10.0, 20.0, 20.0, 10.0]);
        assert!(
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .is_none()
        );
    }

    #[test]
    fn resolved_spike_exactly_three_regimes_long_is_a_spike() {
        // With min_regime 3 the shortest detectable spike holds exactly 3*3 = 9
        // points: a baseline, an elevated plateau, and a recovery of three each. The
        // `n < min * 3` gate must be a strict `<`; a `<=`/`==` slip would reject this
        // minimal spike, whose 3-vs-3 rise and recovery are both rank significant.
        let config = AnalysisConfig {
            min_regime: 3,
            ..AnalysisConfig::default()
        };
        let series = series_of(&[10.0, 10.0, 10.0, 100.0, 100.0, 100.0, 10.0, 10.0, 10.0]);
        assert!(evaluate_resolved_spike(&series, &values_of(&series), &config).is_some());
    }

    #[test]
    fn resolved_spike_with_a_still_elevated_tail_is_not_a_spike() {
        // The recovery tail (30) stays far above the baseline (10), so the series has
        // not recovered; an active change-point handles it instead.
        let series = series_of(&[10.0, 10.0, 20.0, 20.0, 30.0, 30.0]);
        assert!(
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .is_none()
        );
    }
}
