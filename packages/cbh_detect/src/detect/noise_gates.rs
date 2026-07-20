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
pub(crate) const MIN_REGIME: usize = 2;

/// Default `change_alpha`: the significance level a change-point's Mann–Whitney
/// rank test must clear.
pub(crate) const CHANGE_ALPHA: f64 = 0.05;

/// Default `fdr_q`: the Benjamini–Hochberg target false-discovery rate over a batch
/// of candidates.
pub(crate) const FDR_Q: f64 = 0.10;

/// Default `drift_min_points`: a series needs at least this many points before a
/// slow-drift finding is considered.
pub(crate) const DRIFT_MIN_POINTS: usize = 6;

/// Default `drift_alpha`: the significance level a drift's Mann–Kendall trend must
/// clear.
pub(crate) const DRIFT_ALPHA: f64 = 0.05;

/// Default `practical_relative`: a history move must shift the level by at least
/// this fraction (3%) to matter in practice, regardless of significance.
pub(crate) const PRACTICAL_RELATIVE: f64 = 0.03;

/// Default `practical_absolute`: a move on a quantized metric must span at least
/// this many of its integer units, so a single-quantum run-to-run wobble on a tiny
/// baseline never flags as a large percentage regression.
pub(crate) const PRACTICAL_ABSOLUTE: f64 = 5.0;

/// Default `compare_window`: how many recent base-side points form the level a
/// branch tip is compared against.
pub(crate) const COMPARE_WINDOW: usize = 8;

/// Default `branch_practical_relative`: a branch move must reach this fraction
/// (5%), raised above the history floor, to keep pull-request false positives down.
pub(crate) const BRANCH_PRACTICAL_RELATIVE: f64 = 0.05;

/// Default `branch_noise_multiple`: multiple of the per-measurement noise floor a
/// branch move with too few points to rank-test must exceed.
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
