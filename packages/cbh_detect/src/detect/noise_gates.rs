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
/// and does not count toward the false-discovery family.
///
/// Two full regimes are the least a change-point can be built from. Below this floor
/// no split can satisfy [`MIN_REGIME`] on both sides, so evaluating the series can
/// only produce noise.
pub const MIN_SERIES_POINTS: usize = 2 * MIN_REGIME;

/// The most points any one series is analyzed over: older points beyond this count are
/// dropped before analysis, keeping only the most recent window.
///
/// The tool is designed for series of dozens to a few hundred points; see
/// `docs/DESIGN.md`, "Supported series length". The cap bounds how far the search for a
/// step looks back and therefore bounds the work done by runtime permutation
/// calibration. The value sits well above any realistic history, so the cap changes
/// nothing in ordinary use; it prevents an unusually long series from consuming
/// unbounded analysis time.
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
/// conservatively account for choosing between them. Ref: `docs/DESIGN.md`,
/// "Choosing between a step and a drift".
pub const HISTORY_DETECTOR_COUNT: usize = 2;

/// Conditional permutations added for each judged series in the false-discovery
/// family, up to [`MAX_CHANGE_PERMUTATIONS`].
///
/// At the hardest rank-1 Benjamini-Hochberg boundary, the pre-arbitration
/// change-point p-value must be below `TARGET_FALSE_DISCOVERY_RATE /
/// (HISTORY_DETECTOR_COUNT * family_size)`. This budget gives the null sample
/// enough expected observations at that boundary to estimate it stably until the
/// absolute cap takes over. Ref: `docs/DESIGN.md`, "Selection adjustment".
pub const PERMUTATIONS_PER_JUDGED_SERIES: usize = 600;

/// Absolute conditional-permutation budget for one selected change point.
///
/// The cap keeps work per series independent of an arbitrarily large analysis
/// family. Its plus-one floor can still resolve the rank-1 Benjamini-Hochberg
/// boundary at the stress harness's large-family scale after the analytic/
/// permutation weighting and two-detector correction. With the current policy,
/// zero-exceedance permutation resolves rank one through 22,500 judged series;
/// above that, a candidate needs help from the analytic component. Clear changes
/// normally use that component instead, while null candidates stop sequentially.
pub const MAX_CHANGE_PERMUTATIONS: usize = 500_000;

/// Extreme shuffled procedures needed before sequential calibration may stop.
///
/// This retains the former calibration target at the hardest uncapped family
/// boundary: enough observations to estimate a near-threshold tail without making
/// null candidates consume the entire maximum budget.
pub const CHANGE_PERMUTATION_EXCEEDANCES: usize = 30;

/// Bonferroni weight allocated to the analytic selection-adjustment component.
///
/// The remaining weight goes to conditional permutation. Keeping most weight on
/// permutation limits the ordinary power cost, while the analytic component needs
/// only a small allocation to certify clear steps from an exponentially small
/// finite-population tail bound.
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

/// Default `scatter_floor_count`: the smallest scatter an instruction or branch
/// count can express.
///
/// This is the metric's *quantum* rather than a significance threshold. It bounds
/// the base window's standard deviation from below in the branch-mode prediction
/// interval, so a window that happens to repeat one integer still yields a usable
/// standard error instead of a degenerate one. A stored count is a per-iteration
/// figure and so need not be a whole number, but no sample of it can establish a
/// scatter finer than the unit it counts.
pub const SCATTER_FLOOR_COUNT: f64 = 1.0;

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
pub const SCATTER_FLOOR_TIME: f64 = 0.0;

/// Default `scatter_floor_alloc`: the smallest scatter an allocation metric can
/// express.
///
/// The case it exists for is code that allocated nothing and now allocates: a base
/// window of zeroes has exactly zero scatter, and without a floor the standard error
/// collapses and the move cannot be judged at all. The finest scatter the underlying
/// count can distinguish is the unit it counts.
pub const SCATTER_FLOOR_ALLOC: f64 = 1.0;

/// Default `compare_window`: how many recent base-side **commits** branch mode
/// inspects.
///
/// The window is the evidence branch mode inspects to understand the current base
/// level. When it contains a genuine level shift, branch mode narrows the prediction
/// interval to the trailing regime after that shift; otherwise the whole window is
/// the comparison sample. Its size sets how small a move branch mode can resolve and
/// how far back it can look for a current-regime boundary. The detectable move
/// shrinks steeply up to about this many commits and only marginally beyond, while a
/// longer window reaches further back into history that may no longer describe the
/// current base level.
///
/// It counts commits rather than stored runs because several runs can share one
/// commit and collapse to a single level before the comparison: a point-counted
/// window would hold a different number of levels depending on how many runs fell
/// inside it, and could shrink to a useless sample however long the history grew.
pub const COMPARE_WINDOW: usize = 16;

/// Default `branch_practical_relative`: a branch move must reach this fraction,
/// raised above the history floor, to keep pull-request false positives down.
pub const BRANCH_PRACTICAL_RELATIVE: f64 = 0.05;

/// Default `excursion_relative_magnitude`: how far a base-window level must stand from
/// its surroundings before branch mode treats it as a measurement excursion rather than
/// as evidence.
///
/// Set where measurement wobble stops and runner interference starts, measured on this
/// project's own stored history: across the wall-time series, isolated levels straying up
/// to roughly a quarter from their surroundings stray high and low about equally often —
/// the two-sided signature of ordinary measurement scatter — while beyond this threshold
/// they are almost exclusively *slow*, which is the one-sided signature of a runner
/// losing time to something else. Setting it lower would discard ordinary scatter, which
/// is the sample's own dispersion and the very thing the comparison is judged against.
///
/// It also sits far above [`BRANCH_PRACTICAL_RELATIVE`], so no level a branch move could
/// be reported against is anywhere near being treated as an excursion.
pub const EXCURSION_RELATIVE_MAGNITUDE: f64 = 0.30;

/// Default `excursion_neighbour_agreement`: how closely the levels on either side of a
/// candidate excursion must agree before it can be discarded.
///
/// Held equal to [`BRANCH_PRACTICAL_RELATIVE`] because that is the smallest move branch
/// mode would report: surroundings that agree to within it describe one level as far as
/// any verdict is concerned, and surroundings that do not are a level shift, whose levels
/// must be kept whatever their magnitude.
pub const EXCURSION_NEIGHBOUR_AGREEMENT: f64 = BRANCH_PRACTICAL_RELATIVE;

/// Default `excursion_neighbours`: how many levels on each side of a candidate form the
/// surroundings it is judged against.
///
/// Enough that one *further* excursion adjacent to the candidate cannot by itself decide
/// what a side says, since these arrive in clusters, and few enough that the surroundings
/// stay local to the candidate rather than reaching across a level shift the window may
/// legitimately contain.
///
/// A candidate without this many levels on *both* sides is never discarded. Judging one
/// against a shorter side would let a single adjacent level speak for a whole side, which
/// is precisely the case this count exists to outvote.
pub const EXCURSION_NEIGHBOURS: usize = 3;

/// Default `excursion_max_removals`: how many excursions a window may contain before it
/// is left alone entirely.
///
/// One. A window offering a second is not a clean window with a bad reading in it: two
/// separated levels agreeing on a value their surroundings do not is the signature of a
/// benchmark that visits more than one level, and how often it does so is exactly what the
/// comparison measures the context run against. Discarding a recurring level would leave a
/// spuriously tight window in which the benchmark's own ordinary values read as large,
/// certain regressions — the failure this whole rule exists to avoid causing.
///
/// Runner interference is rare enough per window that requiring uniqueness costs almost
/// nothing: the stored history puts a second excursion inside the same window at well
/// under a percent of comparisons.
pub const EXCURSION_MAX_REMOVALS: usize = 1;

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
