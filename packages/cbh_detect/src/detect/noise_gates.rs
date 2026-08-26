//! Thresholds for the noise-aware analysis gates, gathered in one place.
//!
//! Every floor, significance level, and window the detectors apply is a named
//! constant here, so the whole gating policy can be read — and tuned — in one file
//! rather than hunted for across the detectors. The detectors read these constants
//! directly; the policy is not configurable, so there is no per-run override and a
//! test exercises exactly the policy the tool ships. To prove *which* gate governs a
//! series, tests read the recorded [`GateLog`] under this fixed policy rather than
//! relaxing a threshold.
//!
//! [`GateLog`]: super::GateLog

/// Default `min_regime`: each side of a change must hold at least this many points
/// for the step to be trusted, so a one-off blip on the latest point cannot flag.
///
/// A regime this size takes its level as a median over a handful of points, which is
/// a stable estimator. Smaller regimes make the level an extreme order statistic of a
/// few recent measurements, which the residual gate — estimated from the series as a
/// whole — cannot then judge fairly.
pub const MIN_REGIME: usize = 5;

/// Default `min_series_points`: a series shorter than this is not evaluated at all,
/// and in history mode does not count toward the false-discovery family.
///
/// Two full regimes are the least a change-point can be built from. Below this floor
/// no split can satisfy [`MIN_REGIME`] on both sides, so evaluating the series can
/// only produce noise.
///
/// A series that meets this floor is judged, but a lone change-point at this length
/// cannot clear the group-wide correction in a large family. Ref:
/// `../../cargo-bench-history/docs/DESIGN.md`, "Multiple-comparison discipline".
pub const MIN_SERIES_POINTS: usize = 2 * MIN_REGIME;

/// The most points any one series is analyzed over: older points beyond this count are
/// dropped before analysis, keeping only the most recent window.
///
/// The tool is designed for series of dozens to a few hundred points; see
/// `../../cargo-bench-history/docs/DESIGN.md`, "Supported series length". The cap bounds how
/// far the search for a step looks back and therefore bounds exact-group calibration
/// work. The value sits well above any realistic history, so the cap changes nothing
/// in ordinary use; it prevents an unusually long series from consuming unbounded
/// analysis time.
pub const MAX_SERIES_POINTS: usize = 1000;

/// The largest selection-adjusted chance level a change-point's Mann–Whitney rank test
/// may report and still be treated as a finding.
///
/// The conventional 5% significance level: a finding this gate admits would arise by
/// chance at most about one time in twenty on a series with no real step.
pub const MAX_CHANGE_CHANCE_LEVEL: f64 = 0.05;

/// The Benjamini–Hochberg target false-discovery rate over a batch of candidates: at most
/// this fraction of the reported findings is expected to be a false alarm.
///
/// Set looser than [`MAX_CHANGE_CHANCE_LEVEL`] because it governs the expected *proportion*
/// of false findings across a whole batch rather than the chance of any single false
/// finding, and a 10% false-discovery rate keeps the reported set trustworthy without
/// discarding genuine regressions that a stricter rate would cull.
pub const TARGET_FALSE_DISCOVERY_RATE: f64 = 0.10;

/// The history detectors run as a pair and the better-fitting model is reported.
///
/// Their individual p-values are multiplied by this count before gating to
/// conservatively account for choosing between them. Ref:
/// `../../cargo-bench-history/docs/DESIGN.md`, "Multiple-comparison discipline".
pub const HISTORY_DETECTOR_COUNT: usize = 2;

/// Exact permutation-group order budget added for each judged series in the
/// false-discovery family, up to [`MAX_CHANGE_PERMUTATION_ORDER`].
///
/// At the hardest rank-1 Benjamini-Hochberg boundary, the pre-arbitration
/// change-point p-value must be below `TARGET_FALSE_DISCOVERY_RATE /
/// (HISTORY_DETECTOR_COUNT * family_size)`. Scaling the permitted exact group
/// order with the family preserves useful resolution until the absolute cap takes
/// over. Ref: `../../cargo-bench-history/docs/DESIGN.md`,
/// "Multiple-comparison discipline".
pub const PERMUTATION_ORDER_PER_JUDGED_SERIES: usize = 600;

/// Minimum exact permutation-orbit order budget for any calibrated change point.
///
/// This admits the balanced all-position subgroup used by short histories,
/// preserving useful conditional resolution even in a one-series family. It also
/// covers every distinct ordering of the shortest tied steps.
pub const MIN_CHANGE_PERMUTATION_ORDER: usize = 259_200;

/// Absolute exact permutation-group order budget for one selected change point.
///
/// The cap keeps work per series independent of an arbitrarily large analysis
/// family. At the maximum supported series length, the largest realizable group
/// can still resolve the rank-1 Benjamini-Hochberg boundary at the stress harness's
/// large-family scale after analytic/permutation weighting and two-detector
/// correction. With the current policy, its smallest nonzero result resolves rank
/// one through 22,394 judged series. Shorter histories may realize a smaller group;
/// clear changes normally use the analytic component instead.
pub const MAX_CHANGE_PERMUTATION_ORDER: usize = 500_000;

/// Bonferroni weight allocated to the analytic selection-adjustment component.
///
/// The remaining weight goes to conditional permutation. Keeping most weight on
/// permutation limits the power cost of conservative finite-population bounds, while
/// the analytic component can still certify clear steps.
pub const CHANGE_ANALYTIC_WEIGHT: f64 = 0.10;

/// Default `drift_min_points`: a series needs at least this many points before a
/// slow-drift finding is considered.
///
/// Matched to [`MIN_SERIES_POINTS`] so both history detectors demand the same
/// minimum evidence and a series is either evaluable by both or by neither.
pub const DRIFT_MIN_POINTS: usize = MIN_SERIES_POINTS;

/// The largest selection-adjusted chance level a drift's Mann–Kendall trend test may
/// report and still be treated as a finding.
///
/// Held equal to [`MAX_CHANGE_CHANCE_LEVEL`] so both history detectors admit a finding at
/// the same conventional 5% significance level.
pub const MAX_DRIFT_CHANCE_LEVEL: f64 = 0.05;

/// Default `practical_relative`: a history move must shift the level by at least
/// this fraction to matter in practice, regardless of significance.
pub const PRACTICAL_RELATIVE: f64 = 0.03;

/// Default `practical_absolute_count`: a move on an instruction or branch count must
/// span at least this many units.
///
/// Code layout shifts these counts by a few units between builds of identical
/// source, so a handful of instructions carries no information about the code's
/// cost.
pub const PRACTICAL_ABSOLUTE_COUNT: f64 = 5.0;

/// Default `practical_absolute_time`: a timing move must span at least this many
/// nanoseconds.
///
/// This is a practical-significance judgement, not a resolution limit: the
/// regression slope a timing engine reports resolves far below a nanosecond, but a
/// move of under one nanosecond per iteration is not worth acting on whatever
/// percentage it works out to.
pub const PRACTICAL_ABSOLUTE_TIME: f64 = 1.0;

/// Default `practical_absolute_alloc`: an allocation move must span at least this
/// many bytes or allocations.
///
/// A fraction of a byte or of an allocation cannot happen, so one whole unit is the
/// smallest move worth reporting; the floor only rejects the sub-unit moves that
/// amortizing across a run's iterations can manufacture.
pub const PRACTICAL_ABSOLUTE_ALLOC: f64 = 1.0;

/// Maximum recent base-side **commits** branch mode inspects.
///
/// The cap supplies enough history for regime selection and useful historical
/// comparison ranks while bounding the cost of evaluating every comparable commit
/// as the candidate in turn. Shorter histories remain fully supported.
///
/// It counts commits rather than stored runs because several runs can share one
/// commit and collapse to a single level before the comparison: a point-counted
/// window would hold a different number of levels depending on how many runs fell
/// inside it, and could shrink to a useless sample however long the history grew.
///
/// Ref: `../../cargo-bench-history/docs/DESIGN.md`, "Branch analysis".
pub const MAX_BRANCH_BASE_COMMITS: usize = 128;

/// Base commits required before branch mode attempts to separate historical regimes.
///
/// Alternating selector and reference lanes leave the selector lane with half of the
/// observations. Requiring this many base commits therefore leaves enough selector
/// observations for two minimum-sized regimes.
pub const MIN_BRANCH_REGIME_SELECTION_COMMITS: usize = 2 * MIN_SERIES_POINTS;

/// Consecutive selector observations required before a short trailing group can make the
/// current base regime unresolved.
///
/// A single unusual base observation is evidence about the observed range, not evidence of a
/// new regime. Requiring a repeated level distinguishes an emerging step from an isolated
/// extreme while still withholding judgment before a complete [`MIN_REGIME`] is established.
pub const MIN_UNRESOLVED_BRANCH_REGIME_POINTS: usize = 2;

/// Comparable base commits required for a report-wide historical comparison.
///
/// This matches the minimum supported regime size, preserving a useful leave-one-out
/// comparison without manufacturing additional observations.
pub const MIN_BRANCH_COMPARISON_COMMITS: usize = MIN_REGIME;

/// Overall false-boundary budget for recursive branch regime selection.
///
/// The budget is shared conservatively across every possible recursive search, so
/// repeatedly looking for a newer supported step cannot amplify weak evidence.
pub const MAX_BRANCH_REGIME_CHANCE_LEVEL: f64 = MAX_CHANGE_CHANCE_LEVEL;

/// Default `branch_practical_relative`: a branch move must reach this fraction,
/// raised above the history floor, to keep pull-request false positives down.
pub const BRANCH_PRACTICAL_RELATIVE: f64 = 0.05;

/// Default `branch_noise_multiple`: multiple of the per-measurement noise floor a
/// branch move must exceed where the engine reports per-point confidence intervals.
///
/// This vetoes a move that the engine's own dispersion cannot distinguish from
/// noise, independently of how the tip compares against the base level.
pub const BRANCH_NOISE_MULTIPLE: f64 = 2.0;

/// Default `drift_noise_multiple`: multiple of the per-measurement noise floor a
/// drift's total movement must exceed where the engine reports per-point confidence
/// intervals.
///
/// Serves the same role for a trend that [`BRANCH_NOISE_MULTIPLE`] serves for a
/// branch move: it vetoes movement the engine's own dispersion cannot distinguish
/// from noise. The two are held equal because the question each asks is the same one
/// — whether the endpoints separate by more than the per-point dispersion — and neither
/// detector has evidence the other lacks to justify a different standard.
pub const DRIFT_NOISE_MULTIPLE: f64 = 2.0;

/// Default `residual_noise_multiple`: multiple of a series' own between-commit
/// residual scatter a move must exceed to clear the primary noise gate.
pub const RESIDUAL_NOISE_MULTIPLE: f64 = 3.0;

/// Default `min_regime_separation`: the Mann–Whitney probability-of-superiority a
/// level shift's two regimes must reach to be trusted.
pub const MIN_REGIME_SEPARATION: f64 = 0.85;

/// Default `min_base_split_separation`: the probability of superiority a base-window
/// split must reach before branch mode accepts it as a regime boundary and discards
/// the levels before it.
///
/// Held above [`MIN_REGIME_SEPARATION`] because the two decisions carry asymmetric
/// costs. Reporting a move makes a claim that a human then checks. Accepting a
/// boundary *discards evidence*: the comparison sample shrinks to the trailing
/// regime and the scatter estimate is rebuilt from it alone, so a wrong boundary can
/// collapse a noisy window's scatter to near zero and make any subsequent tip
/// read as certain. A boundary that throws data away must therefore be unambiguous,
/// which is a higher standard than merely reporting a move.
///
/// The statistic is coarse at these sample sizes — the smallest regimes hold
/// [`MIN_REGIME`] levels each, so the superiority of a smallest-regime split moves in
/// steps of one twenty-fifth — so this floor is read as "essentially no crossing pair
/// may contradict the boundary" rather than as a precise probability: at the smallest
/// regimes it admits one contradicting pair in twenty-five and no more, and a wider
/// split tolerates correspondingly few. A stationary series that oscillates between
/// two levels leaves several contradicting pairs in every candidate split and is
/// rejected on that basis.
pub const MIN_BASE_SPLIT_SEPARATION: f64 = 0.95;
