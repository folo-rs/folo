//! Default thresholds for the noise-aware analysis gates, gathered in one place.
//!
//! Every floor, significance level, and window the detectors apply has its default
//! value here as a named constant, so the whole gating policy can be read — and
//! tuned — in one file rather than hunted for across the detectors. The
//! [`AnalysisConfig`] default is assembled entirely from these; a test that needs a
//! different policy builds an [`AnalysisConfig`] explicitly instead of editing a
//! constant.
//!
//! [`AnalysisConfig`]: super::AnalysisConfig

/// Default `min_regime`: each side of a change must hold at least this many points
/// for the step to be trusted, so a one-off blip on the latest point cannot flag.
///
/// A regime this size has its level taken as a median over five points, which is a
/// stable estimator. Smaller regimes make the level an extreme order statistic of a
/// handful of recent measurements, which the residual gate — estimated from the
/// series as a whole — cannot then judge fairly.
pub(crate) const MIN_REGIME: usize = 5;

/// Default `min_series_points`: a series shorter than this is not evaluated at all,
/// and does not count toward the false-discovery family.
///
/// Two full regimes are the least a change-point can be built from, so this is
/// twice [`MIN_REGIME`]. Below it no split can satisfy the regime floor, so
/// evaluating the series can only produce noise.
pub(crate) const MIN_SERIES_POINTS: usize = 2 * MIN_REGIME;

/// Default `change_alpha`: the significance level a change-point's Mann–Whitney
/// rank test must clear.
pub(crate) const CHANGE_ALPHA: f64 = 0.05;

/// Default `fdr_q`: the Benjamini–Hochberg target false-discovery rate over a batch
/// of candidates.
pub(crate) const FDR_Q: f64 = 0.10;

/// Default `drift_min_points`: a series needs at least this many points before a
/// slow-drift finding is considered.
///
/// Matched to [`MIN_SERIES_POINTS`] so both history detectors demand the same
/// minimum evidence and a series is either evaluable by both or by neither.
pub(crate) const DRIFT_MIN_POINTS: usize = MIN_SERIES_POINTS;

/// Default `drift_alpha`: the significance level a drift's Mann–Kendall trend must
/// clear.
pub(crate) const DRIFT_ALPHA: f64 = 0.05;

/// Default `practical_relative`: a history move must shift the level by at least
/// this fraction (3%) to matter in practice, regardless of significance.
pub(crate) const PRACTICAL_RELATIVE: f64 = 0.03;

/// Default `practical_absolute_count`: a move on an instruction or branch count must
/// span at least this many units.
///
/// Code layout shifts these counts by a few units between builds of identical
/// source, so a handful of instructions carries no information about the code's
/// cost.
pub(crate) const PRACTICAL_ABSOLUTE_COUNT: f64 = 5.0;

/// Default `practical_absolute_time`: a timing move must span at least this many
/// nanoseconds.
///
/// This is a practical-significance judgement, not a resolution limit: the
/// regression slope a timing engine reports resolves far below a nanosecond, but a
/// move of under one nanosecond per iteration is not worth acting on whatever
/// percentage it works out to.
pub(crate) const PRACTICAL_ABSOLUTE_TIME: f64 = 1.0;

/// Default `practical_absolute_alloc`: an allocation move must span at least this
/// many bytes or allocations.
///
/// A fraction of a byte or of an allocation cannot happen, so one whole unit is the
/// smallest move worth reporting; the floor only rejects the sub-unit moves that
/// amortizing across a run's iterations can manufacture.
pub(crate) const PRACTICAL_ABSOLUTE_ALLOC: f64 = 1.0;

/// Default `scatter_floor_count`: the smallest scatter an instruction or branch
/// count can express, one whole count.
///
/// This is the metric's *quantum* rather than a significance threshold. It bounds
/// the base window's standard deviation from below in the branch-mode prediction
/// interval, so a window that happens to repeat one integer still yields a usable
/// standard error instead of a degenerate one. A stored count is a per-iteration
/// figure and so need not be a whole number, but no sample of it can establish a
/// scatter finer than the unit it counts.
pub(crate) const SCATTER_FLOOR_COUNT: f64 = 1.0;

/// Default `scatter_floor_time`: timing metrics have no quantum, so their scatter is
/// not bounded from below at all.
///
/// A stored time is a through-origin regression slope over the many iterations of a
/// run, which resolves far below a clock tick — a benchmark reported at a couple of
/// nanoseconds an iteration is measured that finely. Any positive floor here would
/// impose an absolute detection threshold in units of the standard error and cost
/// short benchmarks several-fold sensitivity, so what a timing move must clear is
/// left to [`PRACTICAL_ABSOLUTE_TIME`] alone. The price is that a base window with
/// exactly zero scatter yields no verdict, which is silence rather than a spurious
/// certainty.
pub(crate) const SCATTER_FLOOR_TIME: f64 = 0.0;

/// Default `scatter_floor_alloc`: the smallest scatter an allocation metric can
/// express, one whole byte or allocation.
///
/// The case it exists for is code that allocated nothing and now allocates: a base
/// window of zeroes has exactly zero scatter, and without a floor the standard error
/// collapses and the move cannot be judged at all. One unit is the finest scatter
/// the underlying count can distinguish.
pub(crate) const SCATTER_FLOOR_ALLOC: f64 = 1.0;

/// Default `compare_window`: how many recent base-side **commits** form the level a
/// branch tip is compared against.
///
/// The window is the sample the tip's prediction interval is built from, so its
/// size sets how small a move branch mode can resolve. The detectable move shrinks
/// steeply up to about this many commits and only marginally beyond, while a longer
/// window reaches further back into history that may no longer describe the current
/// base level.
///
/// It counts commits rather than stored runs because several runs can share one
/// commit and collapse to a single level before the comparison: a point-counted
/// window would hold a different number of levels depending on how many runs fell
/// inside it, and could shrink to a useless sample however long the history grew.
pub(crate) const COMPARE_WINDOW: usize = 16;

/// Default `branch_practical_relative`: a branch move must reach this fraction
/// (5%), raised above the history floor, to keep pull-request false positives down.
pub(crate) const BRANCH_PRACTICAL_RELATIVE: f64 = 0.05;

/// Default `branch_noise_multiple`: multiple of the per-measurement noise floor a
/// branch move must exceed where the engine reports per-point confidence intervals.
///
/// This vetoes a move that the engine's own dispersion cannot distinguish from
/// noise, independently of how the tip compares against the base level.
pub(crate) const BRANCH_NOISE_MULTIPLE: f64 = 2.0;

/// Default `residual_noise_multiple`: multiple of a series' own between-commit
/// residual scatter a move must exceed to clear the primary noise gate.
pub(crate) const RESIDUAL_NOISE_MULTIPLE: f64 = 3.0;

/// Default `min_regime_separation`: the Mann–Whitney probability-of-superiority a
/// level shift's two regimes must reach to be trusted.
pub(crate) const MIN_REGIME_SEPARATION: f64 = 0.85;

/// Largest interior window size resolved-spike search will scan; longer histories
/// skip the quadratic search rather than stall.
pub(crate) const RESOLVED_SPIKE_MAX_POINTS: usize = 200;
