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
//! unchanged machine* often reports the same count every time — its simulated
//! counter barely notices the machine conditions that move a timing. What is not
//! fixed is everything feeding it across the commits we compare: a different OS
//! or CPU-microcode patch level, a different compiler patch release, the
//! compiler's own run-to-run nondeterministic code-generation choices (inlining,
//! ordering, layout) even at the same version, and Criterion scheduling a
//! different iteration count when background load differs (which shifts how
//! warmup and buffer-resize costs are amortized). Any of these perturbs the
//! measured count without the code under test changing, so no metric can be
//! assumed reproducible commit to commit. Every series is therefore judged
//! noise-aware:
//!
//! * A Pettitt change-point *locates* a candidate split (its analytic p-value is
//!   too conservative on short series to gate significance); both regimes must
//!   hold at least `min_regime` points (persistence), and a series shorter than
//!   `min_series_points` is not judged at all.
//! * A Mann–Whitney rank test must then confirm the two regimes differ, the move
//!   must clear a practical-magnitude floor, and it must exceed the series' own
//!   between-commit residual scatter (the primary, series-intrinsic noise gate).
//!   The practical-magnitude floor is relative (a minimum percentage) *and*
//!   absolute (a minimum span of the metric's own units), so a move too small to
//!   act on — a few instructions of build layout, a fraction of a nanosecond —
//!   cannot read as a large-percentage regression on a small baseline.
//! * Where the engine reports a per-point confidence interval (Criterion,
//!   `all_the_time`, `alloc_tracker`) the two regimes' intervals must also be
//!   disjoint; if they overlap this veto *withholds* the finding, treating the
//!   move as measurement noise. The veto direction is one-way: it can only
//!   suppress a candidate the other gates would have reported — it can never
//!   promote a move into a finding.
//! * Surviving candidates are screened to the directions the mode reports, then pass
//!   a Benjamini–Hochberg false-discovery filter, taken over every series judged
//!   rather than only those that raised a candidate, so a batch of series does not
//!   manufacture spurious findings and the rate the filter controls is the rate among
//!   the findings actually reported.
//!
//! A separate slow-[`Drift`](FindingMethod::Drift) finding is raised from a
//! Mann–Kendall trend test plus a Theil–Sen slope, gated by the same practical
//! floor and residual-scatter check, and is suppressed when a single step on the
//! same series already explains at least as much movement.
//!
//! Every gate above tests evidence, never cause. A level shift the surrounding
//! infrastructure produced — a toolchain upgrade, a move to a different runner — is
//! a real shift in the measured series, so it is reported as a finding to be blessed
//! rather than filtered out on the grounds of its origin. That is a statement about
//! what the detectors decline to look at, not a promise of detection: such a shift
//! still has to clear the same history-length, magnitude, scatter, significance and
//! false-discovery gates as any other move, and one that fails any of them goes
//! unreported like any other.
//!
//! Polarity: every metric is lower-is-better (instruction counts, branch counts,
//! allocations, wall and processor time), so a rise is a
//! [`Direction::Regression`] and a fall is a [`Direction::Improvement`].

use std::collections::BTreeMap;
use std::ops::Range;
#[cfg(any(test, feature = "private-test-util"))]
use std::slice;
use std::sync::Arc;

use anyspawn::Spawner;
use cbh_model::{BenchmarkId, DiscriminantSet, MetricKind};
use cbh_stats as stats;
use serde::Serialize;

use crate::detect::gate_log::{Gate, GateLog, GateStage, StageLog};
use crate::detect::parallel::{balanced_chunk_sizes, worker_count};
use crate::detect::{BaseLevel, Series, SeriesPoint, noise_gates};

/// Tunable parameters of the engine-aware analysis.
///
/// Every field is read as supplied and gates one stage of detection on its own; no
/// relationship between fields is required or enforced, so any combination is a
/// valid configuration. A combination that leaves a stage unreachable — a
/// [`compare_window`](Self::compare_window) holding fewer commit levels than
/// [`min_series_points`](Self::min_series_points), say — makes that stage abstain
/// rather than misreport.
///
/// [`AnalysisConfig::default()`](Self::default) is the tuned policy the tool ships.
/// Field documentation that gives a worked example, or that relates one field's
/// value to another's, describes that default configuration rather than the type.
#[derive(Clone, Copy, Debug)]
pub struct AnalysisConfig {
    /// Minimum points each side of a change must hold for the step to be trusted.
    ///
    /// A split leaving either regime shorter than this raises no candidate, so a move
    /// confined to fewer than this many trailing points cannot flag (persistence).
    /// Branch mode applies it to the base-side commit levels either side of a
    /// candidate regime boundary, and to the comparison sample as a whole.
    pub min_regime: usize,
    /// Minimum points a series must carry before it is evaluated at all.
    ///
    /// A shorter series raises no finding and does not count toward the
    /// false-discovery family: it is unjudged rather than judged-and-quiet (see
    /// [`Testability`]). History mode measures the post-blessing window against this
    /// floor; branch mode measures the base-side commit levels inside
    /// [`compare_window`](Self::compare_window) against it.
    ///
    /// Under [`AnalysisConfig::default()`](Self::default) this is two full regimes,
    /// the shortest series a split can satisfy [`min_regime`](Self::min_regime) on
    /// both sides of.
    pub min_series_points: usize,
    /// Significance level a change-point's Mann–Whitney rank test must clear.
    ///
    /// Pettitt only locates the split — its analytic p-value is too conservative on
    /// short series to gate significance — so the rank test between the two regimes
    /// decides. Branch mode holds its prediction-interval p-value to the same level,
    /// as does a candidate base-side regime boundary.
    pub change_alpha: f64,
    /// Target false-discovery rate for the Benjamini–Hochberg filter.
    ///
    /// The filter runs over the surviving candidates with the family sized from every
    /// series the pass judged (see [`SeriesCensus::judged`]), so a larger judged suite
    /// demands a smaller p-value of each candidate.
    pub fdr_q: f64,
    /// Minimum points a series needs before a slow-drift finding is considered.
    ///
    /// Independent of [`min_series_points`](Self::min_series_points), which decides
    /// whether the series is judged at all: a series above that floor but below this
    /// one is judged, and can raise only a level shift.
    ///
    /// [`AnalysisConfig::default()`](Self::default) sets the two equal, so both
    /// history detectors demand the same evidence and a series is evaluable by both
    /// or by neither.
    pub drift_min_points: usize,
    /// Significance level a drift's Mann–Kendall trend must clear.
    pub drift_alpha: f64,
    /// Multiple of the per-measurement noise floor a drift's total movement must
    /// exceed.
    ///
    /// Applies only where the engine reports per-point confidence intervals, whose
    /// median half-width is that noise floor. An additional veto on top of the trend
    /// test, able only to suppress a candidate. This is the drift counterpart of
    /// [`branch_noise_multiple`](Self::branch_noise_multiple), and
    /// [`AnalysisConfig::default()`](Self::default) holds the two equal.
    pub drift_noise_multiple: f64,
    /// Minimum relative magnitude a move must reach to matter in practice.
    ///
    /// Applied to the move against its baseline, independently of statistical
    /// significance, so a certain but tiny move is not reported. Branch mode
    /// substitutes [`branch_practical_relative`](Self::branch_practical_relative).
    pub practical_relative: f64,
    /// Absolute magnitude floor for instruction and branch counts, in counts.
    ///
    /// Composed by conjunction with the relative floor in force
    /// ([`practical_relative`](Self::practical_relative), or
    /// [`branch_practical_relative`](Self::branch_practical_relative) in branch mode):
    /// a move must clear both. An absolute floor is needed alongside the relative one
    /// because these counts move in whole units, so on a small baseline a handful of
    /// units of build-layout jitter works out to a large *percentage* move that the
    /// relative floor alone would let through.
    pub practical_absolute_count: f64,
    /// Absolute magnitude floor for timing moves, in nanoseconds.
    ///
    /// Composed by conjunction with the relative floor in force, exactly as
    /// [`practical_absolute_count`](Self::practical_absolute_count) is. A timing
    /// figure resolves far below a nanosecond, so this expresses which moves are worth
    /// acting on rather than what the engine can measure.
    ///
    /// Under [`AnalysisConfig::default()`](Self::default) this is the binding gate on
    /// a benchmark measuring a couple of nanoseconds an iteration, where the relative
    /// floor works out to a fraction of a nanosecond.
    pub practical_absolute_time: f64,
    /// Absolute magnitude floor for allocation moves, in bytes or allocations.
    ///
    /// Composed by conjunction with the relative floor in force, exactly as
    /// [`practical_absolute_count`](Self::practical_absolute_count) is. What an
    /// allocation floor rejects is the sub-unit moves that amortizing a run's warmup
    /// and buffer-resize allocations across its iterations manufactures, since a
    /// fraction of a byte or of an allocation cannot happen.
    pub practical_absolute_alloc: f64,
    /// Lower bound on the scatter of an instruction or branch count sample, in counts.
    ///
    /// Branch mode's prediction interval takes its standard error from the base
    /// window's standard deviation, and this bounds that deviation from below, so a
    /// window that happens to repeat one value still yields a usable standard error
    /// rather than a degenerate one. A scatter floor is the metric's *quantum*, not a
    /// statement about which moves matter — see
    /// [`scatter_floor_time`](Self::scatter_floor_time) for what raising one costs.
    pub scatter_floor_count: f64,
    /// Lower bound on the scatter of a timing sample, in nanoseconds.
    ///
    /// Serves the same role as [`scatter_floor_count`](Self::scatter_floor_count).
    /// Raising it makes every timing series behave as if it wobbled by at least that
    /// much, imposing an absolute detection threshold in units of the standard error
    /// on top of the [`practical_absolute_time`](Self::practical_absolute_time) floor
    /// that already decides which timing moves are worth reporting.
    ///
    /// [`AnalysisConfig::default()`](Self::default) leaves timing scatter unbounded
    /// from below, because a time is a regression slope over a run's iterations and
    /// resolves far below a clock tick, so it has no quantum to express. The price is
    /// that a base window of identical timings yields no verdict, which is silence
    /// rather than a spurious certainty.
    pub scatter_floor_time: f64,
    /// Lower bound on the scatter of an allocation sample, in bytes or allocations.
    ///
    /// Serves the same role as [`scatter_floor_count`](Self::scatter_floor_count).
    /// The case it exists for is code that allocated nothing and now allocates: a base
    /// window of zeroes has exactly zero scatter, and without a positive floor the
    /// standard error collapses and that (real and important) move cannot be judged.
    pub scatter_floor_alloc: f64,
    /// How many recent base-side commits branch mode inspects.
    ///
    /// The window is the base evidence the branch's latest state is compared against.
    /// A genuine level shift accepted inside it narrows the comparison to the trailing
    /// regime; otherwise the whole window is the comparison sample. Its size therefore
    /// sets both how small a move branch mode can resolve and how far back it looks
    /// for a current-regime boundary, and a window holding fewer commit levels than
    /// [`min_series_points`](Self::min_series_points) leaves branch mode unable to
    /// judge the series at all.
    pub compare_window: usize,
    /// Minimum relative magnitude a *branch* move must reach.
    ///
    /// Branch mode's substitute for [`practical_relative`](Self::practical_relative),
    /// applied both to a reported move and to a candidate base-side regime boundary.
    ///
    /// [`AnalysisConfig::default()`](Self::default) holds it above the history floor:
    /// a feature-branch signal must be high-confidence, since we would rather miss a
    /// small move than cry wolf on a pull request.
    pub branch_practical_relative: f64,
    /// Multiple of the per-measurement noise floor a branch move must exceed.
    ///
    /// Applies only where the engine reports per-point confidence intervals, whose
    /// median half-width is that noise floor. An additional veto on top of the
    /// prediction-interval test, able only to suppress a candidate.
    pub branch_noise_multiple: f64,
    /// Multiple of a series' own residual scatter a move must exceed to be trusted.
    ///
    /// The scatter is the median absolute residual of the fitted step or line model —
    /// the series' between-commit wobble. This is the primary, series-intrinsic noise
    /// gate applied to every engine: a clean series has near-zero residual scatter, so
    /// any persistent move clears it, while a jittery series demands a move that stands
    /// out above its own run-to-run wobble. It composes with (and is independent of)
    /// the optional confidence-interval veto available on dispersion-reporting engines.
    pub residual_noise_multiple: f64,
    /// Minimum **probability of superiority** a level shift's two regimes must reach.
    ///
    /// The probability of superiority is the Mann–Whitney common-language effect size:
    /// the fraction of after-vs-before commit pairs that move in the finding's
    /// direction. It is the *effect-size* companion to the rank test's *significance*
    /// gate, and closes a hole the significance gate cannot: a rank test grows
    /// "significant" with sample size even for two heavily overlapping regimes, so a
    /// long but stationary series that merely oscillates between two levels — noisy
    /// yet stable — otherwise reads as a change-point. A genuine step scores ~1 here;
    /// bimodal jitter scores near ½. The gate composes by conjunction, so it can only
    /// suppress a candidate the median-based gates were fooled by, never create one.
    pub min_regime_separation: f64,
    /// Minimum probability of superiority a base-window split must reach.
    ///
    /// Branch mode accepts a base-side split as a regime boundary — discarding the
    /// levels before it — only when the split clears this rather than
    /// [`min_regime_separation`](Self::min_regime_separation).
    ///
    /// The two decisions carry asymmetric costs, which is why
    /// [`AnalysisConfig::default()`](Self::default) holds this the higher of the pair.
    /// Reporting a move makes a claim a human then checks; accepting a boundary
    /// *discards evidence*, shrinking the comparison sample to the trailing regime and
    /// rebuilding the scatter estimate from it alone — so a wrong boundary can collapse
    /// a noisy window's dispersion to near zero and make any subsequent context run
    /// read as certain. A boundary that throws data away must be unambiguous.
    ///
    /// The statistic is coarse at these sample sizes, so at the default this reads as
    /// "essentially no crossing pair may contradict the boundary" rather than as a
    /// precise probability: with the smallest regimes on both sides it admits one
    /// contradicting pair in twenty-five and no more.
    pub min_base_split_separation: f64,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            min_regime: noise_gates::MIN_REGIME,
            min_series_points: noise_gates::MIN_SERIES_POINTS,
            change_alpha: noise_gates::CHANGE_ALPHA,
            fdr_q: noise_gates::FDR_Q,
            drift_min_points: noise_gates::DRIFT_MIN_POINTS,
            drift_alpha: noise_gates::DRIFT_ALPHA,
            drift_noise_multiple: noise_gates::DRIFT_NOISE_MULTIPLE,
            practical_relative: noise_gates::PRACTICAL_RELATIVE,
            practical_absolute_count: noise_gates::PRACTICAL_ABSOLUTE_COUNT,
            practical_absolute_time: noise_gates::PRACTICAL_ABSOLUTE_TIME,
            practical_absolute_alloc: noise_gates::PRACTICAL_ABSOLUTE_ALLOC,
            scatter_floor_count: noise_gates::SCATTER_FLOOR_COUNT,
            scatter_floor_time: noise_gates::SCATTER_FLOOR_TIME,
            scatter_floor_alloc: noise_gates::SCATTER_FLOOR_ALLOC,
            compare_window: noise_gates::COMPARE_WINDOW,
            branch_practical_relative: noise_gates::BRANCH_PRACTICAL_RELATIVE,
            branch_noise_multiple: noise_gates::BRANCH_NOISE_MULTIPLE,
            residual_noise_multiple: noise_gates::RESIDUAL_NOISE_MULTIPLE,
            min_regime_separation: noise_gates::MIN_REGIME_SEPARATION,
            min_base_split_separation: noise_gates::MIN_BASE_SPLIT_SEPARATION,
        }
    }
}

/// Which analysis a [`find_changes_spawned`] pass performs.
///
/// The mode is auto-detected by the caller from git topology and the admitted data
/// set (a base ref whose context commit is its own merge-base with no dirty run
/// admitted on that commit is [`History`](AnalysisMode::History); commits — or an
/// admitted dirty run — on top of the base make it [`Branch`](AnalysisMode::Branch)).
/// The working tree affects the choice only indirectly, through the exception that
/// admits a dirty run at the context commit while the tree is dirty.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisMode {
    /// Long-range trend and change-point analysis over a base branch's history.
    History,
    /// Latest context-run comparison against the base ref, ignoring the
    /// intermediate commits the branch passed through.
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
/// Carries which analysis to perform, the tuned parameters, and the topology anchors
/// the mode needs.
#[derive(Clone, Copy, Debug)]
pub struct AnalysisContext {
    /// The analysis to perform.
    pub mode: AnalysisMode,
    /// The tuned analysis parameters.
    pub config: AnalysisConfig,
    /// First-parent topological index of the merge-base on the context line, when it
    /// lies there. Branch detection does not derive its base window from this split;
    /// it compares against [`Series::base_window`], which is loaded from the base ref.
    pub merge_base_index: Option<usize>,
    /// First-parent topological index of the base ref's resolved commit.
    ///
    /// Branch mode uses this base-ref coordinate to measure comparison-base lag and
    /// to place the context commit just after the base ref in comparison charts.
    pub base_ref_index: Option<usize>,
    /// First-parent topological index of the analyzed context commit (the resolved
    /// `--context`/HEAD). History-mode chart building uses it as the trailing-fill
    /// target so a series that stops short of the context renders the data-less commits
    /// after its last observation as a gap. Consulted only in [`AnalysisMode::History`].
    pub tip_index: usize,
}

impl AnalysisContext {
    /// Whether a finding of the given `direction` is reported in this mode.
    ///
    /// The two modes ask differently shaped questions, so they keep different
    /// directions (DESIGN §8.3, §8.5). History mode is a drift watch over the base
    /// branch, where improvement over time is the expected background and only a
    /// worsening warrants attention, so it is one-directional. Branch mode judges one
    /// change against its base, where any movement is what the reader came for.
    fn keeps(&self, direction: Direction) -> bool {
        match self.mode {
            AnalysisMode::History => direction == Direction::Regression,
            AnalysisMode::Branch => true,
        }
    }

    /// Whether this analysis reports improvements at all. `false` in history mode's
    /// regressions-only drift watch, where an always-zero improvement tally is noise
    /// the report omits.
    #[must_use]
    pub fn reports_improvements(&self) -> bool {
        self.keeps(Direction::Improvement)
    }
}

/// Why a series was left unjudged.
///
/// A detection pass reaches a verdict only on series carrying enough evidence for
/// their mode's detector; every other series is unjudged for one of these reasons.
/// The set is exhaustive over the ways the analysis declines to test a series, so a
/// silent report can say how much of the suite its silence covers.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum UnjudgedReason {
    /// The benchmark carries no measurement at the analyzed context commit, so it is no
    /// longer part of the suite and was dropped before detection.
    Ghost,
    /// History mode: the series carries fewer than
    /// [`min_series_points`](AnalysisConfig::min_series_points) points.
    TooFewPoints,
    /// History mode: a blessing re-baselined the series and fewer than
    /// [`min_series_points`](AnalysisConfig::min_series_points) points have been
    /// measured since, so the evidence the blessing left standing is too thin to
    /// judge.
    TooFewPointsSinceBlessing,
    /// Branch mode: the context commit measured nothing for this series, so there is no
    /// context state to compare against the base.
    NotMeasuredOnBranch,
    /// Branch mode: the recent base window holds fewer than
    /// [`min_series_points`](AnalysisConfig::min_series_points) base-ref commit
    /// levels, so there is not enough base evidence to judge the branch. A later
    /// regime split may compare against a shorter trailing regime, but only after
    /// this full-window evidence floor is met.
    TooFewBaseCommits,
}

impl UnjudgedReason {
    /// Every reason, in declaration order, so a test can cover the set exhaustively.
    ///
    /// Reachable from the documentation generator as well as from this crate's own tests,
    /// because the appendix lists every reason and a list nothing checks would fall
    /// silently out of step the first time the set changed.
    #[cfg(any(test, feature = "private-test-util"))]
    pub const ALL: [Self; 5] = [
        Self::Ghost,
        Self::TooFewPoints,
        Self::TooFewPointsSinceBlessing,
        Self::NotMeasuredOnBranch,
        Self::TooFewBaseCommits,
    ];

    /// The lowercase wire name of the reason.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ghost => "ghost",
            Self::TooFewPoints => "too_few_points",
            Self::TooFewPointsSinceBlessing => "too_few_points_since_blessing",
            Self::NotMeasuredOnBranch => "not_measured_on_branch",
            Self::TooFewBaseCommits => "too_few_base_commits",
        }
    }

    /// A prose phrase describing the shortfall, worded to follow a count of series:
    /// `"9 series with too few points in the analyzed window"`.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Ghost => "not measured at the analyzed context commit",
            Self::TooFewPoints => "with too few points in the analyzed window",
            Self::TooFewPointsSinceBlessing => "with too few points since being blessed",
            Self::NotMeasuredOnBranch => "not measured on the branch",
            Self::TooFewBaseCommits => "with too few base-ref commits to compare against",
        }
    }
}

/// Whether a series carries enough evidence for its mode's detector to reach a
/// verdict, and if not, what it lacks.
///
/// The false-discovery family is exactly the [`Judged`](Testability::Judged) series.
/// A series that cannot be judged is not a hypothesis that was tested, so counting it
/// in the family would only dilute the correction. Conversely a series that *is*
/// judged must be counted whether or not it raised a candidate, since it had the same
/// opportunity to produce a false positive as any other.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Testability {
    /// The detector reached a verdict on the series.
    Judged,
    /// The series was not tested at all.
    Unjudged(UnjudgedReason),
}

impl Testability {
    /// Whether the detector reached a verdict.
    #[must_use]
    fn is_judged(self) -> bool {
        self == Self::Judged
    }
}

/// How many series an analysis judged, and why it left the rest unjudged.
///
/// This is what makes a report's silence readable: "nothing moved" says something
/// about the code only for the series that were judged, so the census travels with
/// the findings and every rendering discloses it. Each series is accounted for
/// exactly once, whether it was dropped before detection or declined by it, so
/// [`total`](Self::total) is the whole suite the analysis started from.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SeriesCensus {
    judged: usize,
    unjudged: BTreeMap<UnjudgedReason, usize>,
}

impl SeriesCensus {
    /// Accounts for one series with the verdict [`testability`] reached on it.
    pub fn record(&mut self, testability: Testability) {
        match testability {
            Testability::Judged => self.judged = self.judged.saturating_add(1),
            Testability::Unjudged(reason) => self.record_unjudged(reason, 1),
        }
    }

    /// Accounts for `series` series left unjudged for the same `reason`.
    ///
    /// Bulk form for the stages that drop series before detection sees them, which
    /// know only how many they dropped.
    pub fn record_unjudged(&mut self, reason: UnjudgedReason, series: usize) {
        if series == 0 {
            return;
        }
        let counted = self.unjudged.entry(reason).or_default();
        *counted = counted.saturating_add(series);
    }

    /// Absorbs another census, so a pass split across workers can recombine into one
    /// account.
    fn merge(&mut self, other: &Self) {
        self.judged = self.judged.saturating_add(other.judged);
        for (&reason, &series) in &other.unjudged {
            self.record_unjudged(reason, series);
        }
    }

    /// How many series the detectors reached a verdict on — the false-discovery
    /// family size.
    #[must_use]
    pub fn judged(&self) -> usize {
        self.judged
    }

    /// How many series went unjudged, for any reason.
    #[must_use]
    pub fn unjudged(&self) -> usize {
        self.unjudged
            .values()
            .fold(0_usize, |total, &series| total.saturating_add(series))
    }

    /// How many series the analysis accounted for in total.
    #[must_use]
    pub fn total(&self) -> usize {
        self.judged.saturating_add(self.unjudged())
    }

    /// The unjudged series broken down by reason, in [`UnjudgedReason`] order.
    ///
    /// That order runs with the pipeline: what the ghost filter dropped before
    /// detection, then the history-mode shortfalls, then the branch-mode ones.
    /// Reasons that account for no series are omitted.
    pub fn reasons(&self) -> impl Iterator<Item = (UnjudgedReason, usize)> + '_ {
        self.unjudged
            .iter()
            .map(|(&reason, &series)| (reason, series))
    }
}

/// What a detection pass found, and what it judged to find it.
///
/// The census travels with the findings because the two are only meaningful
/// together: an empty finding list means "nothing moved" only across the series the
/// census reports as judged.
#[derive(Clone, Debug, Default)]
pub struct Detection {
    /// The surviving findings, ranked most-notable first.
    pub findings: Vec<Finding>,
    /// What the pass judged, and why it left the rest unjudged.
    pub census: SeriesCensus,
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
    /// First-parent topological index of the commit the point was measured against.
    /// Charting places each point in its own per-commit column and materializes gaps
    /// for the data-less commits between points; it is not part of the JSON contract.
    pub topo_index: usize,
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
    /// Commit the change is attributed to, if known. For a change point this is the
    /// first commit of the new level; for a branch comparison it is the context
    /// commit; for a drift it is the newest commit the trend reached (paired with
    /// [`window_start_commit`](Self::window_start_commit) to name the accumulation
    /// range, since a drift belongs to the whole window rather than one commit).
    pub commit: Option<String>,
    /// The oldest commit of a drift's accumulation window, so the report can name the
    /// range the trend accrued over. `Some` only for a drift finding; `None` for a
    /// change point or branch comparison, which are attributed to a single commit.
    pub window_start_commit: Option<String>,
    /// Abbreviated commit of the blessing that re-baselined this series, if any.
    pub blessed_at: Option<String>,
    /// Effective (committer) time of the blessed commit, RFC 3339, if blessed.
    pub blessed_commit_time: Option<String>,
    /// The full underlying series, oldest-first. Retained internally so the text and
    /// Markdown reports can draw a chart; it is not part of the machine-readable JSON
    /// contract.
    pub series: Vec<SeriesValue>,
    /// Base-ref first-parent index of the newest base datum in the comparison sample.
    ///
    /// Set only in branch mode (`None` in history mode, where there is no single
    /// comparison base). When branch mode discards stale levels before an accepted
    /// base-side step, this remains the newest point of the trailing regime, because
    /// lag classification answers how current the compared base state is. Internal
    /// cross-crate analysis metadata that lets the analysis measure how far the
    /// comparison base sits behind the base ref; it is not part of the JSON finding
    /// contract.
    pub comparison_base_index: Option<usize>,
    /// Trailing-fill target for the chart: the first-parent index the charted series
    /// extends to when its last observation stops short of it. `Some(tip_index)` in
    /// history mode, so the data-less commits between the last observation and the
    /// analyzed context commit render as a gap; `None` in branch mode, where the
    /// context commit is the always-present last column. Chart-only — like
    /// [`Finding::series`] it is not part of the JSON finding contract.
    pub chart_base_ref: Option<usize>,
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
pub(crate) fn count_to_f64(count: usize) -> f64 {
    count as f64
}

/// One source point as a compact [`SeriesValue`] chart point, carrying its
/// `topo_index` so the renderer can place it in its own per-commit column.
fn series_value_of(point: &SeriesPoint) -> SeriesValue {
    SeriesValue {
        commit: owned_commit(point),
        value: point.value,
        dirty: point.dirty,
        topo_index: point.topo_index,
    }
}

/// Builds a surviving finding's compact chart series and its trailing-fill target,
/// dispatching on the analysis mode.
///
/// The series stays compact — one [`SeriesValue`] per real observation, never a
/// materialized gap — and every point carries its `topo_index`, so the renderer can
/// place each in its own per-commit column and draw the data-less commits between (and
/// after) observations as gaps. This is presentation-only: detection reads
/// `Series.points`, never this series.
///
/// History mode maps every source point 1:1 and returns `Some(context.tip_index)` as
/// the trailing-fill target, so a series that stops short of the analyzed context
/// commit renders the intervening commits as the "no newer data" gap. Branch mode
/// collapses the series (see [`branch_chart_series`]).
fn build_chart_series(
    source: &Series,
    finding: &Finding,
    context: &AnalysisContext,
) -> (Vec<SeriesValue>, Option<usize>) {
    match context.mode {
        AnalysisMode::History => (
            source.points.iter().map(series_value_of).collect(),
            Some(context.tip_index),
        ),
        AnalysisMode::Branch => branch_chart_series(source, finding, context),
    }
}

/// The branch-collapsed chart series and its (absent) trailing-fill target.
///
/// Branch mode judges the context commit alone, so the chart keeps the base ref's
/// comparison-window levels at their real base-ref `topo_index`, including any stale
/// pre-step base levels that detection deliberately ignored, drops every interior
/// context commit, and represents the context by a single point just after the base
/// ref. The chart is context rather than the comparison sample; when the detector
/// narrows to a trailing base regime, the older base points remain visible so the
/// base-side shift is understandable. The trailing-fill target is `None` — the
/// context is the always-present last column.
///
/// A real branch finding always carries a known base ref and comparison base; the
/// fallback to a plain whole-series chart is defensive (never a panic) for a finding
/// that somehow lacks either.
fn branch_chart_series(
    source: &Series,
    finding: &Finding,
    context: &AnalysisContext,
) -> (Vec<SeriesValue>, Option<usize>) {
    debug_assert!(
        context.base_ref_index.is_some() && finding.comparison_base_index.is_some(),
        "a branch finding always carries a known base ref and comparison base",
    );
    let Some(base_ref_index) = context.base_ref_index else {
        return (source.points.iter().map(series_value_of).collect(), None);
    };
    let mut series: Vec<SeriesValue> = source
        .base_window
        .iter()
        .map(|level| SeriesValue {
            commit: None,
            value: level.value,
            dirty: false,
            topo_index: level.topo_index,
        })
        .collect();
    let latest = latest_context_run(&source.points, context.tip_index);
    series.push(SeriesValue {
        commit: finding.commit.clone(),
        value: finding.latest,
        dirty: latest.last().is_some_and(|point| point.dirty),
        topo_index: base_ref_index.saturating_add(1),
    });
    (series, None)
}

/// The commit of a point as an owned `String`, for the JSON output.
///
/// Points intern their commit as a shared `Arc<str>`; the public finding fields are
/// plain owned strings, so a surviving finding pays one allocation here rather than
/// every point carrying its own copy.
fn owned_commit(point: &SeriesPoint) -> Option<String> {
    point.commit.as_deref().map(str::to_owned)
}

/// The direction of a change, given the signed delta from the baseline.
///
/// Every metric is lower-is-better, so a positive delta is a regression and a
/// negative one an improvement. The caller only reaches this with a non-zero delta,
/// so the exact zero case never arises in practice; it is defined as an improvement
/// so the classification is total.
fn direction_of(delta: f64) -> Direction {
    if delta > 0.0 {
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

/// The absolute-magnitude floor that applies to `kind`, in the metric's own units.
///
/// Each metric has a magnitude below which a move is not worth reporting, whatever
/// percentage it works out to: a few instructions is build layout, a fraction of a
/// nanosecond is not worth acting on, and a fraction of an allocation cannot happen.
/// The floors differ because those units do. This gates the *move*; the scatter of
/// the sample it is judged against is bounded separately (see [`scatter_floor`]).
fn absolute_floor(kind: MetricKind, config: &AnalysisConfig) -> f64 {
    match kind {
        MetricKind::InstructionCount
        | MetricKind::ConditionalBranches
        | MetricKind::IndirectBranches => config.practical_absolute_count,
        MetricKind::WallTime | MetricKind::ProcessorTime => config.practical_absolute_time,
        MetricKind::AllocatedBytes | MetricKind::AllocationCount => config.practical_absolute_alloc,
    }
}

/// The smallest scatter `kind` can express, in the metric's own units — its
/// *quantum*.
///
/// This is not a judgement about which moves matter (that is [`absolute_floor`]).
/// It bounds the *denominator* of the branch-mode prediction interval from below,
/// so a base window that happens to carry no scatter at all cannot collapse the
/// standard error. Counted metrics move in whole units and so cannot resolve a
/// scatter finer than one; a time is a regression slope over a run's iterations,
/// resolves far below a clock tick, and therefore has no quantum at all.
///
/// The match is exhaustive on purpose: a new metric kind must state its own
/// quantum rather than inherit one.
fn scatter_floor(kind: MetricKind, config: &AnalysisConfig) -> f64 {
    match kind {
        MetricKind::InstructionCount
        | MetricKind::ConditionalBranches
        | MetricKind::IndirectBranches => config.scatter_floor_count,
        MetricKind::WallTime | MetricKind::ProcessorTime => config.scatter_floor_time,
        MetricKind::AllocatedBytes | MetricKind::AllocationCount => config.scatter_floor_alloc,
    }
}

/// Whether a move clears the absolute-magnitude floor for `series`.
///
/// `delta` must span at least [`absolute_floor`] of the metric's own units,
/// otherwise a move too small to mean anything would clear the relative floor and
/// read as a regression on a small baseline. The gate composes with the relative
/// floor by conjunction and can only *suppress*, never promote, a move.
fn clears_absolute_floor(
    series: &Series,
    delta: f64,
    config: &AnalysisConfig,
    log: &mut StageLog<'_>,
) -> bool {
    let floor = absolute_floor(series.kind, config);
    log.numeric(
        Gate::AbsoluteFloor,
        delta.abs(),
        floor,
        delta.abs() >= floor,
    )
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

/// Whether two regimes' confidence intervals stand apart, where the engine reports
/// dispersion.
///
/// A one-way veto: a regime pair whose intervals overlap is treated as one measurement
/// spread across two windows and the candidate is withheld. An engine that reports no
/// dispersion offers no evidence either way, so the veto abstains and the move is
/// trusted — which is why the gate is recorded only when both intervals exist.
fn regime_intervals_are_disjoint(
    before: &[&SeriesPoint],
    after: &[&SeriesPoint],
    log: &mut StageLog<'_>,
) -> bool {
    let (Some(before_ci), Some(after_ci)) = (regime_interval(before), regime_interval(after))
    else {
        return true;
    };
    log.boolean(
        Gate::IntervalDisjoint,
        intervals_disjoint(before_ci, after_ci),
    )
}

/// Whether a branch base window's interval stands apart from the context run.
///
/// Branch mode collapses each base-ref commit before comparison, so the base-side
/// interval is reconstructed from those per-commit intervals rather than from raw
/// points. Like [`regime_intervals_are_disjoint`], this is a one-way veto: if either
/// side lacks interval evidence the gate abstains, because absent dispersion cannot
/// prove overlap.
fn base_intervals_are_disjoint(
    before: &[BaseLevel],
    after: &[&SeriesPoint],
    log: &mut StageLog<'_>,
) -> bool {
    let (Some(before_ci), Some(after_ci)) = (base_interval(before), regime_interval(after)) else {
        return true;
    };
    log.boolean(
        Gate::IntervalDisjoint,
        intervals_disjoint(before_ci, after_ci),
    )
}

/// The representative confidence interval of a collapsed base-ref comparison window.
fn base_interval(levels: &[BaseLevel]) -> Option<(f64, f64)> {
    let mut lows: Vec<f64> = levels
        .iter()
        .filter_map(|level| level.interval.map(|(low, _)| low))
        .collect();
    let mut highs: Vec<f64> = levels
        .iter()
        .filter_map(|level| level.interval.map(|(_, high)| high))
        .collect();
    Some((
        stats::median_in_place(&mut lows)?,
        stats::median_in_place(&mut highs)?,
    ))
}

/// Whether a move exceeds `multiple` times the per-measurement noise floor, where the
/// engine reports dispersion.
///
/// The noise floor is the median confidence-interval half-width across `points` (see
/// [`median_half_width`]): the per-point dispersion a single measurement carries. A move
/// inside that band is indistinguishable from that dispersion however the rest of the
/// evidence reads, so this is a one-way veto like [`regime_intervals_are_disjoint`], and
/// it abstains — recording nothing — on an engine that reports no dispersion.
fn exceeds_noise_band(
    delta: f64,
    points: &[SeriesPoint],
    multiple: f64,
    log: &mut StageLog<'_>,
) -> bool {
    let Some(half_width) = median_half_width(points) else {
        return true;
    };
    let band = multiple * half_width;
    log.numeric(
        Gate::IntervalNoiseBand,
        delta.abs(),
        band,
        delta.abs() > band,
    )
}

/// Branch-mode measurement-noise veto over the base levels and context run together.
///
/// The branch baseline is collapsed to one [`BaseLevel`] per base-ref commit before
/// comparison, but those levels still carry their representative confidence interval.
/// The noise band therefore uses the median half-width across the compared base commits
/// plus the context run, preserving the raw-regime behaviour branch mode had before the
/// base-ref window was loaded separately.
fn exceeds_branch_noise_band(
    delta: f64,
    before: &[BaseLevel],
    after: &[&SeriesPoint],
    multiple: f64,
    log: &mut StageLog<'_>,
) -> bool {
    let mut halves: Vec<f64> = before
        .iter()
        .filter_map(|level| level.interval.map(interval_half_width))
        .chain(after.iter().filter_map(|point| point_half_width(point)))
        .collect();
    let Some(half_width) = median_half_widths(&mut halves) else {
        return true;
    };
    let band = multiple * half_width;
    log.numeric(
        Gate::IntervalNoiseBand,
        delta.abs(),
        band,
        delta.abs() > band,
    )
}

/// The median confidence-interval half-width across `points`, when the engine
/// reports dispersion. Used as the per-measurement noise floor for noisy drift.
fn median_half_width(points: &[SeriesPoint]) -> Option<f64> {
    let mut halves: Vec<f64> = points.iter().filter_map(point_half_width).collect();
    median_half_widths(&mut halves)
}

fn median_half_widths(halves: &mut [f64]) -> Option<f64> {
    if halves.is_empty() {
        return None;
    }
    stats::median_in_place(halves)
}

fn point_half_width(point: &SeriesPoint) -> Option<f64> {
    match (point.interval_low, point.interval_high) {
        (Some(low), Some(high)) => Some(interval_half_width((low, high))),
        _ => None,
    }
}

fn interval_half_width((low, high): (f64, f64)) -> f64 {
    (high - low) / 2.0
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
///
/// A sample of a single point is its own median, so it contributes one residual of
/// exactly zero and nothing at all about the scatter it was drawn from. Pooling that zero
/// with a real sample's residuals only pulls the median down and weakens the gate, so a
/// single-point sample is left out. Branch mode compares against a single context commit, and
/// its comparison sample can be as short as `min_regime`, which is exactly where a diluted
/// residual would be least affordable.
fn sample_step_residual(before: &[f64], after: &[f64]) -> Option<f64> {
    let mut residuals: Vec<f64> = Vec::new();
    collect_scatter_residuals(before, &mut residuals);
    collect_scatter_residuals(after, &mut residuals);
    stats::median_in_place(&mut residuals)
}

/// Appends each point of `sample` distance from the sample's median to `residuals`,
/// unless the sample is too short to say anything about scatter.
fn collect_scatter_residuals(sample: &[f64], residuals: &mut Vec<f64>) {
    if sample.len() < 2 {
        return;
    }
    let Some(median) = stats::median(sample) else {
        return;
    };
    residuals.extend(sample.iter().map(|value| (value - median).abs()));
}

/// Whether `delta` stands clear of a series' own between-commit scatter: it must
/// exceed `config.residual_noise_multiple` times the model's median absolute
/// residual. A clean series has a near-zero residual, so any persistent move
/// passes; a jittery one demands a move that stands out above its wobble. A missing
/// residual (an empty model) is treated as no evidence of noise, so the move is
/// trusted.
fn exceeds_residual_noise(
    delta: f64,
    residual: Option<f64>,
    config: &AnalysisConfig,
    log: &mut StageLog<'_>,
) -> bool {
    match residual {
        Some(residual) => {
            let band = config.residual_noise_multiple * residual;
            log.numeric(Gate::ResidualNoise, delta.abs(), band, delta.abs() > band)
        }
        None => true,
    }
}

/// Whether the two regimes of a `delta`-signed level shift are *separated enough*
/// to be distinct populations, rather than two windows onto one noisy distribution.
///
/// A rank test's p-value proves only that the regimes *differ*, and it grows more
/// significant with sample size even for a heavily overlapping move — so a long but
/// stationary series that oscillates between two levels (noisy yet stable) passes
/// the significance gate. This gate adds the effect-size the significance test
/// lacks: the Mann–Whitney probability of superiority (the chance a random `after`
/// point exceeds a random `before` one), oriented in the move's direction, must
/// reach `floor`. A genuine step scores ~1; bimodal jitter scores near ½ and is
/// rejected. Missing statistics (`None`, from an empty sample) are treated as no
/// evidence of overlap, so the move is trusted.
///
/// The `floor` is the caller's, because what the separation buys differs by caller:
/// reporting a move is held to `min_regime_separation`, while accepting a base-window
/// regime boundary — which discards the levels before it — is held to the stricter
/// `min_base_split_separation`.
fn regimes_are_separated(
    mann_whitney: Option<stats::MannWhitneyU>,
    delta: f64,
    floor: f64,
    log: &mut StageLog<'_>,
) -> bool {
    match mann_whitney {
        // `superiority` is P(after > before); a fall is judged by the complementary
        // P(before > after), so both directions are measured against the same floor.
        Some(mann_whitney) => {
            let superiority = mann_whitney.superiority();
            let directional = if delta >= 0.0 {
                superiority
            } else {
                1.0 - superiority
            };
            log.numeric(
                Gate::RegimeSeparation,
                directional,
                floor,
                directional >= floor,
            )
        }
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
/// practical-magnitude floor (relative, plus the metric's own absolute floor), stand
/// above the series' own between-commit residual
/// scatter, separate the two regimes as populations (the Mann–Whitney effect-size
/// gate that rejects a noisy-but-stable series whose levels interleave), and — when
/// the engine reports per-point confidence intervals — separate the two regimes'
/// intervals.
fn evaluate_change_point(
    series: &Series,
    values: &[f64],
    config: &AnalysisConfig,
    log: &mut GateLog,
) -> Option<Candidate> {
    let mut log = log.stage(GateStage::ChangePoint);
    let points = &series.points;
    let n = points.len();

    let located = stats::pettitt(values);
    if !log.boolean(Gate::SplitLocated, located.is_some()) {
        return None;
    }
    let change = located?;
    let tau = change.index;
    let before_len = tau;
    let after_len = n.checked_sub(tau)?;
    let shortest = before_len.min(after_len);
    if !log.numeric(
        Gate::MinRegime,
        count_to_f64(shortest),
        count_to_f64(config.min_regime),
        shortest >= config.min_regime,
    ) {
        return None;
    }

    let before = values.get(..tau)?;
    let after = values.get(tau..)?;
    let baseline = stats::median(before)?;
    let latest = stats::median(after)?;
    let delta = latest - baseline;
    if !log.numeric(Gate::NonZeroDelta, delta.abs(), 0.0, delta.abs() > 0.0) {
        return None;
    }
    let relative_delta = relative_delta_of(delta, baseline);

    let mann_whitney_u = stats::MannWhitneyU::new(before, after);
    let mann_whitney = mann_whitney_u.map_or(1.0, |ranked| ranked.two_sided_p_value());
    if !log.numeric(
        Gate::Significance,
        mann_whitney,
        config.change_alpha,
        mann_whitney < config.change_alpha,
    ) {
        return None;
    }
    if !log.numeric(
        Gate::RelativeFloor,
        relative_delta.abs(),
        config.practical_relative,
        relative_delta.abs() >= config.practical_relative,
    ) {
        return None;
    }
    if !clears_absolute_floor(series, delta, config, &mut log) {
        return None;
    }
    if !exceeds_residual_noise(delta, step_model_residual(values, tau), config, &mut log) {
        return None;
    }
    if !regimes_are_separated(
        mann_whitney_u,
        delta,
        config.min_regime_separation,
        &mut log,
    ) {
        return None;
    }
    let before_points: Vec<&SeriesPoint> = points.iter().take(tau).collect();
    let after_points: Vec<&SeriesPoint> = points.iter().skip(tau).collect();
    if !regime_intervals_are_disjoint(&before_points, &after_points, &mut log) {
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
            direction: direction_of(delta),
            baseline,
            latest,
            delta,
            relative_delta,
            confidence: (1.0 - effective_p).clamp(0.0, 1.0),
            commit,
            window_start_commit: None,
            blessed_at: None,
            blessed_commit_time: None,
            series: Vec::new(),
            comparison_base_index: None,
            chart_base_ref: None,
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
/// movement must clear the practical-magnitude floor (relative, plus the metric's
/// own absolute floor) and stand above the series'
/// own residual scatter about the fitted line; where the engine reports confidence
/// intervals it must additionally exceed a multiple of the per-measurement noise floor
/// (`config.drift_noise_multiple` times the median half-width), so jitter does not read
/// as a trend.
fn evaluate_drift(
    series: &Series,
    values: &[f64],
    config: &AnalysisConfig,
    log: &mut GateLog,
) -> Option<Candidate> {
    let mut log = log.stage(GateStage::Drift);
    let points = &series.points;
    let n = points.len();
    if !log.numeric(
        Gate::MinSeriesPoints,
        count_to_f64(n),
        count_to_f64(config.drift_min_points),
        n >= config.drift_min_points,
    ) {
        return None;
    }

    let trend = stats::mann_kendall(values);
    if !log.numeric(
        Gate::Significance,
        trend.p_value,
        config.drift_alpha,
        trend.p_value < config.drift_alpha,
    ) {
        return None;
    }
    let (slope, intercept) = stats::theil_sen_line(values)?;
    let span = count_to_f64(n.checked_sub(1)?);
    let baseline = intercept;
    let latest = intercept + slope * span;
    let delta = latest - baseline;
    if !log.numeric(Gate::NonZeroDelta, delta.abs(), 0.0, delta != 0.0) {
        return None;
    }
    let relative_delta = relative_delta_of(delta, baseline);
    if !log.numeric(
        Gate::RelativeFloor,
        relative_delta.abs(),
        config.practical_relative,
        relative_delta.abs() >= config.practical_relative,
    ) {
        return None;
    }
    if !clears_absolute_floor(series, delta, config, &mut log) {
        return None;
    }
    if !exceeds_residual_noise(
        delta,
        line_model_residual(values, slope, intercept),
        config,
        &mut log,
    ) {
        return None;
    }
    // Where the engine reports dispersion, a trend must also clear the measurement
    // noise floor: the endpoints have to separate by more than the run-to-run
    // dispersion, or it is just jitter.
    if !exceeds_noise_band(delta, points, config.drift_noise_multiple, &mut log) {
        return None;
    }

    let commit = points.last().and_then(owned_commit);
    let window_start_commit = points.first().and_then(owned_commit);
    Some(Candidate {
        finding: Finding {
            set: series.set.clone(),
            id: series.id.clone(),
            kind: series.kind,
            method: FindingMethod::Drift,
            direction: direction_of(delta),
            baseline,
            latest,
            delta,
            relative_delta,
            confidence: (1.0 - trend.p_value).clamp(0.0, 1.0),
            commit,
            window_start_commit,
            blessed_at: None,
            blessed_commit_time: None,
            series: Vec::new(),
            comparison_base_index: None,
            chart_base_ref: None,
        },
        source_index: 0,
        bh_p: trend.p_value,
        split: None,
        line: Some((slope, intercept)),
    })
}

/// The points forming the last `window` levels of `points` (all of them when fewer
/// levels are present).
///
/// The window is measured in the groups [`commit_levels`] collapses to a single
/// level — normally one per commit — so it always yields at most `window` levels,
/// whatever number of stored runs those levels were reduced from. Measured in
/// points instead it would yield a different number of levels depending on how
/// many runs happened to fall inside it, and could shrink to a sample too small to
/// test against however long the history grew.
///
/// `points` is sorted by `(topo_index, dirty, object_ordinal)`, so each group is
/// contiguous and the window is a suffix slice.
#[cfg(test)]
fn recent_commits<'a>(points: &[&'a SeriesPoint], window: usize) -> Vec<&'a SeriesPoint> {
    let mut start = points.len();
    let mut commits = 0_usize;
    let mut current: Option<(usize, bool)> = None;
    for (index, point) in points.iter().enumerate().rev() {
        let key = (point.topo_index, point.dirty);
        if current != Some(key) {
            if commits == window {
                break;
            }
            commits = commits.saturating_add(1);
            current = Some(key);
        }
        start = index;
    }
    points
        .get(start..)
        .map(<[&SeriesPoint]>::to_vec)
        .unwrap_or_default()
}

/// The context commit's single latest run.
///
/// Branch mode judges the newest context state, not a reconstructed within-branch
/// regime and not a cohort of several runs at that commit. `points` is sorted by
/// `(topo_index, dirty, object_ordinal)`, so the latest run is simply the last point:
/// the context's committed (clean) run, or — when the working tree is dirty — the
/// newest dirty snapshot taken on top of it, which supersedes the clean run as the
/// newer state. Any earlier run at the same commit is not the state a merge would
/// land, so it is discarded. This keeps the comparison to one target observation,
/// matching the prediction interval it is judged against. An empty series yields no
/// point. A series whose newest observation stops before `context_index` was not
/// measured at the context commit and yields no point.
fn latest_context_run(points: &[SeriesPoint], context_index: usize) -> Vec<&SeriesPoint> {
    points
        .last()
        .filter(|point| point.topo_index == context_index)
        .map(|point| vec![point])
        .unwrap_or_default()
}

/// A commit group's level and where that group starts in the source point slice.
struct CommitLevel {
    start: usize,
    level: f64,
}

/// The per-commit levels of `points`, with their source-slice boundaries.
///
/// The boundaries let branch mode discard whole stale commit groups when it narrows a
/// base window to its trailing regime. The grouping is identical to [`commit_levels`]:
/// clean and dirty measurements at the same topological commit are different states,
/// while repeated runs of the same state collapse to their median.
#[cfg(test)]
fn commit_level_spans(points: &[&SeriesPoint]) -> Vec<CommitLevel> {
    let mut spans = Vec::new();
    let mut group: Vec<f64> = Vec::new();
    let mut current: Option<(usize, bool)> = None;
    let mut start = 0_usize;
    for (index, point) in points.iter().enumerate() {
        let key = (point.topo_index, point.dirty);
        if current != Some(key) {
            if let Some(level) = stats::median_in_place(&mut group) {
                spans.push(CommitLevel { start, level });
            }
            group.clear();
            current = Some(key);
            start = index;
        }
        group.push(point.value);
    }
    if let Some(level) = stats::median_in_place(&mut group) {
        spans.push(CommitLevel { start, level });
    }
    spans
}

/// One span per already-collapsed base-window level.
fn level_spans(levels: &[f64]) -> Vec<CommitLevel> {
    levels
        .iter()
        .enumerate()
        .map(|(start, &level)| CommitLevel { start, level })
        .collect()
}

/// The per-commit levels of `points`, oldest first.
///
/// Several stored runs can share one commit — repeated dirty snapshots re-measure
/// the same working tree — and those are replicates of a single tree state on a
/// single runner, not independent observations of the base level, so they collapse
/// to that group's median. What remains is a sample of the *between-commit*
/// scatter, which is the distribution a new commit's level must be judged against.
///
/// A commit's clean run and its dirty snapshots form separate groups: a dirty tree
/// is different source than the commit it sits at, so the two are not replicates of
/// each other. `points` is sorted by `(topo_index, dirty, object_ordinal)`, so every
/// group is contiguous.
#[cfg(test)]
fn commit_levels(points: &[&SeriesPoint]) -> Vec<f64> {
    commit_level_spans(points)
        .into_iter()
        .map(|span| span.level)
        .collect()
}

/// The point-slice index where the current base regime starts.
///
/// Branch mode first asks whether the whole recent base window has enough levels to be
/// evidence. Only then may it narrow the comparison sample to a shorter trailing regime:
/// a base-side step is accepted as a regime boundary only when Pettitt locates it, the two
/// sides each satisfy `min_regime`, Mann–Whitney significance and separation both pass,
/// and the step clears the same relative and absolute floors that make a branch move
/// reportable. If several suffixes expose qualifying steps, the newest split is used,
/// because the comparison should describe the regime the branch would merge into.
///
/// A split whose trailing regime the prediction interval cannot characterise is not
/// taken, so the window stays whole rather than becoming unjudgeable.
fn current_base_regime_start(
    series: &Series,
    spans: &[CommitLevel],
    config: &AnalysisConfig,
    practical_floor: f64,
) -> usize {
    let levels: Vec<f64> = spans.iter().map(|span| span.level).collect();
    let min_regime = config.min_regime.max(1);
    let Some(min_window) = min_regime.checked_mul(2) else {
        return 0;
    };
    let Some(last_start) = levels.len().checked_sub(min_window) else {
        return 0;
    };

    let mut latest_split: Option<usize> = None;
    for start in 0..=last_start {
        let Some(suffix) = levels.get(start..) else {
            continue;
        };
        let Some(change) = stats::pettitt(suffix) else {
            continue;
        };
        let Some(split) = start.checked_add(change.index) else {
            continue;
        };
        if base_regime_split_qualifies(series, suffix, change.index, config, practical_floor) {
            latest_split = Some(latest_split.map_or(split, |latest| latest.max(split)));
        }
    }

    latest_split
        .and_then(|split| spans.get(split))
        .map_or(0, |span| span.start)
}

/// Whether `split` is a genuine base-side regime boundary.
///
/// Every gate a reportable branch move must clear applies here, and the separation
/// gate applies at the stricter `min_base_split_separation` floor: accepting a
/// boundary discards the levels before it, and a boundary drawn through noise both
/// shrinks the comparison sample and collapses the scatter estimate it is rebuilt
/// from, so it must be unambiguous rather than merely reportable.
///
/// The trailing regime must also be one the prediction interval can characterise (see
/// [`regime_supports_prediction`]), since narrowing exists to sharpen the comparison
/// and a regime that yields no verdict would instead silence it.
///
/// The gates here are not observed: this is called once per candidate suffix split while
/// searching for the newest qualifying boundary, so its decisions describe the search
/// rather than the verdict on the series, and recording them would bury the branch
/// comparison's own gates under hundreds of rejected candidates.
fn base_regime_split_qualifies(
    series: &Series,
    levels: &[f64],
    split: usize,
    config: &AnalysisConfig,
    practical_floor: f64,
) -> bool {
    let mut unobserved = GateLog::disabled();
    let mut log = unobserved.stage(GateStage::Branch);
    let Some(before) = levels.get(..split) else {
        return false;
    };
    let Some(after) = levels.get(split..) else {
        return false;
    };
    let min_regime = config.min_regime.max(1);
    if before.len() < min_regime || after.len() < min_regime {
        return false;
    }
    if !regime_supports_prediction(series, after, config) {
        return false;
    }
    let Some(baseline) = stats::median(before) else {
        return false;
    };
    let Some(current) = stats::median(after) else {
        return false;
    };
    let delta = current - baseline;
    if delta.abs() <= 0.0 {
        return false;
    }
    if relative_delta_of(delta, baseline).abs() < practical_floor {
        return false;
    }
    if !clears_absolute_floor(series, delta, config, &mut log) {
        return false;
    }
    let mann_whitney_u = stats::MannWhitneyU::new(before, after);
    let mann_whitney = mann_whitney_u.map_or(1.0, |ranked| ranked.two_sided_p_value());
    if mann_whitney >= config.change_alpha {
        return false;
    }
    regimes_are_separated(
        mann_whitney_u,
        delta,
        config.min_base_split_separation,
        &mut log,
    )
}

/// Whether `levels` can serve as a branch-mode comparison sample.
///
/// The prediction interval needs a positive standard error, which comes from the
/// sample's own scatter or, where the sample carries none, from the metric's quantum
/// (see [`scatter_floor`](fn@scatter_floor)). A regime that offers neither yields no
/// verdict at all, so narrowing onto it would trade a comparison the full window can
/// still make for silence. Narrowing exists to move the comparison onto the current
/// level, not to withdraw it.
fn regime_supports_prediction(series: &Series, levels: &[f64], config: &AnalysisConfig) -> bool {
    if scatter_floor(series.kind, config) > 0.0 {
        return true;
    }
    stats::sample_std_dev(levels).is_some_and(|scatter| scatter > 0.0)
}

/// The two-sided p-value for `latest` being drawn from the same distribution as the
/// `base` levels, as a Student-t **prediction interval**.
///
/// The question branch mode asks is not "do these two samples differ" — there is
/// only one new observation — but "is a single new commit at this level surprising,
/// given how much the base level moves from commit to commit?". That is a
/// prediction interval for one future observation: the standard error carries the
/// scatter of the base levels *plus* the uncertainty in their mean, giving
/// `sd·√(1 + 1/n)` on `n − 1` degrees of freedom.
///
/// The mean and the Bessel-corrected sample standard deviation are used rather than
/// a median and a MAD: the MAD's low efficiency, its small-sample downward bias, and
/// the mismatch between a median centre and a mean-based standard error compound
/// into a test that fires far more often than its nominal rate. Base-side outliers
/// inflate the sample standard deviation, which errs toward silence.
///
/// Both the centre and the scale are deliberately non-robust *together*. Branch mode
/// handles a settled base-side step by moving both onto the trailing regime before this
/// function is called. That keeps the prediction interval coherent: making only the
/// scale robust while the centre stayed the mixed-window mean would put the centre
/// between two levels and make a context run agreeing exactly with the newer level read
/// as displaced from it.
///
/// `scatter_floor` is a lower bound on the standard deviation, in the metric's own
/// units: the smallest scatter the metric can express (see
/// [`scatter_floor`](fn@scatter_floor)). It guards against a base window whose
/// observed scatter is exactly zero, which would otherwise collapse the standard
/// error. A metric with no quantum passes zero here, and a degenerate window then
/// yields `None` — silence, not a spurious certainty.
///
/// `None` when the base sample is too small to estimate scatter at all, or when the
/// standard error is degenerate.
fn prediction_interval_p(base: &[f64], latest: f64, scatter_floor: f64) -> Option<f64> {
    let n = base.len();
    if n < 2 {
        return None;
    }
    let mean = stats::mean(base)?;
    let sd = stats::sample_std_dev(base)?.max(scatter_floor);
    let n_f = count_to_f64(n);
    let standard_error = sd * (1.0 + 1.0 / n_f).sqrt();
    if standard_error.is_nan() || standard_error <= 0.0 {
        return None;
    }
    let t = (latest - mean) / standard_error;
    Some(stats::student_t_two_sided_p(t, n_f - 1.0))
}

/// Compares base levels against a context run and returns a branch [`Candidate`].
///
/// `before` is the selected base-ref comparison sample, already collapsed to one
/// level per clean base commit. `after` is the context commit's latest run. The
/// comparison is therefore one new context level against the base ref's commit-to-commit
/// distribution.
///
/// The caller verifies that the full recent base window contains at least
/// `min_series_points` levels before this function runs. The comparison sample may be
/// shorter when that window holds a genuine base-side level shift: then branch mode
/// discards the stale prefix and compares against the trailing regime, whose
/// `min_regime` floor is the evidence needed for the prediction interval. The two
/// thresholds differ deliberately: `min_series_points` decides whether branch mode has
/// enough base evidence to test this series at all, while `min_regime` decides whether
/// an accepted current regime is large enough to serve as the comparison sample.
///
/// The base level is the comparison sample's **mean**, which is the centre
/// [`prediction_interval_p`] measures against, so the magnitude the finding reports
/// is the one its p-value describes.
///
/// The relative move must clear `practical_floor` and the metric's absolute floor,
/// stand above the comparison sample's own residual scatter, and then be significant as
/// a Student-t prediction interval. Where the engine reports per-point confidence
/// intervals the base sample and context measurement must also clear their combined
/// measurement noise band — an extra veto that can only *suppress* a candidate the
/// other gates would have reported, never turn a non-finding into a finding.
fn compare_branch_levels(
    series: &Series,
    before: &[BaseLevel],
    after: &[&SeriesPoint],
    config: &AnalysisConfig,
    practical_floor: f64,
    commit: Option<String>,
    log: &mut GateLog,
) -> Option<Candidate> {
    let mut log = log.stage(GateStage::Branch);
    let before_values: Vec<f64> = before.iter().map(|level| level.value).collect();
    let after_values: Vec<f64> = after.iter().map(|point| point.value).collect();
    let baseline = stats::mean(&before_values)?;
    let latest = stats::median(&after_values)?;
    let delta = latest - baseline;
    if !log.numeric(Gate::NonZeroDelta, delta.abs(), 0.0, delta != 0.0) {
        return None;
    }
    let relative_delta = relative_delta_of(delta, baseline);

    let min_regime = config.min_regime.max(1);
    if !log.numeric(
        Gate::MinRegime,
        count_to_f64(before_values.len()),
        count_to_f64(min_regime),
        before_values.len() >= min_regime,
    ) {
        return None;
    }
    if !log.numeric(
        Gate::RelativeFloor,
        relative_delta.abs(),
        practical_floor,
        relative_delta.abs() >= practical_floor,
    ) {
        return None;
    }
    if !clears_absolute_floor(series, delta, config, &mut log) {
        return None;
    }
    if !exceeds_residual_noise(
        delta,
        sample_step_residual(&before_values, &after_values),
        config,
        &mut log,
    ) {
        return None;
    }
    let interval_p =
        prediction_interval_p(&before_values, latest, scatter_floor(series.kind, config));
    if !log.boolean(Gate::BaseScatter, interval_p.is_some()) {
        return None;
    }
    let effective_p = interval_p?;
    if !log.numeric(
        Gate::Significance,
        effective_p,
        config.change_alpha,
        effective_p < config.change_alpha,
    ) {
        return None;
    }
    if !base_intervals_are_disjoint(before, after, &mut log) {
        return None;
    }
    // Where per-point confidence intervals exist, require the move to also clear the
    // measurement noise band — a veto that can only suppress this candidate.
    if !exceeds_branch_noise_band(delta, before, after, config.branch_noise_multiple, &mut log) {
        return None;
    }

    Some(Candidate {
        finding: Finding {
            set: series.set.clone(),
            id: series.id.clone(),
            kind: series.kind,
            method: FindingMethod::ChangePoint,
            direction: direction_of(delta),
            baseline,
            latest,
            delta,
            relative_delta,
            confidence: (1.0 - effective_p).clamp(0.0, 1.0),
            commit,
            window_start_commit: None,
            blessed_at: None,
            blessed_commit_time: None,
            series: Vec::new(),
            comparison_base_index: None,
            chart_base_ref: None,
        },
        source_index: 0,
        bh_p: effective_p,
        split: None,
        line: None,
    })
}

/// Test adapter for the branch sample comparison before base windows are collapsed.
#[cfg(test)]
fn compare_samples(
    series: &Series,
    before: &[&SeriesPoint],
    after: &[&SeriesPoint],
    config: &AnalysisConfig,
    practical_floor: f64,
    _comparison_base_index: Option<usize>,
    log: &mut GateLog,
) -> Option<Candidate> {
    let before_levels = test_base_levels(before);
    let commit = after.last().and_then(|point| owned_commit(point));
    compare_branch_levels(
        series,
        &before_levels,
        after,
        config,
        practical_floor,
        commit,
        log,
    )
}

#[cfg(test)]
fn test_base_levels(points: &[&SeriesPoint]) -> Vec<BaseLevel> {
    let spans = commit_level_spans(points);
    spans
        .iter()
        .enumerate()
        .filter_map(|(index, span)| {
            let end = spans
                .get(index.saturating_add(1))
                .map_or(points.len(), |next| next.start);
            let group = points.get(span.start..end)?;
            let topo_index = group.first()?.topo_index;
            Some(BaseLevel {
                topo_index,
                value: span.level,
                interval: regime_interval(group),
            })
        })
        .collect()
}

/// Evaluates a series in branch mode against the recent base-ref level.
///
/// The context's intermediate first-parent commits are ignored — only its newest
/// run matters (see [`latest_context_run`]), since that is the state a merge lands
/// in the base. A new benchmark introduced on the context (no base-ref points) or
/// an empty context yields nothing, since there is no baseline to compare.
///
/// The recent base window is first collapsed to per-commit levels and checked for
/// enough evidence as a whole. If that window contains a genuine level shift, located
/// with Pettitt and accepted only by the same Mann–Whitney significance, separation,
/// relative-floor, and absolute-floor gates that make such a split trustworthy, the
/// stale prefix before the newest accepted split is discarded. The prediction interval
/// then compares the context run against the trailing regime, moving its centre and
/// scatter together onto the base level the context would merge into.
fn evaluate_branch(
    series: &Series,
    config: &AnalysisConfig,
    context_index: usize,
    log: &mut GateLog,
) -> Option<Candidate> {
    // The base window arrives already capped to the recent `compare_window` levels
    // (attach_base_windows/`base_window_levels` own that truncation), so detection reads
    // it whole rather than re-slicing it here.
    let base_window = &series.base_window[..];
    let levels: Vec<f64> = base_window.iter().map(|level| level.value).collect();
    let base_spans = level_spans(&levels);
    if !log.stage(GateStage::Branch).numeric(
        Gate::MinBaseCommits,
        count_to_f64(base_spans.len()),
        count_to_f64(config.min_series_points),
        base_spans.len() >= config.min_series_points,
    ) {
        return None;
    }
    let comparison_start = current_base_regime_start(
        series,
        &base_spans,
        config,
        config.branch_practical_relative,
    );
    let comparison_base = base_window.get(comparison_start..).unwrap_or_default();
    let latest_points = latest_context_run(&series.points, context_index);
    let commit = latest_points.last().and_then(|point| owned_commit(point));
    // The newest base-ref point in the selected comparison sample is this series'
    // comparison base. Truncating stale levels changes the sample's start, not this
    // newest point, so lag classification still measures freshness against the base
    // state the context would merge into.
    let comparison_base_index = comparison_base.last().map(|level| level.topo_index);
    let mut candidate = compare_branch_levels(
        series,
        comparison_base,
        &latest_points,
        config,
        config.branch_practical_relative,
        commit,
        log,
    )?;
    candidate.finding.comparison_base_index = comparison_base_index;
    Some(candidate)
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
        base_window: series.base_window.clone(),
        active_start: 0,
        blessing: None,
    }
}

/// Records a history-mode finding's re-baseline provenance, so the report can name
/// the blessing.
///
/// The finding's charting points ([`Finding::series`]) are filled in later, when
/// the candidate survives filtering (see [`find_changes_spawned`]); a dropped candidate
/// never builds them.
fn stamp_history(finding: &mut Finding, series: &Series) {
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

/// Serial reference for the spawner-distributed [`find_changes_spawned`]: detects
/// every series in one contiguous scan, then runs the shared finalize tail.
///
/// Exists only as test scaffolding — the independent oracle for
/// `find_changes_spawned_matches_the_serial_pass` (the spawned path chunks and
/// recombines; this one never chunks), a spawner-free convenience for the crate's
/// unit tests (the tests below and the `signal_validation` suite), and the
/// documentation generator's batch entry point. Production detection goes through
/// [`find_changes_spawned`].
#[cfg(any(test, feature = "private-test-util"))]
#[must_use]
pub fn find_changes(series: &[Series], context: &AnalysisContext) -> Detection {
    let (candidates, census) = detect_all(series, context);
    let findings = finalize_findings(candidates, &census, series, context);
    Detection { findings, census }
}

/// Evaluates every series and returns the surviving findings, ranked
/// most-notable first, together with the census of what was judged to produce them
/// — the analysis's detection entry point.
///
/// The [`AnalysisContext`] selects the per-series detector: history mode locates a
/// change-point and a drift and keeps the better-fitting one; branch mode compares
/// the branch's latest state against its base. A series that cannot be judged (see
/// [`testability`]) is never evaluated and is accounted for in the returned
/// [`SeriesCensus`] instead.
/// Surviving candidates are screened to the directions the mode reports and then pass
/// a Benjamini–Hochberg false-discovery filter at `config.fdr_q`, so every reported
/// finding is one the correction rejected. Findings are ordered by descending relative
/// move, then method, then a stable identity tie-break.
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
) -> Detection {
    let (candidates, census) = detect_all_spawned(&series, context, spawner).await;
    let findings = finalize_findings(candidates, &census, &series, &context);
    Detection { findings, census }
}

/// Whether `series` carries enough evidence for its mode's detector to reach a
/// verdict, and if not, what it lacks.
///
/// This is the single definition of what "judged" means: detection consults it to
/// decide whether to evaluate a series at all, the census counts its answers, and the
/// false-discovery family is exactly the series it calls
/// [`Judged`](Testability::Judged).
#[must_use]
pub fn testability(series: &Series, context: &AnalysisContext) -> Testability {
    let config = &context.config;
    match context.mode {
        AnalysisMode::History => {
            // The detectors run on the post-blessing window (see `active_view`), so
            // that window's length — not the whole series' — is the evidence.
            let active_points = series.points.len().saturating_sub(series.active_start);
            if active_points >= config.min_series_points {
                Testability::Judged
            } else if series.active_start > 0 {
                Testability::Unjudged(UnjudgedReason::TooFewPointsSinceBlessing)
            } else {
                Testability::Unjudged(UnjudgedReason::TooFewPoints)
            }
        }
        AnalysisMode::Branch => {
            if latest_context_run(&series.points, context.tip_index).is_empty() {
                return Testability::Unjudged(UnjudgedReason::NotMeasuredOnBranch);
            }
            // Testability asks whether the full recent base window contains enough
            // evidence to run a branch comparison at all. Detection may then narrow to
            // a `min_regime`-sized trailing regime after an accepted base-side shift;
            // that does not make this census reason untruthful, because the evidence
            // floor was met before any history was discarded.
            let base_points = series.base_window.len().min(config.compare_window);
            if base_points < config.min_series_points {
                return Testability::Unjudged(UnjudgedReason::TooFewBaseCommits);
            }
            Testability::Judged
        }
    }
}

/// Applies the false-discovery filter, materialises the surviving findings' charting
/// points, and ranks them — the cross-series tail shared by the serial and
/// spawner-distributed detection passes.
///
/// `candidates` must be in series order (the order both detection paths produce) so
/// the Benjamini–Hochberg mask stays aligned, and `census` must be the account the
/// same pass produced, since its judged tally is the family the correction divides
/// by.
fn finalize_findings(
    candidates: Vec<Candidate>,
    census: &SeriesCensus,
    series: &[Series],
    context: &AnalysisContext,
) -> Vec<Finding> {
    let config = &context.config;

    // Screen to the directions this mode reports *before* the correction, so that every
    // hypothesis the correction rejects is a finding the report goes on to show.
    // Correcting over both directions and discarding one afterwards would attach the
    // false-discovery guarantee to a set larger than the reported one: discarding true
    // improvements shrinks the denominator the rate is defined over while leaving false
    // regressions in place, so the regressions actually shown would inherit no bound.
    // Screening first costs only power, and it is conservative: the detectors report
    // two-sided p-values from symmetric nulls, so for an unchanged series the chance of
    // raising a candidate in a direction named in advance is at most half the chance of
    // raising one either way. The p-values the correction sees therefore overstate the
    // risk of what it admits, and the bound holds with room to spare.
    // Ref: DESIGN.md §8.3.
    let candidates: Vec<Candidate> = candidates
        .into_iter()
        .filter(|candidate| context.keeps(candidate.finding.direction))
        .collect();

    // Control the false-discovery rate across every series that was actually judged,
    // not merely those that raised a candidate. Feeding the filter only its own
    // survivors would make it a no-op: each has already cleared `change_alpha`, which
    // is below the loosest Benjamini–Hochberg threshold, so nothing could ever be
    // rejected. The family is the whole set of hypotheses tested, which is precisely
    // what the census counted as judged.
    let family_size = census.judged();
    let candidate_p: Vec<f64> = candidates.iter().map(|candidate| candidate.bh_p).collect();
    let keep = stats::benjamini_hochberg(&candidate_p, config.fdr_q, family_size);
    let mut keep_iter = keep.into_iter();

    // `candidates` and `candidate_p` were built in the same order, so advancing
    // `keep_iter` for each candidate keeps the mask aligned. A surviving finding
    // materialises its charting points here — a dropped candidate never pays for them.
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
            let source = series
                .get(source_index)
                .expect("the source index was assigned from this series slice");
            let (chart_series, chart_base_ref) = build_chart_series(source, &finding, context);
            finding.series = chart_series;
            finding.chart_base_ref = chart_base_ref;
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
/// order — the order [`finalize_findings`] relies on — and the census of what was
/// judged.
#[cfg(any(test, feature = "private-test-util"))]
fn detect_all(series: &[Series], context: &AnalysisContext) -> (Vec<Candidate>, SeriesCensus) {
    detect_range(series, 0..series.len(), context, &mut GateLog::disabled())
}

/// Detects every series, distributed across workers: splits the series into one
/// balanced contiguous chunk per worker (the worker count is the available
/// parallelism capped at the series count), runs each chunk on its own blocking task
/// via `spawner`, and recombines the candidates in series order and the per-chunk
/// censuses into one.
///
/// A single available CPU (which is what Miri reports) yields a single worker — one
/// chunk, one task covering every series — so the one-worker case is just the
/// degenerate partition rather than a separate serial branch. An empty slice yields no
/// workers and dispatches no task.
async fn detect_all_spawned(
    series: &Arc<[Series]>,
    context: AnalysisContext,
    spawner: &Spawner,
) -> (Vec<Candidate>, SeriesCensus) {
    let len = series.len();
    let workers = worker_count(len);

    // Spawn every chunk before awaiting any, so the blocking tasks run concurrently;
    // each owns a shared `Arc` handle to the series and a `Copy` of the context.
    let mut handles = Vec::with_capacity(workers);
    let mut start: usize = 0;
    for size in balanced_chunk_sizes(len, workers) {
        let end = start.saturating_add(size);
        let chunk = Arc::clone(series);
        handles.push(spawner.spawn_blocking(move || {
            detect_range(&chunk, start..end, &context, &mut GateLog::disabled())
        }));
        start = end;
    }

    // Concatenate in spawn order, which is series order, so the candidate sequence is
    // identical to the serial pass. The census is order-insensitive, but every chunk
    // must be absorbed or the family would shrink to whatever one worker saw.
    let mut candidates = Vec::new();
    let mut census = SeriesCensus::default();
    for handle in handles {
        let (chunk_candidates, chunk_census) = handle.await;
        candidates.extend(chunk_candidates);
        census.merge(&chunk_census);
    }
    (candidates, census)
}

/// Detects the series in `range`, returning the raised candidates in index order and
/// the census of which of them were judged.
///
/// `log` observes the gates of every series in `range`, so a caller that wants a
/// readable log passes a range covering exactly one series (see [`evaluate_with_log`]);
/// every other caller passes a disabled log.
fn detect_range(
    series: &[Series],
    range: Range<usize>,
    context: &AnalysisContext,
    log: &mut GateLog,
) -> (Vec<Candidate>, SeriesCensus) {
    let mut candidates = Vec::new();
    let mut census = SeriesCensus::default();
    for index in range {
        let one = series
            .get(index)
            .expect("the range is within the series slice");
        let verdict = testability(one, context);
        census.record(verdict);
        // Judging and counting are the same decision, so a series the census reports
        // as unjudged provably never reached a detector.
        if verdict.is_judged()
            && let Some(candidate) = detect_one(index, one, context, log)
        {
            candidates.push(candidate);
        }
    }
    (candidates, census)
}

/// Runs the mode-appropriate detector on the series at `index` and returns its
/// candidate finding, if one is raised.
///
/// This is pure and depends on no other series, which is what lets
/// [`find_changes_spawned`] evaluate the series across workers. Callers must have
/// established that the series can be judged (see [`testability`]). History mode
/// locates a change-point and a drift and keeps the better-fitting one; branch mode
/// delegates to its dedicated detector.
/// `index` is the series' position in the analysed slice, stamped onto the candidate so
/// the finalize tail can materialise its charting points only if it survives filtering.
fn detect_one(
    index: usize,
    one: &Series,
    context: &AnalysisContext,
    log: &mut GateLog,
) -> Option<Candidate> {
    let config = &context.config;
    let candidate = match context.mode {
        AnalysisMode::History => {
            let active = active_view(one);
            // The point values are projected once here and shared by every history
            // detector, rather than each rebuilding the same `Vec<f64>`.
            let values: Vec<f64> = active.points.iter().map(|point| point.value).collect();
            let change = evaluate_change_point(&active, &values, config, log);
            let drift = evaluate_drift(&active, &values, config, log);
            arbitrate(&values, change, drift).map(|mut candidate| {
                stamp_history(&mut candidate.finding, one);
                candidate
            })
        }
        AnalysisMode::Branch => evaluate_branch(one, config, context.tip_index, log),
    };
    candidate.map(|mut candidate| {
        candidate.source_index = index;
        candidate
    })
}

/// Evaluates one series exactly as an analysis pass would, returning both the finding it
/// yields and a [`GateLog`] of how the detectors reached that verdict.
///
/// This is the observable form of detection: the same code path a real pass runs for one
/// series, including both history detectors and the arbitration between them, the
/// false-discovery filter, the mode's direction filter, and the charting points a
/// surviving finding carries. The verdict is therefore the verdict — evaluating a series
/// here and inside a whole-suite pass differ only in the size of the false-discovery
/// family, which one series makes as small as it can be.
///
/// Exists for tests that must assert *why* a series was reported or was quiet, and for
/// the documentation figures, which read the log rather than restating the policy. It is
/// an inspection facility, not part of the analysis API, so it is available only to
/// in-workspace consumers under `private-test-util`; the recording itself is compiled
/// unconditionally, so what is observed here is what production runs.
#[cfg(any(test, feature = "private-test-util"))]
#[must_use]
pub fn evaluate_with_log(series: &Series, context: &AnalysisContext) -> (Option<Finding>, GateLog) {
    let mut log = GateLog::recording();
    let batch = slice::from_ref(series);
    let (candidates, census) = detect_range(batch, 0..batch.len(), context, &mut log);
    let findings = finalize_findings(candidates, &census, batch, context);
    (findings.into_iter().next(), log)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "metric values are exact integer-derived counts"
    )]
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use std::slice;
    use std::sync::Arc;

    use cbh_model::{DiscriminantSet, Engine, MetricKind};
    use jiff::Timestamp;
    use nonempty::nonempty;

    use super::*;
    use crate::detect::gate_log::GateOutcome;
    use crate::detect::noise_gates::{
        COMPARE_WINDOW, DRIFT_MIN_POINTS, DRIFT_NOISE_MULTIPLE, MIN_REGIME, MIN_SERIES_POINTS,
        PRACTICAL_ABSOLUTE_COUNT, PRACTICAL_RELATIVE, RESIDUAL_NOISE_MULTIPLE,
    };
    use crate::detect::recorded::{
        STATIONARY_BIMODAL_BASE, STATIONARY_BIMODAL_HIGH, STATIONARY_BIMODAL_NOISE,
    };
    use crate::detect::scatter::{scattered, seed_of};
    use crate::detect::{Blessing, SeriesPoint, examples};

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
                engine: Engine::Callgrind,
                target_triple: "t".into(),
                machine_key: "m1".into(),
            },
            id: BenchmarkId::new(nonempty!["group".to_owned(), "case".to_owned()]),
            kind,
            points,
            base_window: Vec::new(),
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

    /// The `(value, threshold)` the first recorded outcome for `gate` compared, or
    /// `None` when the gate never ran or has no number to report.
    fn gate_value(log: &GateLog, gate: Gate) -> Option<(f64, f64)> {
        let outcome = log.entries().iter().find(|entry| entry.gate == gate)?;
        Some((outcome.value?, outcome.threshold?))
    }

    /// Three consecutive [`MIN_REGIME`]-point regimes at the given levels: the
    /// shortest history that can hold a level moving twice.
    fn three_regimes(first: f64, second: f64, third: f64) -> Vec<f64> {
        let mut values = vec![first; MIN_REGIME];
        values.extend(std::iter::repeat_n(second, MIN_REGIME));
        values.extend(std::iter::repeat_n(third, MIN_REGIME));
        values
    }

    /// Two consecutive [`MIN_REGIME`]-point regimes: the shortest history that can
    /// hold a change point, and exactly [`MIN_SERIES_POINTS`] points long.
    fn step_values(before: f64, after: f64) -> Vec<f64> {
        let mut values = vec![before; MIN_REGIME];
        values.extend(std::iter::repeat_n(after, MIN_REGIME));
        values
    }

    /// A perfectly straight `count`-point ramp starting at `start` and climbing by
    /// `slope` per point, which Theil-Sen fits exactly.
    fn ramp(start: f64, slope: f64, count: usize) -> Vec<f64> {
        #[expect(
            clippy::cast_precision_loss,
            reason = "fixture lengths are far below the f64 integer limit"
        )]
        (0..count)
            .map(|index| slope.mul_add(index as f64, start))
            .collect()
    }

    /// A `count`-point staircase starting at `start` that gains one unit every second
    /// point, which Theil-Sen fits with a slope of one half.
    fn staircase(start: f64, count: usize) -> Vec<f64> {
        ramp(start, 1.0, count)
            .into_iter()
            .flat_map(|level| [level, level])
            .take(count)
            .collect()
    }

    /// The topological index of the first point after a base run built by
    /// [`base_run`], i.e. the merge base a branch built on that run forks from.
    fn base_merge_base() -> usize {
        MIN_SERIES_POINTS - 1
    }

    /// A base-branch run holding the fewest commits branch mode will compare
    /// against: one point per commit, all at `value`, occupying topological
    /// indices `0..MIN_SERIES_POINTS`.
    fn base_run(value: f64) -> Vec<(usize, f64, bool)> {
        (0..MIN_SERIES_POINTS)
            .map(|index| (index, value, false))
            .collect()
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
                    engine: Engine::Callgrind,
                    target_triple: "t".into(),
                    machine_key: "m1".into(),
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
                window_start_commit: None,
                blessed_at: None,
                blessed_commit_time: None,
                series: Vec::new(),
                comparison_base_index: None,
                chart_base_ref: None,
            },
            source_index: 0,
            bh_p: 0.0,
            split,
            line,
        }
    }

    /// The largest `topo_index` across every point of `series`, the realistic context
    /// index for a history-mode [`AnalysisContext`] over this test fixture. Zero when
    /// there are no points.
    fn max_topo_index(series: &[Series]) -> usize {
        series
            .iter()
            .flat_map(|one| one.points.iter())
            .map(|point| point.topo_index)
            .max()
            .unwrap_or(0)
    }

    /// Runs the branch-mode detector for one fixture, using the fixture's newest
    /// topological point as the context commit.
    fn evaluate_branch(
        series: &Series,
        config: &AnalysisConfig,
        log: &mut GateLog,
    ) -> Option<Candidate> {
        let context_index = max_topo_index(slice::from_ref(series));
        if series.base_window.is_empty() {
            let mut series = series.clone();
            attach_test_base_windows(slice::from_mut(&mut series), context_index.checked_sub(1));
            return super::evaluate_branch(&series, config, context_index, log);
        }
        super::evaluate_branch(series, config, context_index, log)
    }

    /// Runs the history-mode detector with default config.
    fn changes(series: &[Series]) -> Vec<Finding> {
        find_changes(series, &history_context(series)).findings
    }

    /// The history-mode [`AnalysisContext`] the [`changes`] helper runs under.
    fn history_context(series: &[Series]) -> AnalysisContext {
        AnalysisContext {
            mode: AnalysisMode::History,
            config: AnalysisConfig::default(),
            merge_base_index: None,
            base_ref_index: None,
            tip_index: max_topo_index(series),
        }
    }

    /// Asserts that every series in `batch` is long enough to be judged and that the
    /// history detectors nevertheless raise nothing. Silence is only evidence about a
    /// gate when the series reached the gates at all, so a negative assertion must
    /// never be satisfied by a fixture that was never testable.
    fn judged_but_silent(batch: &[Series]) {
        let detection = find_changes(batch, &history_context(batch));
        assert_eq!(
            detection.census.judged(),
            batch.len(),
            "every fixture series must be judged, or the silence proves nothing"
        );
        assert!(detection.findings.is_empty());
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
        let step_up = step_values(100.0, 130.0);
        let step_down = step_values(130.0, 100.0);
        let flat = [100.0; MIN_SERIES_POINTS];
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
            base_ref_index: None,
            tip_index: max_topo_index(&series),
        };

        let serial = find_changes(&series, &context);
        let spawned = futures::executor::block_on(find_changes_spawned(
            Arc::from(series.as_slice()),
            context,
            &synchronous_spawner(),
        ));

        // `Finding` is not `PartialEq`; its `Debug` projection is a faithful, total
        // rendering of every field, so equal debug output means equal findings. The
        // census is compared too: a chunked pass that lost a worker's account would
        // shrink the false-discovery family without changing any single finding.
        assert!(
            !serial.findings.is_empty(),
            "the fixture must raise some findings"
        );
        assert_eq!(serial.census.judged(), series.len());
        assert_eq!(format!("{serial:#?}"), format!("{spawned:#?}"));
    }

    #[test]
    fn direction_of_flags_a_rise_as_a_regression() {
        // Every metric is lower-is-better, so a positive delta is a regression and a
        // negative one an improvement.
        assert_eq!(direction_of(1.0), Direction::Regression);
        assert_eq!(direction_of(-1.0), Direction::Improvement);
    }

    #[test]
    fn direction_of_classifies_a_zero_delta_as_an_improvement() {
        // The classification is total: a zero delta (never reached in practice) is
        // defined as an improvement.
        assert_eq!(direction_of(0.0), Direction::Improvement);
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
    fn noise_band_is_strict_at_the_exact_floor() {
        // A move equal to the band is within noise, not above it: the gate requires a
        // strict excess, so a move exactly at `multiple * half_width` is suppressed.
        // The scenario fixes an exact boundary (half-width 2.0, multiple 3.0, band
        // 6.0, delta 6.0), so relaxing `>` to `>=` would wrongly clear the gate here.
        let series = wall_series(&[100.0, 100.0, 100.0], 2.0);
        let mut unobserved = GateLog::disabled();
        let mut log = unobserved.stage(GateStage::Drift);
        assert!(!exceeds_noise_band(6.0, &series.points, 3.0, &mut log));
    }

    #[test]
    fn branch_noise_band_is_strict_at_the_exact_floor() {
        // The branch noise band draws its floor from the base levels' and context
        // run's intervals together; a move equal to that band is within noise. The
        // exact boundary (both half-widths 2.0, multiple 3.0, band 6.0, delta 6.0)
        // pins the strict `>` so a `>=` slip cannot pass a boundary move.
        let before = [BaseLevel {
            topo_index: 0,
            value: 100.0,
            interval: Some((98.0, 102.0)),
        }];
        let after_points = pts(&[(130.0, 2.0)]);
        let after: Vec<&SeriesPoint> = after_points.iter().collect();
        let mut unobserved = GateLog::disabled();
        let mut log = unobserved.stage(GateStage::Branch);
        assert!(!exceeds_branch_noise_band(
            6.0, &before, &after, 3.0, &mut log
        ));
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
    fn sample_step_residual_ignores_samples_too_short_to_show_scatter() {
        // A single point is its own median, so it says nothing about scatter and is left
        // out: only [10,12,20] contributes, whose residuals about 12 are 2,0,8.
        assert_eq!(
            sample_step_residual(&[10.0, 12.0, 20.0], &[30.0]),
            Some(2.0)
        );
        assert_eq!(sample_step_residual(&[], &[1.0, 2.0]), Some(0.5));
    }

    #[test]
    fn sample_step_residual_of_two_short_samples_is_none() {
        assert_eq!(sample_step_residual(&[], &[1.0]), None);
    }

    #[test]
    fn exceeds_residual_noise_requires_the_move_to_clear_the_scatter_band() {
        let config = AnalysisConfig::default();
        let mut unobserved = GateLog::disabled();
        let mut log = unobserved.stage(GateStage::ChangePoint);
        // A residual of 1.0 puts the band at 3x = 3.0. A move inside the band is
        // not clear of it, a move exactly at the band is still not (the comparison
        // is strict), a move above it is, and a missing residual trusts the move.
        assert!(!exceeds_residual_noise(1.0, Some(1.0), &config, &mut log));
        assert!(!exceeds_residual_noise(3.0, Some(1.0), &config, &mut log));
        assert!(exceeds_residual_noise(3.5, Some(1.0), &config, &mut log));
        assert!(exceeds_residual_noise(0.0, None, &config, &mut log));
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
        // Pettitt splits at tau=MIN_REGIME, so the before regime holds exactly
        // `min_regime` points: a `<=`/`==` slip on the before-regime bound would
        // reject the step. The after regime is padded to twice that so the split is
        // lopsided and only the before-regime bound is at its limit.
        let mut values = vec![100.0; MIN_REGIME];
        values.extend(std::iter::repeat_n(130.0, 2 * MIN_REGIME));
        let finding = only(changes(&[series_of(&values)]));
        assert_eq!(finding.method, FindingMethod::ChangePoint);
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.latest, 130.0);
    }

    #[test]
    fn change_point_accepts_a_minimal_after_regime() {
        // The mirror image: Pettitt splits at tau=2*MIN_REGIME, so the after regime
        // holds exactly `min_regime` points and a `<=` slip on the after-regime bound
        // would reject the step.
        let mut values = vec![100.0; 2 * MIN_REGIME];
        values.extend(std::iter::repeat_n(130.0, MIN_REGIME));
        let finding = only(changes(&[series_of(&values)]));
        assert_eq!(finding.method, FindingMethod::ChangePoint);
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.latest, 130.0);
    }

    #[test]
    fn change_point_rejects_a_single_point_regime() {
        // Pettitt splits at tau=1, leaving a one-point before regime (below
        // min_regime) against a full-size after regime. The size guard rejects when
        // *either* regime is too small, so a `||`->`&&` slip would wrongly admit this
        // lopsided split. A permissive rank-test threshold isolates the guard: only
        // the size check keeps it out.
        let config = AnalysisConfig {
            change_alpha: 0.5,
            ..AnalysisConfig::default()
        };
        let mut values = vec![100.0];
        values.extend(std::iter::repeat_n(130.0, MIN_REGIME));
        let series = series_of(&values);
        assert!(
            evaluate_change_point(
                &series,
                &values_of(&series),
                &config,
                &mut GateLog::disabled()
            )
            .is_none()
        );
    }

    #[test]
    fn change_point_within_its_own_residual_scatter_is_suppressed() {
        // A rank-significant step (medians 102 -> 132, delta 30) whose regimes each
        // wobble by 2. Under the default residual multiple the move stands clear of
        // that scatter and is flagged; a deliberately high multiple pushes the noise
        // band above the move, so only the residual gate rejects it (every earlier
        // gate — persistence, Mann–Whitney, practical floor — still passes).
        let series = series_of(&[
            100.0, 104.0, 100.0, 104.0, 102.0, 130.0, 134.0, 130.0, 134.0, 132.0,
        ]);
        assert!(
            evaluate_change_point(
                &series,
                &values_of(&series),
                &AnalysisConfig::default(),
                &mut GateLog::disabled()
            )
            .is_some()
        );
        let config = AnalysisConfig {
            residual_noise_multiple: 20.0,
            ..AnalysisConfig::default()
        };
        assert!(
            evaluate_change_point(
                &series,
                &values_of(&series),
                &config,
                &mut GateLog::disabled()
            )
            .is_none()
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "asserts a strict `<` at the exact recorded p-value, which needs bit-identical \
                  recomputation of the rank statistic; Miri's float nondeterminism perturbs it"
    )]
    fn change_point_significance_is_a_strict_boundary() {
        // The rank-test gate is a strict `<`: a candidate whose p-value lands exactly on
        // the threshold is rejected. Rather than hard-code that p, take it from the
        // detector's own recording at the default alpha, then set alpha to it — so a
        // `<`->`<=` slip that admits the boundary turns this reported step silent.
        let series = series_of(&[
            100.0, 104.0, 100.0, 104.0, 102.0, 130.0, 134.0, 130.0, 134.0, 132.0,
        ]);
        let mut log = GateLog::recording();
        assert!(
            evaluate_change_point(
                &series,
                &values_of(&series),
                &AnalysisConfig::default(),
                &mut log
            )
            .is_some(),
            "the fixture must report at the default alpha"
        );
        let (p, _) = gate_value(&log, Gate::Significance).expect("the significance gate ran");
        let at_boundary = AnalysisConfig {
            change_alpha: p,
            ..AnalysisConfig::default()
        };
        assert!(
            evaluate_change_point(
                &series,
                &values_of(&series),
                &at_boundary,
                &mut GateLog::disabled()
            )
            .is_none(),
            "a p-value exactly at alpha must be rejected by the strict gate"
        );
    }

    #[test]
    fn regimes_are_separated_rejects_interleaved_levels() {
        let floor = AnalysisConfig::default().min_regime_separation;
        let mut unobserved = GateLog::disabled();
        let mut log = unobserved.stage(GateStage::ChangePoint);
        // A clean rise: every after-point exceeds every before-point (superiority 1).
        assert!(regimes_are_separated(
            stats::MannWhitneyU::new(&[10.0, 11.0, 12.0], &[20.0, 21.0, 22.0]),
            10.0,
            floor,
            &mut log,
        ));
        // A clean fall: judged by the complementary direction, still fully separated.
        assert!(regimes_are_separated(
            stats::MannWhitneyU::new(&[20.0, 21.0, 22.0], &[10.0, 11.0, 12.0]),
            -10.0,
            floor,
            &mut log,
        ));
        // Two levels that recur on both sides: only 0.75 of the after-vs-before pairs
        // move in the rise's direction, below the 0.85 floor, so it is not separated.
        assert!(!regimes_are_separated(
            stats::MannWhitneyU::new(&[10.0, 10.0, 10.0, 30.0], &[30.0, 30.0, 30.0, 10.0]),
            20.0,
            floor,
            &mut log,
        ));
        // The falling mirror of that overlap: the same two levels recur on both sides,
        // so only 0.75 of the pairs move in the fall's (complementary) direction and it
        // is likewise rejected. Unlike the clean fall above — whose superiority of 0
        // leaves `1 − superiority` indistinguishable from other arithmetic — this pins
        // the fall branch at a fractional superiority (0.25), so the complementary
        // `1 − 0.25 = 0.75 < 0.85` is exercised as a genuine subtraction.
        assert!(!regimes_are_separated(
            stats::MannWhitneyU::new(&[30.0, 30.0, 30.0, 10.0], &[10.0, 10.0, 10.0, 30.0]),
            -20.0,
            floor,
            &mut log,
        ));
        // No statistics at all (an empty regime): the gate has nothing to veto on, so
        // it trusts the move rather than suppressing it.
        assert!(regimes_are_separated(None, 10.0, floor, &mut log));
    }

    #[test]
    fn the_floor_a_caller_passes_is_the_one_the_separation_gate_applies() {
        // One overlapping split, judged against both floors: the reporting floor admits
        // it and the stricter base-split floor rejects it. This is what makes the two
        // floors distinct policies rather than one constant read from two places.
        let config = AnalysisConfig::default();
        let mut unobserved = GateLog::disabled();
        let mut log = unobserved.stage(GateStage::Branch);
        // Seven before-levels against five after-levels, with one before-level lying
        // inside the after regime but under only its topmost value: 34 of 35 crossing
        // pairs fall, a superiority of ~0.971.
        let after = [10.0, 11.0, 12.0, 13.0, 14.0];
        let before = [30.0, 31.0, 32.0, 33.0, 34.0, 35.0, 13.5];
        let ranked = stats::MannWhitneyU::new(&before, &after);
        assert!(regimes_are_separated(
            ranked,
            -20.0,
            config.min_regime_separation,
            &mut log,
        ));
        assert!(regimes_are_separated(
            ranked,
            -20.0,
            config.min_base_split_separation,
            &mut log,
        ));

        // The same shape with that stray level one step lower, so it sits under two of
        // the after-levels: 33 of 35 pairs fall, ~0.943.
        let before = [30.0, 31.0, 32.0, 33.0, 34.0, 35.0, 12.5];
        let ranked = stats::MannWhitneyU::new(&before, &after);
        assert!(regimes_are_separated(
            ranked,
            -20.0,
            config.min_regime_separation,
            &mut log,
        ));
        assert!(!regimes_are_separated(
            ranked,
            -20.0,
            config.min_base_split_separation,
            &mut log,
        ));
    }

    #[test]
    fn change_point_across_interleaved_regimes_is_suppressed() {
        // The real-world series that motivated the separation gate: a wall-time metric
        // that oscillates between ~13 and ~25-29 throughout its whole history, so no
        // commit marks a real level shift. Pettitt aligns the split with each side's
        // dominant mode, collapsing the median-absolute residual so the residual gate
        // is fooled and (before this gate) a spurious "regression via change point"
        // was emitted. The regimes overlap heavily (probability of superiority ~0.72),
        // so the separation gate rejects it. Dropping the separation floor to zero
        // admits the split again, proving that gate is the sole reason it is silent.
        let values = STATIONARY_BIMODAL_NOISE.to_vec();
        let series = series_of(&values);
        let permissive = AnalysisConfig {
            min_regime_separation: 0.0,
            ..AnalysisConfig::default()
        };
        assert!(
            evaluate_change_point(&series, &values, &permissive, &mut GateLog::disabled())
                .is_some()
        );
        assert!(
            evaluate_change_point(
                &series,
                &values,
                &AnalysisConfig::default(),
                &mut GateLog::disabled()
            )
            .is_none()
        );
    }

    #[test]
    fn sustained_step_is_flagged_as_a_change_point() {
        // A clean step from 100 to 130 with `min_regime` points each side: the
        // shortest history a change point can be found in.
        let series = series_of(&step_values(100.0, 130.0));
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
        assert_eq!(
            finding.commit.as_deref(),
            Some(format!("commit{MIN_REGIME}").as_str())
        );
    }

    #[test]
    fn step_below_the_practical_floor_is_suppressed() {
        // A sub-3% move is treated as measurement noise even when it looks clean, so
        // the practical-magnitude floor suppresses it: 1000 -> 1001 is a 0.1% move.
        let series = series_of(&step_values(1000.0, 1001.0));
        judged_but_silent(&[series]);
    }

    #[test]
    fn step_at_the_practical_floor_is_flagged() {
        // The practical floor is a strict `<` rejection, so a step whose relative move
        // EQUALS the 3% floor is still reported: 1000 -> 1030 is exactly a 3% move.
        let series = series_of(&step_values(1000.0, 1030.0));
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
        let series = series_of(&step_values(1000.0, 999.0));
        judged_but_silent(&[series]);
    }

    #[test]
    fn change_point_below_the_absolute_floor_is_suppressed() {
        // On a quantized metric a 4-count move clears the relative floor (4/60 ≈ 6.7%
        // ≥ 3%) and every other gate — significant separation, zero residual — yet is
        // suppressed because it falls short of the absolute floor of 5, where a
        // single-quantum wobble on a tiny count would otherwise read as a regression.
        let series = series_of(&step_values(60.0, 64.0));
        judged_but_silent(&[series]);
    }

    #[test]
    fn change_point_at_the_absolute_floor_is_flagged() {
        // The absolute floor is a `>=` gate, so a 5-count move exactly at the floor is
        // still reported (a `>`/`==` mutant would suppress or misgate it).
        let series = series_of(&step_values(60.0, 65.0));
        let finding = only(changes(&[series]));
        assert_eq!(finding.method, FindingMethod::ChangePoint);
        assert_eq!(finding.delta, 5.0);
    }

    #[test]
    fn change_point_below_the_absolute_floor_on_a_continuous_metric_is_suppressed() {
        // The absolute floor is universal: a continuous metric gets its own, much
        // smaller floor rather than an exemption. This sub-nanosecond wall-time move
        // clears the relative floor, separates cleanly and carries disjoint
        // intervals, yet 0.63 ns of movement is below the resolution any wall-clock
        // measurement can be trusted at, so it stays silent. Zeroing that one floor
        // admits it again, proving the floor is the sole reason for the silence.
        let series = wall_series(&step_values(2.49, 3.12), 0.05);
        judged_but_silent(slice::from_ref(&series));
        let permissive = AnalysisConfig {
            practical_absolute_time: 0.0,
            ..AnalysisConfig::default()
        };
        assert!(
            evaluate_change_point(
                &series,
                &values_of(&series),
                &permissive,
                &mut GateLog::disabled()
            )
            .is_some()
        );
    }

    #[test]
    fn branch_count_rise_is_a_regression() {
        // Branch-execution counts are lower-is-better, so a sustained rise is a
        // regression.
        let series = series_with(
            &step_values(70.0, 100.0),
            MetricKind::ConditionalBranches,
            &[],
        );
        let finding = only(changes(&[series]));
        assert!(finding.is_regression());
        assert_eq!(finding.delta, 30.0);
    }

    #[test]
    fn flat_series_never_flags() {
        let series = series_of(&[100.0; MIN_SERIES_POINTS]);
        judged_but_silent(&[series]);
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
            series.push(named_series(&name, &step_values(100.0, raised)));
            stepped_ids.push(BenchmarkId::new(nonempty![name, "case".to_owned()]).qualified());
            // A flat companion never flags, so it must be absent from the output.
            series.push(named_series(
                &format!("flat{raw:03}"),
                &[100.0; MIN_SERIES_POINTS],
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
        let mut values = vec![100.0; MIN_SERIES_POINTS];
        values.push(175.0);
        judged_but_silent(&[series_of(&values)]);
    }

    /// A sustained excursion that has since returned to its opening level is silent, whatever
    /// its magnitude, because the analysis reports what is true of the current state and the
    /// current state matches the baseline.
    ///
    /// This is the property the analysis is narrowed to, so it is asserted directly rather
    /// than left to follow from the detectors' arithmetic: a future change to the
    /// change-point or arbitration path could start reporting such an excursion at its rise,
    /// and nothing else in the suite would notice.
    #[test]
    fn an_excursion_that_returned_to_its_opening_level_is_silent() {
        judged_but_silent(&[series_of(&three_regimes(100.0, 130.0, 100.0))]);
    }

    #[test]
    fn step_in_the_final_point_fails_persistence() {
        // The shift has one point too few after it, so it is rejected even though the
        // levels differ; `change_point_accepts_a_minimal_after_regime` pins the other
        // side of the same boundary.
        let mut values = vec![100.0; MIN_SERIES_POINTS];
        values.extend(std::iter::repeat_n(130.0, MIN_REGIME - 1));
        judged_but_silent(&[series_of(&values)]);
    }

    #[test]
    fn noisy_jitter_around_a_stable_mean_is_not_flagged() {
        // Pure measurement jitter with no real shift must stay silent.
        let series = wall_series(
            &[
                100.0, 103.0, 98.0, 101.0, 99.0, 102.0, 97.0, 100.0, 101.0, 99.0,
            ],
            5.0,
        );
        judged_but_silent(&[series]);
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
        judged_but_silent(&[series]);
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
        let candidate = evaluate_change_point(
            &series,
            &values_of(&series),
            &config,
            &mut GateLog::disabled(),
        )
        .unwrap();
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
        judged_but_silent(&[series]);
    }

    #[test]
    fn monotonic_drift_is_flagged() {
        // A steady climb with no single dominant step surfaces as a drift finding.
        let series = series_of(&ramp(100.0, 4.0, MIN_SERIES_POINTS));
        let finding = only(changes(&[series]));
        assert_eq!(finding.method, FindingMethod::Drift);
        assert_eq!(finding.direction, Direction::Regression);
        assert!(finding.delta > 0.0);
        // baseline = fitted intercept (100), latest = intercept + slope*(n-1).
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.latest, 136.0);
    }

    #[test]
    fn a_sharp_step_is_reported_as_a_change_point_not_a_drift() {
        // A series that both trends and steps: the two-regime model fits the sharp
        // jump better than a line, so it is reported once, as a change-point.
        let mut values = ramp(100.0, 1.0, MIN_REGIME);
        values.extend(ramp(160.0, 1.0, MIN_REGIME));
        let findings = changes(&[series_of(&values)]);
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
                        99.0 + bump,
                        100.0,
                    ],
                    6.0,
                )
            })
            .collect();
        judged_but_silent(&series);
    }

    #[test]
    fn a_strong_noisy_signal_survives_the_false_discovery_filter() {
        // One unmistakable step alongside many flat series. The false-discovery
        // correction divides by the number of *testable* series, so the single real
        // finding must carry a p-value below q/m to survive: six-point regimes with
        // distinct values inside each give it the margin that a bare `min_regime`
        // step would not have.
        let mut series = vec![wall_series(
            &[
                98.0, 100.0, 102.0, 99.0, 101.0, 100.0, 148.0, 150.0, 152.0, 149.0, 151.0, 150.0,
            ],
            2.0,
        )];
        for _ in 0..6 {
            series.push(wall_series(
                &[
                    100.0, 101.0, 99.0, 100.0, 101.0, 99.0, 100.0, 101.0, 99.0, 100.0,
                ],
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
        let larger = series_of(&step_values(100.0, 200.0));
        let smaller = series_of(&step_values(1000.0, 1050.0));
        let findings = changes(&[smaller, larger]);
        assert_eq!(findings.len(), 2);
        assert!(findings[0].relative_delta.abs() > findings[1].relative_delta.abs());
        assert_eq!(findings[0].latest, 200.0);
        assert_eq!(findings[1].latest, 1050.0);
    }

    #[test]
    fn find_changes_retains_distinct_identities_ordered_by_move() {
        let larger = series_with(
            &step_values(100.0, 200.0),
            MetricKind::InstructionCount,
            &[],
        );
        let mut smaller = series_with(
            &step_values(100.0, 150.0),
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

    /// Builds a series of `kind` from explicit `(topo_index, value, dirty)` points, so
    /// branch splits can be modelled precisely. Points are taken in the given order
    /// (already topological) and carry no dispersion.
    fn placed_series_of_kind(points: &[(usize, f64, bool)], kind: MetricKind) -> Series {
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
                engine: Engine::Callgrind,
                target_triple: "t".into(),
                machine_key: "m1".into(),
            },
            id: BenchmarkId::new(nonempty!["group".to_owned(), "case".to_owned()]),
            kind,
            points,
            base_window: Vec::new(),
            active_start: 0,
            blessing: None,
        }
    }

    /// Builds a Callgrind-style (instruction count) series from explicit
    /// `(topo_index, value, dirty)` points.
    fn placed_series(points: &[(usize, f64, bool)]) -> Series {
        placed_series_of_kind(points, MetricKind::InstructionCount)
    }

    /// Runs the branch-mode detector with default config and the given merge-base.
    fn branch_changes(series: &[Series], merge_base_index: Option<usize>) -> Vec<Finding> {
        let mut series = series.to_vec();
        attach_test_base_windows(&mut series, merge_base_index);
        find_changes(&series, &branch_context(&series, merge_base_index)).findings
    }

    /// The branch-mode [`AnalysisContext`] the [`branch_changes`] helper runs under.
    fn branch_context(series: &[Series], merge_base_index: Option<usize>) -> AnalysisContext {
        AnalysisContext {
            mode: AnalysisMode::Branch,
            config: AnalysisConfig::default(),
            merge_base_index,
            base_ref_index: merge_base_index,
            tip_index: max_topo_index(series),
        }
    }

    /// Gives branch-mode fixtures the base-ref window production loads separately.
    fn attach_test_base_windows(series: &mut [Series], merge_base_index: Option<usize>) {
        let Some(merge_base_index) = merge_base_index else {
            return;
        };
        let config = AnalysisConfig::default();
        for one in series {
            let base: Vec<&SeriesPoint> = one
                .points
                .iter()
                .filter(|point| !point.dirty && point.topo_index <= merge_base_index)
                .collect();
            let base = recent_commits(&base, config.compare_window);
            one.base_window = test_base_levels(&base);
        }
    }

    /// A branch-mode fixture of `kind`: a [`base_run`] at `base` followed by
    /// `branch_points` commits at `branch`, the first of which sits just past
    /// [`base_merge_base`].
    fn branch_over_base_of_kind(
        base: f64,
        branch: f64,
        branch_points: usize,
        kind: MetricKind,
    ) -> Series {
        let mut points = base_run(base);
        points.extend(
            (0..branch_points)
                .map(|offset| (MIN_SERIES_POINTS.saturating_add(offset), branch, false)),
        );
        let mut series = placed_series_of_kind(&points, kind);
        attach_test_base_windows(slice::from_mut(&mut series), Some(base_merge_base()));
        series
    }

    /// A branch-mode fixture on an instruction count: a [`base_run`] at `base` followed
    /// by `branch_points` commits at `branch`, the first of which sits just past
    /// [`base_merge_base`].
    fn branch_over_base(base: f64, branch: f64, branch_points: usize) -> Series {
        branch_over_base_of_kind(base, branch, branch_points, MetricKind::InstructionCount)
    }

    /// A branch fixture whose base window contains an old level followed by the
    /// current one, then one context run point.
    fn branch_after_base_shift(old: f64, current: f64, current_len: usize, context: f64) -> Series {
        let old_len = COMPARE_WINDOW.checked_sub(current_len).unwrap();
        let mut points: Vec<(usize, f64, bool)> =
            (0..old_len).map(|index| (index, old, false)).collect();
        points.extend(
            (0..current_len).map(|offset| (old_len.checked_add(offset).unwrap(), current, false)),
        );
        points.push((COMPARE_WINDOW, context, false));
        let mut series = placed_series(&points);
        attach_test_base_windows(
            slice::from_mut(&mut series),
            Some(shifted_base_merge_base()),
        );
        series
    }

    /// The merge-base for [`branch_after_base_shift`].
    fn shifted_base_merge_base() -> usize {
        COMPARE_WINDOW - 1
    }

    /// Where the branch detector starts comparing a shifted base fixture.
    fn shifted_base_regime_start(series: &Series) -> usize {
        let mut series = series.clone();
        attach_test_base_windows(
            slice::from_mut(&mut series),
            Some(shifted_base_merge_base()),
        );
        let levels: Vec<f64> = series.base_window.iter().map(|level| level.value).collect();
        let spans = level_spans(&levels);
        let config = AnalysisConfig::default();
        current_base_regime_start(&series, &spans, &config, config.branch_practical_relative)
    }

    /// A base-branch run of [`MIN_SERIES_POINTS`] commits whose levels alternate
    /// ±`wobble` around `value`, so the window carries genuine between-commit scatter
    /// while its mean stays exactly `value`.
    fn wobbling_base_run(value: f64, wobble: f64) -> Vec<(usize, f64, bool)> {
        const {
            assert!(
                MIN_SERIES_POINTS.is_multiple_of(2),
                "an odd window would not centre on the level it wobbles around"
            );
        }
        (0..MIN_SERIES_POINTS)
            .map(|index| {
                let offset = if index % 2 == 0 { -wobble } else { wobble };
                (index, value + offset, false)
            })
            .collect()
    }

    #[test]
    fn branch_mode_flags_a_late_regression_against_the_base() {
        // A flat base at 100, then a branch that sits at 130.
        let series = branch_over_base(100.0, 130.0, 3);
        let finding = only(branch_changes(&[series], Some(base_merge_base())));
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.latest, 130.0);
    }

    #[test]
    fn branch_mode_reports_the_context_state_after_an_intermediate_change() {
        // The branch first improved (80) then regressed (130): only the context commit
        // lands in the base, so we report the context state (worse than the 100 base)
        // and attribute nothing to the branch's own intermediate history.
        let mut points = base_run(100.0);
        points.extend([
            (MIN_SERIES_POINTS, 80.0, false),
            (MIN_SERIES_POINTS + 1, 80.0, false),
            (MIN_SERIES_POINTS + 2, 130.0, false),
        ]);
        let finding = only(branch_changes(
            &[placed_series(&points)],
            Some(base_merge_base()),
        ));
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.latest, 130.0);
    }

    #[test]
    fn branch_mode_is_silent_when_the_branch_matches_the_base() {
        let series = branch_over_base(100.0, 100.0, 3);
        assert!(branch_changes(&[series], Some(base_merge_base())).is_empty());
    }

    #[test]
    fn branch_mode_reports_an_improvement_over_the_base() {
        // Branch mode reports both directions; only history is regressions-only.
        let series = branch_over_base(100.0, 70.0, 3);
        let finding = only(branch_changes(&[series], Some(base_merge_base())));
        assert_eq!(finding.direction, Direction::Improvement);
        assert!(!finding.is_regression());
        assert_eq!(finding.latest, 70.0);
    }

    #[test]
    fn branch_mode_below_the_absolute_floor_is_suppressed() {
        // A quantized context run 4 counts above a small base (60 -> 64) clears the 5%
        // branch relative floor (6.7%) and the residual gate, but not the absolute
        // floor of 5, so it is suppressed. Without the gate this single-quantum-scale
        // move would flag on the pull request. Dropping the floor far below the move
        // admits it again, so the floor is the sole reason for the silence.
        let series = branch_over_base(60.0, 64.0, 3);
        assert!(
            evaluate_branch(
                &series,
                &AnalysisConfig::default(),
                &mut GateLog::disabled()
            )
            .is_none()
        );
        let permissive = AnalysisConfig {
            practical_absolute_count: 0.1,
            ..AnalysisConfig::default()
        };
        assert!(evaluate_branch(&series, &permissive, &mut GateLog::disabled()).is_some());
    }

    #[test]
    fn branch_mode_floors_the_scatter_at_the_metric_quantum() {
        // A count moves in whole units, so a base window can repeat one integer and
        // have a sample standard deviation of exactly zero. The metric's quantum — one
        // count — stands in for that missing scatter, which is the only reason a
        // verdict can be formed at all: with the quantum set to zero the standard
        // error collapses and the same unmistakable 30-count move yields nothing.
        let config = AnalysisConfig::default();
        assert_eq!(
            stats::sample_std_dev(&[100.0; MIN_SERIES_POINTS]),
            Some(0.0),
            "the base window must be perfectly flat for this to test the floor"
        );
        let series = branch_over_base(100.0, 130.0, 1);
        assert!(evaluate_branch(&series, &config, &mut GateLog::disabled()).is_some());
        let without_quantum = AnalysisConfig {
            scatter_floor_count: 0.0,
            ..config
        };
        assert!(evaluate_branch(&series, &without_quantum, &mut GateLog::disabled()).is_none());
    }

    #[test]
    fn branch_mode_reports_a_series_that_starts_allocating() {
        // Code that allocated nothing and now allocates 48 bytes an iteration: the base
        // window is ten commits of exactly zero, so its scatter is zero and only the
        // one-byte quantum keeps the move judgeable. It is a full-scale move against a
        // zero baseline, 48 bytes clears the one-byte absolute floor, and the floored
        // scatter puts the context run 45.8 standard errors out
        // (48 / (1 * sqrt(1 + 1/10))), so it is reported decisively. Removing the
        // quantum silences it, which is exactly the regression shape this floor exists
        // for.
        let series = branch_over_base_of_kind(0.0, 48.0, 1, MetricKind::AllocatedBytes);
        let config = AnalysisConfig::default();
        let candidate = evaluate_branch(&series, &config, &mut GateLog::disabled())
            .expect("48 bytes is a move");
        assert_eq!(candidate.finding.direction, Direction::Regression);
        assert_eq!(candidate.finding.latest, 48.0);
        assert!(candidate.bh_p < config.change_alpha, "{}", candidate.bh_p);
        let without_quantum = AnalysisConfig {
            scatter_floor_alloc: 0.0,
            ..config
        };
        assert!(evaluate_branch(&series, &without_quantum, &mut GateLog::disabled()).is_none());
    }

    #[test]
    fn branch_mode_is_silent_when_a_timing_base_carries_no_scatter() {
        // Time has no quantum — a stored time is a regression slope over a run's
        // iterations, not a counted unit — so a base window that repeats one value
        // leaves nothing to place the context run against and the standard error is
        // degenerate. A doubling from 20 ns to 40 ns clears every floor and is still not
        // reported: the degenerate case fails silent rather than manufacturing
        // certainty. The same move against a base that does scatter is reported, so the
        // flat base is the sole reason for the silence.
        let flat = branch_over_base_of_kind(20.0, 40.0, 1, MetricKind::WallTime);
        let config = AnalysisConfig::default();
        assert!(evaluate_branch(&flat, &config, &mut GateLog::disabled()).is_none());

        let mut points = wobbling_base_run(20.0, 0.2);
        points.push((MIN_SERIES_POINTS, 40.0, false));
        let scattering = placed_series_of_kind(&points, MetricKind::WallTime);
        assert!(evaluate_branch(&scattering, &config, &mut GateLog::disabled()).is_some());
    }

    #[test]
    fn branch_mode_is_silent_for_a_sub_nanosecond_timing_move() {
        // A benchmark measuring 2.49 ns an iteration whose context run reads 3.12 ns.
        // That is a 25% move on a base scattering by only 0.05 ns, so every statistical
        // gate passes decisively (the context run sits 11.4 standard errors out) — yet
        // the move itself spans 0.63 ns, under the one-nanosecond floor below which a
        // timing move is not worth acting on, so nothing is reported. Lowering that
        // floor admits it, which pins the absolute floor as the sole reason for the
        // silence.
        let mut points = wobbling_base_run(2.49, 0.05);
        points.push((MIN_SERIES_POINTS, 3.12, false));
        let series = placed_series_of_kind(&points, MetricKind::WallTime);
        let config = AnalysisConfig::default();
        assert!(evaluate_branch(&series, &config, &mut GateLog::disabled()).is_none());
        let permissive = AnalysisConfig {
            practical_absolute_time: 0.1,
            ..config
        };
        assert!(evaluate_branch(&series, &permissive, &mut GateLog::disabled()).is_some());
    }

    #[test]
    fn branch_mode_reports_a_small_timing_regression_a_one_nanosecond_scatter_floor_would_hide() {
        // A 20 ns benchmark whose base scatters by 0.2 ns from commit to commit,
        // regressing by 8% (1.6 ns). Against its own scatter the context run sits 7.2
        // standard errors out (1.6 / (0.2108 * sqrt(1 + 1/10))) and the move clears the
        // one-nanosecond absolute floor, so it is reported. Flooring the scatter at that
        // same nanosecond instead would put the context run only 1.5 standard errors out
        // — p = 0.16, comfortably inside the interval — and hide the regression entirely,
        // which is what makes the quantum and the magnitude floor separate quantities.
        let mut points = wobbling_base_run(20.0, 0.2);
        points.push((MIN_SERIES_POINTS, 21.6, false));
        let series = placed_series_of_kind(&points, MetricKind::WallTime);
        let config = AnalysisConfig::default();
        let candidate = evaluate_branch(&series, &config, &mut GateLog::disabled())
            .expect("an 8% move on a quiet base is detectable");
        assert_eq!(candidate.finding.direction, Direction::Regression);
        assert!(candidate.bh_p < config.change_alpha, "{}", candidate.bh_p);
        let floored = AnalysisConfig {
            scatter_floor_time: config.practical_absolute_time,
            ..config
        };
        assert!(evaluate_branch(&series, &floored, &mut GateLog::disabled()).is_none());
    }

    #[test]
    fn branch_finding_reports_the_move_from_the_centre_its_test_used() {
        // The prediction interval places the context run against the base window's
        // *mean*, so the magnitude the finding reports must be measured from that same
        // centre. A window of nine commits at 100 and one at 140 has a mean of 104 and
        // a median of 100, which a context run at 200 turns into a reported move of 96
        // (92.3%) rather than 100 (100%) — the median would describe a move the p-value
        // never tested.
        let mut points = base_run(100.0);
        points[MIN_SERIES_POINTS - 1] = (MIN_SERIES_POINTS - 1, 140.0, false);
        points.push((MIN_SERIES_POINTS, 200.0, false));
        let series = placed_series(&points);
        let levels: Vec<f64> = points
            .get(..MIN_SERIES_POINTS)
            .unwrap()
            .iter()
            .map(|&(_, value, _)| value)
            .collect();
        assert_eq!(stats::mean(&levels), Some(104.0));
        assert_eq!(stats::median(&levels), Some(100.0));

        let candidate = evaluate_branch(
            &series,
            &AnalysisConfig::default(),
            &mut GateLog::disabled(),
        )
        .expect("a doubling against a settled base is a finding");
        assert_eq!(candidate.finding.baseline, 104.0);
        assert_eq!(candidate.finding.delta, 96.0);
        assert_eq!(candidate.finding.relative_delta, 96.0 / 104.0);
    }

    #[test]
    fn branch_mode_is_silent_for_a_benchmark_new_on_the_branch() {
        // Every point is past the merge-base: no base-side baseline to compare to.
        let series = placed_series(&[
            (MIN_SERIES_POINTS, 130.0, false),
            (MIN_SERIES_POINTS + 1, 130.0, false),
            (MIN_SERIES_POINTS + 2, 130.0, false),
        ]);
        assert!(branch_changes(&[series], Some(base_merge_base())).is_empty());
    }

    #[test]
    fn branch_mode_admits_a_dirty_snapshot_at_the_merge_base_context() {
        // The merge-base is the context run; a dirty snapshot there is the branch
        // side, while the clean runs at the same and earlier commits are the base.
        let mut points = base_run(100.0);
        points.extend([
            (base_merge_base(), 130.0, true),
            (base_merge_base(), 130.0, true),
            (base_merge_base(), 130.0, true),
        ]);
        let finding = only(branch_changes(
            &[placed_series(&points)],
            Some(base_merge_base()),
        ));
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.latest, 130.0);
    }

    #[test]
    fn branch_mode_interval_disjoint_uses_base_window_intervals() {
        let series = wall_branch_series_with_intervals(130.0, 20.0, 20.0);
        let mut log = GateLog::recording();

        assert!(evaluate_branch(&series, &AnalysisConfig::default(), &mut log).is_none());
        assert_eq!(
            log.declined_by_stage(GateStage::Branch),
            Some(Gate::IntervalDisjoint)
        );
    }

    #[test]
    fn branch_mode_noise_band_uses_base_window_intervals() {
        let series = wall_branch_series_with_intervals(130.0, 20.0, 0.1);
        let mut log = GateLog::recording();

        assert!(evaluate_branch(&series, &AnalysisConfig::default(), &mut log).is_none());
        assert_eq!(
            log.declined_by_stage(GateStage::Branch),
            Some(Gate::IntervalNoiseBand)
        );
        assert_eq!(
            gate_value(&log, Gate::IntervalNoiseBand),
            Some((30.0, 40.0))
        );
    }

    fn wall_branch_series_with_intervals(
        context: f64,
        base_half_width: f64,
        context_half_width: f64,
    ) -> Series {
        let mut values: Vec<f64> = wobbling_base_run(100.0, BASE_WOBBLE)
            .into_iter()
            .map(|(_, value, _)| value)
            .collect();
        values.push(context);
        let mut intervals = vec![base_half_width; MIN_SERIES_POINTS];
        intervals.push(context_half_width);
        series_with(&values, MetricKind::WallTime, &intervals)
    }

    #[test]
    fn branch_finding_stamps_the_newest_base_window_index() {
        // The comparison base is the newest base-side point actually compared
        // against, which for a gapless base run is the merge-base itself.
        let series = branch_over_base(100.0, 130.0, 3);
        let finding = only(branch_changes(&[series], Some(base_merge_base())));
        assert_eq!(finding.comparison_base_index, Some(base_merge_base()));
    }

    #[test]
    fn branch_comparison_base_index_lags_when_recent_base_data_is_missing() {
        // The merge-base is three commits newer than this series' newest base-side
        // run, so its comparison base lags the merge-base.
        let lagging_merge_base = MIN_SERIES_POINTS + 2;
        let mut points = base_run(100.0);
        points.extend([
            (lagging_merge_base + 1, 130.0, false),
            (lagging_merge_base + 2, 130.0, false),
            (lagging_merge_base + 3, 130.0, false),
        ]);
        let finding = only(branch_changes(
            &[placed_series(&points)],
            Some(lagging_merge_base),
        ));
        assert_eq!(finding.comparison_base_index, Some(base_merge_base()));
    }

    #[test]
    fn history_finding_has_no_comparison_base_index() {
        // History mode has no single comparison base, so the field stays `None`.
        let series = series_of(&step_values(100.0, 130.0));
        let finding = only(changes(slice::from_ref(&series)));
        assert_eq!(finding.comparison_base_index, None);
    }

    /// The `(topo_index, value)` pairs of a finding's compact chart series.
    fn chart_pairs(finding: &Finding) -> Vec<(usize, f64)> {
        finding
            .series
            .iter()
            .map(|point| (point.topo_index, point.value))
            .collect()
    }

    #[test]
    fn history_chart_series_maps_every_observation_and_targets_the_context() {
        // History mode keeps the series compact and 1:1 — every observation becomes one
        // chart point carrying its real topo index — and stamps the analyzed context
        // commit as the trailing-fill target so a lagging series can render its "no newer
        // data" gap.
        let values = step_values(100.0, 130.0);
        let series = series_of(&values);
        let finding = only(changes(slice::from_ref(&series)));
        assert_eq!(
            chart_pairs(&finding),
            values
                .iter()
                .copied()
                .enumerate()
                .collect::<Vec<(usize, f64)>>(),
        );
        // `changes` analyses up to the last observation.
        assert_eq!(finding.chart_base_ref, Some(values.len() - 1));
        // Detection is unaffected by carrying the topology through.
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.latest, 130.0);
    }

    #[test]
    fn history_chart_series_preserves_interior_topology_gaps() {
        // Data-less commits between observations survive as a jump in topo index, which
        // the renderer turns into gap columns: the two regimes are separated by a run of
        // commits carrying no data.
        let gap = MIN_SERIES_POINTS;
        let mut points: Vec<(usize, f64, bool)> =
            (0..MIN_REGIME).map(|index| (index, 100.0, false)).collect();
        points.extend((0..MIN_REGIME).map(|index| (gap + index, 130.0, false)));
        let series = placed_series(&points);
        let finding = only(changes(slice::from_ref(&series)));
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(
            chart_pairs(&finding)
                .iter()
                .map(|&(topo, _)| topo)
                .collect::<Vec<_>>(),
            points.iter().map(|&(topo, _, _)| topo).collect::<Vec<_>>(),
            "the interior topo gap is preserved for the renderer to draw",
        );
    }

    #[test]
    fn history_chart_base_ref_is_the_analyzed_context_beyond_the_last_observation() {
        // When analysis reaches commits newer than the last observation, the
        // trailing-fill target is that context commit, so the chart shows the lag as a
        // trailing gap — the visual form of the "lagged history" warning.
        let series = series_of(&step_values(100.0, 130.0));
        let context = AnalysisContext {
            mode: AnalysisMode::History,
            config: AnalysisConfig::default(),
            merge_base_index: None,
            base_ref_index: None,
            tip_index: 20,
        };
        let finding = only(find_changes(slice::from_ref(&series), &context).findings);
        assert_eq!(finding.chart_base_ref, Some(20));
    }

    /// The chart series a branch fixture built by [`branch_over_base`] must collapse
    /// to: every base column, then one context column at `merge_base + 1`.
    fn expected_branch_chart(base: f64, branch: f64) -> Vec<(usize, f64)> {
        let mut expected: Vec<(usize, f64)> =
            (0..MIN_SERIES_POINTS).map(|index| (index, base)).collect();
        expected.push((MIN_SERIES_POINTS, branch));
        expected
    }

    #[test]
    fn branch_chart_series_collapses_interior_commits_onto_the_context() {
        // Branch mode drops every interior branch commit and represents the branch by a
        // single context column at merge_base + 1 carrying the judged latest.
        let series = branch_over_base(100.0, 130.0, 3);
        let finding = only(branch_changes(&[series], Some(base_merge_base())));
        assert_eq!(
            finding.chart_base_ref, None,
            "the context commit is the always-present last column, so no trailing fill"
        );
        assert_eq!(chart_pairs(&finding), expected_branch_chart(100.0, 130.0));
        let context = finding
            .series
            .last()
            .expect("the context column is present");
        assert_eq!(
            context.topo_index, MIN_SERIES_POINTS,
            "the context commit is remapped to merge_base + 1"
        );
        assert_eq!(
            context.value, finding.latest,
            "the context column carries the judged latest, not a raw observation"
        );
    }

    #[test]
    fn branch_chart_series_is_unchanged_by_extra_interior_branch_commits() {
        // Interior branch commits contribute zero columns, so a branch that detoured
        // (improved, then regressed) collapses to the same compact chart series as one
        // that went straight to the context value — the base and context state being
        // equal.
        let straight = branch_over_base(100.0, 130.0, 1);
        let mut detour_points = base_run(100.0);
        detour_points.extend([
            (MIN_SERIES_POINTS, 80.0, false),
            (MIN_SERIES_POINTS + 1, 80.0, false),
            (MIN_SERIES_POINTS + 2, 130.0, false),
        ]);
        let straight_finding = only(branch_changes(&[straight], Some(base_merge_base())));
        let detour_finding = only(branch_changes(
            &[placed_series(&detour_points)],
            Some(base_merge_base()),
        ));
        assert_eq!(
            chart_pairs(&straight_finding),
            chart_pairs(&detour_finding),
            "interior branch commits must not change the collapsed chart series"
        );
        assert_eq!(
            chart_pairs(&straight_finding),
            expected_branch_chart(100.0, 130.0)
        );
    }

    #[test]
    fn branch_chart_series_keeps_stale_base_context_after_regime_narrowing() {
        // Detection compares against only the current base regime after the split, but
        // the chart remains historical context: the stale 200-level base commits are
        // still present before the current 100-level regime and the context run.
        let series = branch_after_base_shift(200.0, 100.0, MIN_REGIME, 130.0);
        let finding = only(branch_changes(&[series], Some(shifted_base_merge_base())));
        let mut expected: Vec<(usize, f64)> = (0..(COMPARE_WINDOW - MIN_REGIME))
            .map(|index| (index, 200.0))
            .collect();
        expected.extend((COMPARE_WINDOW - MIN_REGIME..COMPARE_WINDOW).map(|index| (index, 100.0)));
        expected.push((COMPARE_WINDOW, 130.0));
        assert_eq!(chart_pairs(&finding), expected);
        assert_eq!(finding.baseline, 100.0);
    }

    #[test]
    fn history_does_not_reflag_a_blessed_step() {
        // The unblessed step from 100 to 130 is a change point.
        let mut values = vec![100.0; MIN_REGIME];
        values.extend(std::iter::repeat_n(130.0, MIN_SERIES_POINTS));
        let series = series_of(&values);
        assert_eq!(only(changes(slice::from_ref(&series))).latest, 130.0);

        // Blessing the post-step level re-baselines the series: the active window
        // begins at the first elevated point, leaving a full-length but flat 130
        // regime to judge, which no longer moves.
        let mut blessed = series;
        blessed.active_start = MIN_REGIME;
        blessed.blessing = Some(Blessing {
            commit: "abcdef0123456789".to_owned(),
            commit_time: Some(Timestamp::from_second(3).unwrap()),
        });
        judged_but_silent(&[blessed]);
    }

    #[test]
    fn history_stamps_blessing_provenance_on_a_finding() {
        // Pre-blessing history (100) is retained for charting but excluded from
        // detection; a real step *after* the blessed baseline (130 -> 160) still
        // flags, and the finding carries the blessing provenance and full series.
        let values = three_regimes(100.0, 130.0, 160.0);
        let mut series = series_of(&values);
        series.active_start = MIN_REGIME;
        series.blessing = Some(Blessing {
            commit: "abcdef0123456789cafe".to_owned(),
            commit_time: Some(Timestamp::from_second(3).unwrap()),
        });
        let finding = only(changes(&[series]));
        assert_eq!(finding.baseline, 130.0);
        assert_eq!(finding.latest, 160.0);
        // The full series, including the pre-blessing prefix, is restored for
        // charting...
        assert_eq!(finding.series.len(), values.len());
        // ...and the blessing provenance is recorded.
        assert_eq!(finding.blessed_at.as_deref(), Some("abcdef012345"));
        assert_eq!(
            finding.blessed_commit_time.as_deref(),
            Some("1970-01-01T00:00:03Z")
        );
    }

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
    /// passing `floor` as the practical relative floor and `config` as the analysis
    /// configuration.
    fn compare_with(
        before: &[SeriesPoint],
        after: &[SeriesPoint],
        floor: f64,
        config: &AnalysisConfig,
    ) -> Option<Candidate> {
        let series = wall_series(&[100.0], 1.0);
        let before_refs: Vec<&SeriesPoint> = before.iter().collect();
        let after_refs: Vec<&SeriesPoint> = after.iter().collect();
        compare_samples(
            &series,
            &before_refs,
            &after_refs,
            config,
            floor,
            None,
            &mut GateLog::disabled(),
        )
    }

    /// Compares the `before` and `after` samples with the default configuration.
    fn compare(before: &[SeriesPoint], after: &[SeriesPoint], floor: f64) -> Option<Candidate> {
        compare_with(before, after, floor, &AnalysisConfig::default())
    }

    /// The amount a fixture base window wobbles around its level from commit to
    /// commit, in the metric's own units.
    ///
    /// The [`compare_with`] fixtures run on a wall-time series, whose scatter is not
    /// floored at all (time has no quantum), so a base window repeating one value
    /// leaves the prediction interval no distribution to place the context run in. Real
    /// timing series never repeat a value, so the window alternates by this much instead.
    /// A full window's sample standard deviation is then
    /// `BASE_WOBBLE * sqrt(n / (n - 1))`, far below every move these tests exercise.
    const BASE_WOBBLE: f64 = 0.2;

    /// A base window of the fewest commit levels branch mode will compare against,
    /// each a single run near `value` with confidence half-width `half`.
    ///
    /// Successive commits alternate ±[`BASE_WOBBLE`] around `value`, and the window
    /// holds an even number of commits, so its mean is exactly `value`.
    fn base_window(value: f64, half: f64) -> Vec<SeriesPoint> {
        const {
            assert!(
                MIN_SERIES_POINTS.is_multiple_of(2),
                "an odd window would not centre on the level it wobbles around"
            );
        }
        let specs: Vec<(f64, f64)> = (0..MIN_SERIES_POINTS)
            .map(|index| {
                let offset = if index % 2 == 0 {
                    -BASE_WOBBLE
                } else {
                    BASE_WOBBLE
                };
                (value + offset, half)
            })
            .collect();
        pts(&specs)
    }

    #[test]
    fn compare_samples_at_the_practical_floor_is_not_suppressed() {
        // The relative move (0.03) is exactly the floor: the `relative < floor` gate
        // must be a strict `<` (a `<=`/`==` mutant would suppress it). The move then
        // clears the noise floor (delta 3 > 2 * 0.5) and the prediction interval.
        let before = base_window(100.0, 0.5);
        let after = pts(&[(103.0, 0.5)]);
        assert!(compare(&before, &after, 3.0 / 100.0).is_some());
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "asserts a strict `<` at the exact recorded p-value, which needs bit-identical \
                  recomputation of the rank statistic; Miri's float nondeterminism perturbs it"
    )]
    fn compare_samples_significance_is_a_strict_boundary() {
        // Branch significance is a strict `<` against the prediction-interval p-value.
        // Take that p from the detector's own recording at the default alpha, then set
        // alpha to it: a `<`->`<=` slip that admits the exact boundary turns this
        // reported context run silent.
        let series = wall_series(&[100.0], 1.0);
        let before = base_window(100.0, 0.5);
        let after = pts(&[(108.0, 0.5)]);
        let before_refs: Vec<&SeriesPoint> = before.iter().collect();
        let after_refs: Vec<&SeriesPoint> = after.iter().collect();

        let mut log = GateLog::recording();
        assert!(
            compare_samples(
                &series,
                &before_refs,
                &after_refs,
                &AnalysisConfig::default(),
                0.03,
                None,
                &mut log,
            )
            .is_some(),
            "the 8% move must report at the default alpha"
        );
        let (p, _) = gate_value(&log, Gate::Significance).expect("the significance gate ran");
        let at_boundary = AnalysisConfig {
            change_alpha: p,
            ..AnalysisConfig::default()
        };
        assert!(
            compare_samples(
                &series,
                &before_refs,
                &after_refs,
                &at_boundary,
                0.03,
                None,
                &mut GateLog::disabled(),
            )
            .is_none(),
            "a p-value exactly at alpha must be rejected by the strict gate"
        );
    }

    #[test]
    fn compare_samples_accepts_a_minimal_trailing_regime() {
        // Once branch mode has established that the full base window has enough
        // evidence, an accepted current regime of `min_regime` levels is a valid
        // comparison sample. Keeping the old `min_series_points` threshold here would
        // make the base-regime truncation ineffective.
        let before = pts(&[
            (99.8, 0.5),
            (100.2, 0.5),
            (99.8, 0.5),
            (100.2, 0.5),
            (100.0, 0.5),
        ]);
        let after = pts(&[(130.0, 0.5)]);
        let series = wall_series(&[100.0], 1.0);
        let before_refs: Vec<&SeriesPoint> = before.iter().collect();
        let after_refs: Vec<&SeriesPoint> = after.iter().collect();
        let mut log = GateLog::recording();

        assert!(
            compare_samples(
                &series,
                &before_refs,
                &after_refs,
                &AnalysisConfig::default(),
                0.05,
                None,
                &mut log,
            )
            .is_some()
        );

        let outcome = log
            .entries()
            .iter()
            .find(|entry| entry.stage == GateStage::Branch && entry.gate == Gate::IntervalDisjoint)
            .expect("the interval-disjoint gate must run when both sides carry intervals");
        assert!(
            outcome.passed,
            "disjoint intervals must pass the branch veto"
        );
    }

    #[test]
    fn compare_samples_suppresses_a_significant_move_with_overlapping_intervals() {
        // A move far outside the base's prediction interval, but the branch side's
        // confidence interval is so wide that it overlaps the base's, so the change is
        // rejected. Deleting the `!` in the interval-overlap guard would let it
        // through.
        let before = base_window(100.0, 2.0);
        let after = pts(&[(130.0, 60.0); 5]);
        let series = wall_series(&[100.0], 1.0);
        let before_refs: Vec<&SeriesPoint> = before.iter().collect();
        let after_refs: Vec<&SeriesPoint> = after.iter().collect();
        let mut log = GateLog::recording();

        assert!(
            compare_samples(
                &series,
                &before_refs,
                &after_refs,
                &AnalysisConfig::default(),
                0.05,
                None,
                &mut log,
            )
            .is_none()
        );

        let outcome = log
            .entries()
            .iter()
            .find(|entry| entry.stage == GateStage::Branch && entry.gate == Gate::IntervalDisjoint)
            .expect("the interval-disjoint gate must run when both sides carry intervals");
        assert!(
            !outcome.passed,
            "overlapping intervals must suppress the branch move"
        );
    }

    #[test]
    fn compare_samples_confidence_tracks_the_strength_of_the_evidence() {
        // Branch confidence is `1 - p` from the prediction interval, so it must move
        // with the size of the move rather than being pinned to a constant: the same
        // base window judges a 3% move less confidently than an 8% one. Both stay
        // below 1 (a mutated `1 + p` / `1 / p` would clamp to 1) and neither lands on
        // `1 - change_alpha`, the placeholder confidence a fixed p-value would give.
        let before = base_window(100.0, 0.5);
        let modest = compare(&before, &pts(&[(103.0, 0.5)]), 0.03).unwrap();
        let large = compare(&before, &pts(&[(108.0, 0.5)]), 0.03).unwrap();
        assert!(modest.finding.confidence < large.finding.confidence);
        assert!(large.finding.confidence < 1.0);
        assert!(modest.finding.confidence > 1.0 - AnalysisConfig::default().change_alpha);
    }

    #[test]
    fn compare_samples_at_the_measurement_noise_floor_is_suppressed() {
        // The branch noise band uses the median half-width across the base window and
        // context run together. The base window supplies the majority half-width here,
        // so a context-only implementation would let this through.
        let before = base_window(100.0, 20.0);
        let after = pts(&[(130.0, 0.1); 5]);
        let series = wall_series(&[100.0], 1.0);
        let before_refs: Vec<&SeriesPoint> = before.iter().collect();
        let after_refs: Vec<&SeriesPoint> = after.iter().collect();
        let mut log = GateLog::recording();

        assert!(
            compare_samples(
                &series,
                &before_refs,
                &after_refs,
                &AnalysisConfig::default(),
                0.05,
                None,
                &mut log,
            )
            .is_none()
        );
        assert_eq!(
            log.declined_by_stage(GateStage::Branch),
            Some(Gate::IntervalNoiseBand)
        );
        assert_eq!(
            gate_value(&log, Gate::IntervalNoiseBand),
            Some((30.0, 40.0))
        );

        // Half a unit beyond the base-derived band clears it and is reported.
        let after = pts(&[(140.5, 0.1); 5]);
        assert!(compare(&before, &after, 0.05).is_some());
    }

    #[test]
    fn compare_samples_suppresses_a_context_run_inside_a_bimodal_base() {
        // A base that alternates between two levels (~10 and ~30) from commit to
        // commit. A context run landing on the upper level moves the median by 10, but
        // that is well inside the base's own commit-to-commit scatter, so the
        // prediction interval refuses it — even with every scatter-based veto
        // relaxed, which pins the prediction interval as the sole reason for the
        // silence. A context run clear of *both* levels is reported.
        let mut specs = Vec::new();
        for _ in 0..MIN_REGIME {
            specs.push((10.0, 0.5));
            specs.push((30.0, 0.5));
        }
        let before = pts(&specs);
        let on_the_upper_level = pts(&[(30.0, 0.5)]);
        assert!(compare(&before, &on_the_upper_level, 0.05).is_none());
        let permissive = AnalysisConfig {
            residual_noise_multiple: 0.0,
            branch_noise_multiple: 0.0,
            ..AnalysisConfig::default()
        };
        assert!(compare_with(&before, &on_the_upper_level, 0.05, &permissive).is_none());
        assert!(compare(&before, &pts(&[(60.0, 0.5)]), 0.05).is_some());
    }

    #[test]
    fn latest_context_run_returns_only_the_newest_commit_run() {
        // Two branch commits (topo 3 and topo 5). Only the newest commit's latest
        // run is returned — the context commit is what a merge lands in the base, and a
        // series carries at most one run per commit.
        let series = placed_series(&[(3, 100.0, false), (5, 130.0, false)]);
        let latest = latest_context_run(&series.points, 5);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].topo_index, 5);
    }

    #[test]
    fn latest_context_run_prefers_the_newest_dirty_snapshot_over_the_clean_context_run() {
        // The context commit (topo 5) has a committed clean run plus two dirty snapshots
        // taken on top of it. Only the newest dirty snapshot is returned: it is the
        // single state a merge would land, and mixing runs would blur states.
        let series = placed_series(&[
            (3, 100.0, false),
            (5, 130.0, false),
            (5, 131.0, true),
            (5, 132.0, true),
        ]);
        let latest = latest_context_run(&series.points, 5);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].topo_index, 5);
        assert!(latest[0].dirty);
        assert_eq!(latest[0].value, 132.0);
    }

    #[test]
    fn latest_context_run_of_an_empty_branch_is_empty() {
        assert!(latest_context_run(&[], 0).is_empty());
    }

    #[test]
    fn drift_at_the_practical_floor_is_flagged_with_real_confidence() {
        // A steady climb whose relative drift (0.36) is exactly the floor: the
        // floor gate must be a strict `<`, not a `<=`. Its confidence is 1 - p with
        // p > 0, so a mutated `1 + p` / `1 / p` would clamp to 1.
        let series = series_of(&ramp(100.0, 4.0, MIN_SERIES_POINTS));
        let config = AnalysisConfig {
            practical_relative: 36.0 / 100.0,
            ..AnalysisConfig::default()
        };
        let candidate = evaluate_drift(
            &series,
            &values_of(&series),
            &config,
            &mut GateLog::disabled(),
        )
        .unwrap();
        assert_eq!(candidate.finding.method, FindingMethod::Drift);
        assert_eq!(candidate.finding.relative_delta, config.practical_relative);
        assert!(candidate.finding.confidence < 1.0);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "asserts a strict `<` at the exact recorded p-value, which needs bit-identical \
                  recomputation of the rank statistic; Miri's float nondeterminism perturbs it"
    )]
    fn drift_significance_is_a_strict_boundary() {
        // The Mann–Kendall gate is a strict `<`: a trend whose p-value lands exactly on
        // `drift_alpha` is rejected. Take that p from the detector's own recording at the
        // default alpha, then set alpha to it — so a `<`->`<=` slip that admits the
        // boundary turns this reported ramp silent.
        let series = series_of(&ramp(100.0, 4.0, MIN_SERIES_POINTS));
        let mut log = GateLog::recording();
        assert!(
            evaluate_drift(
                &series,
                &values_of(&series),
                &AnalysisConfig::default(),
                &mut log
            )
            .is_some(),
            "the ramp must report drift at the default alpha"
        );
        let (p, _) = gate_value(&log, Gate::Significance).expect("the significance gate ran");
        let at_boundary = AnalysisConfig {
            drift_alpha: p,
            ..AnalysisConfig::default()
        };
        assert!(
            evaluate_drift(
                &series,
                &values_of(&series),
                &at_boundary,
                &mut GateLog::disabled()
            )
            .is_none(),
            "a trend p-value exactly at drift_alpha must be rejected"
        );
    }

    #[test]
    fn drift_below_the_absolute_floor_is_suppressed() {
        // An upward drift on a quantized metric that gains one count every second
        // commit, totalling only 4.5 counts across the fitted line. Its relative move
        // (4.5%) clears the relative floor and the trend is significant, so disabling
        // the absolute floor admits it; the default floor of 5 is the gate that
        // suppresses it.
        let series = series_of(&staircase(100.0, MIN_SERIES_POINTS));
        let without_absolute_floor = AnalysisConfig {
            practical_absolute_count: 0.0,
            ..AnalysisConfig::default()
        };
        assert!(
            evaluate_drift(
                &series,
                &values_of(&series),
                &without_absolute_floor,
                &mut GateLog::disabled()
            )
            .is_some()
        );
        assert!(
            evaluate_drift(
                &series,
                &values_of(&series),
                &AnalysisConfig::default(),
                &mut GateLog::disabled()
            )
            .is_none()
        );
    }

    #[test]
    fn drift_at_the_absolute_floor_is_flagged() {
        // One more commit on the same staircase carries the fitted line to exactly 5
        // counts, which clears the absolute floor and is flagged, pinning the gate's
        // `>=` boundary.
        let series = series_of(&staircase(100.0, MIN_SERIES_POINTS + 1));
        let candidate = evaluate_drift(
            &series,
            &values_of(&series),
            &AnalysisConfig::default(),
            &mut GateLog::disabled(),
        )
        .unwrap();
        assert_eq!(candidate.finding.method, FindingMethod::Drift);
        assert_eq!(candidate.finding.delta, 5.0);
    }

    #[test]
    fn noisy_drift_within_the_measurement_noise_floor_is_suppressed() {
        // The same climb on a noisy engine, but the endpoints (delta 36) do not
        // separate by more than the confidence half-width (20) times the default
        // `drift_noise_multiple`: jitter, not a trend. The band must be that product (a
        // `+` mutant lowers the floor to 22 and would flag it).
        let series = wall_series(&ramp(100.0, 4.0, MIN_SERIES_POINTS), 20.0);
        assert!(
            evaluate_drift(
                &series,
                &values_of(&series),
                &AnalysisConfig::default(),
                &mut GateLog::disabled()
            )
            .is_none()
        );
    }

    #[test]
    fn the_default_drift_noise_multiple_is_the_named_constant() {
        assert_eq!(
            AnalysisConfig::default().drift_noise_multiple,
            DRIFT_NOISE_MULTIPLE
        );
    }

    #[test]
    fn lowering_the_drift_noise_multiple_admits_a_drift_the_default_suppresses() {
        // The trend the default band withholds: a total movement of 36 against a
        // confidence half-width of 20. The move sits between one and two half-widths,
        // so it is the multiple alone that decides — which is what makes the multiple a
        // policy a caller can set rather than a constant of the detector.
        let series = wall_series(&ramp(100.0, 4.0, MIN_SERIES_POINTS), 20.0);
        let permissive = AnalysisConfig {
            drift_noise_multiple: 1.0,
            ..AnalysisConfig::default()
        };
        let mut log = GateLog::recording();
        let candidate = evaluate_drift(&series, &values_of(&series), &permissive, &mut log);

        assert_eq!(
            candidate.map(|found| found.finding.method),
            Some(FindingMethod::Drift)
        );
        assert_eq!(
            gate_value(&log, Gate::IntervalNoiseBand),
            Some((36.0, 20.0))
        );

        let mut log = GateLog::recording();
        assert!(
            evaluate_drift(
                &series,
                &values_of(&series),
                &AnalysisConfig::default(),
                &mut log
            )
            .is_none()
        );
        assert_eq!(log.declined_by(), Some(Gate::IntervalNoiseBand));
        assert_eq!(
            gate_value(&log, Gate::IntervalNoiseBand),
            Some((36.0, 40.0))
        );
    }

    #[test]
    fn drift_within_its_own_residual_scatter_is_suppressed() {
        // A significant upward trend (100 -> 167.5) that scatters about its Theil-Sen
        // line. Under the default residual multiple the total move dwarfs that
        // scatter and is flagged as drift; a deliberately high multiple lifts the
        // noise band above the move, so only the residual gate rejects it (the
        // length, Mann-Kendall, and practical-floor gates still pass).
        let series = series_of(&[
            100.0, 110.0, 109.0, 120.0, 130.0, 140.0, 139.0, 150.0, 160.0, 170.0,
        ]);
        assert!(
            evaluate_drift(
                &series,
                &values_of(&series),
                &AnalysisConfig::default(),
                &mut GateLog::disabled()
            )
            .is_some()
        );
        let config = AnalysisConfig {
            residual_noise_multiple: 1000.0,
            ..AnalysisConfig::default()
        };
        assert!(
            evaluate_drift(
                &series,
                &values_of(&series),
                &config,
                &mut GateLog::disabled()
            )
            .is_none()
        );
    }

    #[test]
    fn drift_needs_at_least_the_minimum_points() {
        // The length gate is `n < drift_min_points`: a series one point short is
        // rejected outright, while a series of exactly that length is still evaluated
        // (so a gate mutated to reject the longer series instead is caught).
        let config = AnalysisConfig {
            practical_relative: 20.0 / 100.0,
            ..AnalysisConfig::default()
        };
        let short = series_of(&ramp(100.0, 4.0, DRIFT_MIN_POINTS - 1));
        assert!(
            evaluate_drift(
                &short,
                &values_of(&short),
                &config,
                &mut GateLog::disabled()
            )
            .is_none()
        );
        let long = series_of(&ramp(100.0, 4.0, DRIFT_MIN_POINTS));
        assert!(
            evaluate_drift(&long, &values_of(&long), &config, &mut GateLog::disabled()).is_some()
        );
    }

    #[test]
    fn analysis_mode_wire_names() {
        assert_eq!(AnalysisMode::History.as_str(), "history");
        assert_eq!(AnalysisMode::Branch.as_str(), "branch");
    }

    #[test]
    fn history_reports_regressions_only() {
        let context = AnalysisContext {
            mode: AnalysisMode::History,
            config: AnalysisConfig::default(),
            merge_base_index: None,
            base_ref_index: None,
            tip_index: 0,
        };
        // A drift watch over the base branch is one-directional: improvement over time
        // is the expected background there, so only a worsening is a finding.
        assert!(context.keeps(Direction::Regression));
        assert!(!context.keeps(Direction::Improvement));
    }

    #[test]
    fn reports_improvements_reflects_the_mode() {
        let context = |mode| AnalysisContext {
            mode,
            config: AnalysisConfig::default(),
            merge_base_index: None,
            base_ref_index: None,
            tip_index: 0,
        };
        // History is the regressions-only drift watch; branch compares both
        // directions.
        assert!(!context(AnalysisMode::History).reports_improvements());
        assert!(context(AnalysisMode::Branch).reports_improvements());
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
        // comparison is dropped before any significance test.
        let before = base_window(100.0, 0.5);
        let after = pts(&[(101.0, 0.5)]);
        assert!(compare(&before, &after, 0.05).is_none());
    }

    #[test]
    fn compare_samples_suppresses_a_move_the_prediction_interval_cannot_confirm() {
        // A base window whose commit levels are mostly flat but include one outlier
        // each way. The median-based gates see no scatter at all — the residual
        // collapses to zero and the median confidence intervals read as disjoint — but
        // the prediction interval carries the sample standard deviation those outliers
        // inflate, so a 20% move is not yet surprising and is suppressed. Base-side
        // outliers erring toward silence is the intended behaviour; a move large
        // enough to stand clear of that scatter is still reported.
        let mut specs = vec![(100.0, 0.5); MIN_SERIES_POINTS - 2];
        specs.push((70.0, 0.5));
        specs.push((130.0, 0.5));
        let before = pts(&specs);
        assert!(compare(&before, &pts(&[(120.0, 0.5)]), 0.05).is_none());
        assert!(compare(&before, &pts(&[(160.0, 0.5)]), 0.05).is_some());
    }

    #[test]
    fn commit_levels_collapses_repeated_runs_to_the_commit_median() {
        // A `--best-of` sweep gives one commit several runs. They share a build and a
        // runner, so they are one observation, not several: each commit contributes
        // its own median and nothing more.
        let series = placed_series(&[
            (0, 100.0, false),
            (0, 110.0, false),
            (0, 120.0, false),
            (1, 200.0, false),
            (1, 210.0, false),
        ]);
        let points: Vec<&SeriesPoint> = series.points.iter().collect();
        assert_eq!(commit_levels(&points), vec![110.0, 205.0]);
    }

    #[test]
    fn commit_levels_leaves_one_run_per_commit_untouched() {
        // With a single run per commit the levels are the values, in order.
        let series = placed_series(&[(0, 100.0, false), (1, 130.0, false), (2, 90.0, false)]);
        let points: Vec<&SeriesPoint> = series.points.iter().collect();
        assert_eq!(commit_levels(&points), vec![100.0, 130.0, 90.0]);
    }

    #[test]
    fn commit_levels_separates_a_dirty_snapshot_from_its_commit() {
        // A dirty snapshot measures uncommitted work, so it is a different state from
        // the clean run at the same commit and must not be folded into it.
        let series = placed_series(&[(0, 100.0, false), (0, 140.0, true), (0, 160.0, true)]);
        let points: Vec<&SeriesPoint> = series.points.iter().collect();
        assert_eq!(commit_levels(&points), vec![100.0, 150.0]);
    }

    #[test]
    fn commit_levels_of_no_points_is_empty() {
        assert!(commit_levels(&[]).is_empty());
    }

    #[test]
    fn prediction_interval_p_needs_two_base_points() {
        // Scatter cannot be estimated from fewer than two observations, so there is no
        // interval to place the new observation in.
        assert_eq!(prediction_interval_p(&[], 130.0, 1.0), None);
        assert_eq!(prediction_interval_p(&[100.0], 130.0, 1.0), None);
        assert!(prediction_interval_p(&[100.0, 100.0], 130.0, 1.0).is_some());
    }

    #[test]
    fn prediction_interval_p_is_a_student_t_prediction_interval() {
        // The statistic is the textbook prediction interval for one new observation:
        // `t = (latest - mean) / (sd * sqrt(1 + 1/n))` on `n - 1` degrees of freedom.
        // Every term matters — dropping the `1 +`, dividing by the widening factor
        // instead of multiplying, or miscounting the degrees of freedom all leave a
        // plausible-looking p-value that silently retunes every branch-mode verdict,
        // so the closed form is restated here and checked exactly. A flat base makes
        // the floored scatter exactly `sd` and the mean exactly the base level,
        // leaving the formula as the only unknown.
        let base = [100.0; MIN_SERIES_POINTS];
        let n = count_to_f64(base.len());
        let expected = stats::student_t_two_sided_p(3.0 / (1.0 + 1.0 / n).sqrt(), n - 1.0);
        let actual = prediction_interval_p(&base, 103.0, 1.0).unwrap();
        // The tolerance absorbs the last-place differences Miri's floating point
        // emulation introduces while staying orders of magnitude tighter than any
        // algebraic drift in the formula, all of which move the p-value by 1e-3
        // or more.
        assert!((actual - expected).abs() < 1e-12, "{actual} vs {expected}");
        // Cross-checked against an independent implementation (SciPy's
        // `2 * scipy.stats.t.sf(|t|, 9)`), so the closed form above cannot be wrong
        // in the same way the implementation might be.
        assert!(
            (expected - 0.018_768_522_060_029_7).abs() < 1e-12,
            "{expected}"
        );
    }

    #[test]
    fn prediction_interval_p_floors_the_scatter_at_the_metric_quantum() {
        // A count moves in whole units, so a base window can repeat one integer and
        // have an observed scatter of exactly zero. Without a floor the standard error
        // collapses and no verdict can be formed at all; with the metric's quantum of
        // one count standing in, the standard error is `1 * sqrt(1 + 1/10) = 1.0488`,
        // so a five-count move sits 4.77 standard errors out and is decisive while a
        // one-count move sits 0.95 out and is not yet surprising.
        let flat = [1000.0; MIN_SERIES_POINTS];
        assert_eq!(stats::sample_std_dev(&flat), Some(0.0));
        assert_eq!(prediction_interval_p(&flat, 1005.0, 0.0), None);
        let alpha = AnalysisConfig::default().change_alpha;
        let large = prediction_interval_p(&flat, 1005.0, 1.0).unwrap();
        assert!(large < alpha, "{large}");
        let at_the_quantum = prediction_interval_p(&flat, 1001.0, 1.0).unwrap();
        assert!(at_the_quantum >= alpha, "{at_the_quantum}");
    }

    #[test]
    fn branch_mode_counts_base_commits_not_base_runs() {
        // `--best-of` repeats give many base *points* but few base *commits*, and
        // repeated runs of one commit are one observation, not independent evidence.
        // Only the commit levels count toward the minimum, so a base one commit level
        // short is refused however many runs those commits contribute, while the same
        // move over a full set of one-run commits is judged.
        let levels = MIN_SERIES_POINTS - 1;
        let mut points: Vec<(usize, f64, bool)> = Vec::new();
        for commit in 0..levels {
            points.push((commit, 100.0, false));
            points.push((commit, 100.0, false));
        }
        points.push((levels, 130.0, false));
        assert!(
            branch_changes(&[placed_series(&points)], Some(levels - 1)).is_empty(),
            "repeated runs of too few commits are not enough base commit levels"
        );
        assert!(
            !branch_changes(
                &[branch_over_base(100.0, 130.0, 1)],
                Some(base_merge_base())
            )
            .is_empty(),
            "the same move over one run per commit is judged"
        );
    }

    #[test]
    fn branch_mode_fills_its_comparison_window_with_levels_not_runs() {
        // The comparison window is measured in levels, so a repository whose commits
        // each carry several stored runs still reaches a full window. Were it
        // measured in points, those repeats would crowd it out — here two runs per
        // commit would halve it to below `MIN_SERIES_POINTS` and silence branch mode
        // on this repository permanently, however long its history grew.
        const RUNS_PER_COMMIT: usize = 2;
        let base_commits = COMPARE_WINDOW + MIN_SERIES_POINTS;
        const {
            assert!(
                COMPARE_WINDOW < MIN_SERIES_POINTS * RUNS_PER_COMMIT,
                "a point-measured window has to fall short for this to prove anything"
            );
        }
        let mut points: Vec<(usize, f64, bool)> = Vec::new();
        for commit in 0..base_commits {
            for _ in 0..RUNS_PER_COMMIT {
                // Repeated clean measurements of one tree: several stored runs, one level.
                points.push((commit, 100.0, false));
            }
        }
        points.push((base_commits, 130.0, false));
        let finding = only(branch_changes(
            &[placed_series(&points)],
            Some(base_commits - 1),
        ));
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.baseline, 100.0);
    }

    #[test]
    fn recent_commits_yields_at_most_the_window_in_levels() {
        // The window's contract is stated in levels, not points: whatever mix of
        // repeated runs a history carries, windowing it and then collapsing it must
        // never yield more levels than the window asks for, and must not yield fewer
        // when the history can supply them. Measured in points this fails as soon as
        // one commit contributes more than one run.
        let mut specs: Vec<(usize, f64, bool)> = Vec::new();
        for commit in 0..8_usize {
            specs.push((commit, 100.0, false));
            // Two dirty re-measurements of the same tree, which collapse together.
            specs.push((commit, 101.0, true));
            specs.push((commit, 102.0, true));
        }
        let series = placed_series(&specs);
        let borrowed: Vec<&SeriesPoint> = series.points.iter().collect();
        let available = commit_levels(&borrowed).len();
        assert_eq!(available, 16, "a clean level and a dirty level per commit");
        for window in 0..=(available + 4) {
            assert_eq!(
                commit_levels(&recent_commits(&borrowed, window)).len(),
                window.min(available),
                "window of {window}"
            );
        }
    }

    #[test]
    fn recent_commits_windows_whole_groups() {
        // A group's runs travel together: the window never splits one group across
        // its boundary, since a partial group would weight that level by however
        // many of its runs happened to fall inside.
        let series = placed_series(&[
            (0, 1.0, false),
            (1, 2.0, false),
            (1, 3.0, true),
            (1, 4.0, true),
        ]);
        let borrowed: Vec<&SeriesPoint> = series.points.iter().collect();
        let windowed = recent_commits(&borrowed, 1);
        assert_eq!(
            windowed.iter().map(|point| point.value).collect::<Vec<_>>(),
            vec![3.0, 4.0],
            "the newest group is the pair of dirty snapshots, kept whole"
        );
    }

    #[test]
    fn branch_mode_judges_against_only_the_recent_base_window() {
        // The base is compared over its last `compare_window` commits, not its whole
        // history: an older, higher regime beyond that window must not drag the
        // baseline up. Here the recent window sits at 100 and the older half at 200,
        // so a context run at 130 is a regression against the recent level; judged
        // against the whole base it would read as an improvement instead.
        let mut points: Vec<(usize, f64, bool)> = (0..COMPARE_WINDOW)
            .map(|index| (index, 200.0, false))
            .collect();
        points.extend((0..COMPARE_WINDOW).map(|index| (COMPARE_WINDOW + index, 100.0, false)));
        let merge_base = 2 * COMPARE_WINDOW - 1;
        points.push((merge_base + 1, 130.0, false));
        let finding = only(branch_changes(&[placed_series(&points)], Some(merge_base)));
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.direction, Direction::Regression);
    }

    #[test]
    fn branch_mode_compares_against_the_current_base_regime_after_a_shift() {
        // The recent base window itself spans a real 200 -> 100 level shift. The
        // context run is a clear regression against the current 100 level; judging the
        // mixed window as one population would inflate the scatter and hide it.
        let series = branch_after_base_shift(200.0, 100.0, MIN_REGIME, 130.0);
        assert_eq!(
            shifted_base_regime_start(&series),
            COMPARE_WINDOW - MIN_REGIME
        );
        let finding = only(branch_changes(&[series], Some(shifted_base_merge_base())));
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.baseline, 100.0);
        assert_eq!(finding.latest, 130.0);
    }

    #[test]
    fn branch_mode_uses_the_most_recent_qualifying_base_split() {
        // The base window contains two real shifts, 100 -> 200 -> 100. The current
        // regime is the final 100-level tail, so a context run at 130 is a regression
        // against that tail rather than an improvement against the older 200-level
        // regime.
        let mut points: Vec<(usize, f64, bool)> =
            (0..MIN_REGIME).map(|index| (index, 100.0, false)).collect();
        points.extend(
            (0..(COMPARE_WINDOW - 2 * MIN_REGIME))
                .map(|offset| (MIN_REGIME + offset, 200.0, false)),
        );
        points.extend(
            (0..MIN_REGIME).map(|offset| (COMPARE_WINDOW - MIN_REGIME + offset, 100.0, false)),
        );
        points.push((COMPARE_WINDOW, 130.0, false));
        let series = placed_series(&points);
        assert_eq!(
            shifted_base_regime_start(&series),
            COMPARE_WINDOW - MIN_REGIME
        );
        let finding = only(branch_changes(&[series], Some(shifted_base_merge_base())));
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.baseline, 100.0);
    }

    #[test]
    fn branch_mode_does_not_split_a_stationary_bimodal_base_window() {
        // The recorded bimodal series, cut where its recent base window happens to end
        // on five consecutive low-mode commits, with a context run at the high mode the
        // series reaches on roughly half of its commits. A change-point search over that
        // window proposes the split at the start of those five, and the trailing regime's
        // standard deviation is a fraction of the mixed window's — so accepting the split
        // turns an ordinary value into a large, near-certain regression.
        //
        // The stricter base-split separation floor is the sole reason for the silence:
        // relaxing it to the reporting floor accepts the split and manufactures the
        // finding, which is what makes the two floors different policies rather than one
        // value written twice.
        let series = bimodal_branch_probe();
        let merge_base = STATIONARY_BIMODAL_BASE.checked_sub(1).unwrap();
        assert_eq!(bimodal_branch_regime_start(&series), 0);
        assert!(branch_changes(slice::from_ref(&series), Some(merge_base)).is_empty());

        let permissive = AnalysisConfig {
            min_base_split_separation: AnalysisConfig::default().min_regime_separation,
            ..AnalysisConfig::default()
        };
        let manufactured = evaluate_branch(&series, &permissive, &mut GateLog::disabled()).unwrap();
        assert_eq!(manufactured.finding.direction, Direction::Regression);
        assert!(
            manufactured.finding.relative_delta > 0.5,
            "{:?}",
            manufactured.finding
        );
    }

    /// The recorded bimodal series cut to the base window that exposes a spurious
    /// trailing regime, followed by one branch commit at the recording's high mode.
    fn bimodal_branch_probe() -> Series {
        let mut points: Vec<(usize, f64, bool)> = STATIONARY_BIMODAL_NOISE
            .get(..STATIONARY_BIMODAL_BASE)
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, &value)| (index, value, false))
            .collect();
        points.push((STATIONARY_BIMODAL_BASE, STATIONARY_BIMODAL_HIGH, false));
        placed_series_of_kind(&points, MetricKind::WallTime)
    }

    /// Where the branch detector starts comparing [`bimodal_branch_probe`].
    fn bimodal_branch_regime_start(series: &Series) -> usize {
        let borrowed: Vec<&SeriesPoint> = series.points.iter().collect();
        let base = recent_commits(
            borrowed.get(..STATIONARY_BIMODAL_BASE).unwrap(),
            COMPARE_WINDOW,
        );
        let spans = commit_level_spans(&base);
        let config = AnalysisConfig::default();
        current_base_regime_start(series, &spans, &config, config.branch_practical_relative)
    }

    fn base_split_qualifies(before: f64, after: f64, before_len: usize, after_len: usize) -> bool {
        let mut levels = vec![before; before_len];
        levels.extend(std::iter::repeat_n(after, after_len));
        base_regime_split_qualifies(
            &series_of(&[]),
            &levels,
            before_len,
            &AnalysisConfig::default(),
            AnalysisConfig::default().branch_practical_relative,
        )
    }

    /// Whether a split onto a perfectly flat trailing regime qualifies for `kind`.
    fn flat_regime_split_qualifies(kind: MetricKind) -> bool {
        let mut levels = vec![100.0; MIN_REGIME];
        levels.extend(std::iter::repeat_n(130.0, MIN_REGIME));
        let config = AnalysisConfig::default();
        base_regime_split_qualifies(
            &series_with(&[], kind, &[]),
            &levels,
            MIN_REGIME,
            &config,
            config.branch_practical_relative,
        )
    }

    #[test]
    fn a_split_onto_a_flat_regime_needs_a_metric_quantum() {
        // A trailing regime with no scatter of its own leaves the prediction interval
        // nothing but the metric's quantum to work from. A counted metric has one, so the
        // narrowed sample still yields a verdict and the split stands. A timing metric has
        // none, so narrowing would replace a comparison the whole window can still make
        // with silence, and the split is refused.
        assert!(flat_regime_split_qualifies(MetricKind::InstructionCount));
        assert!(!flat_regime_split_qualifies(MetricKind::WallTime));
    }

    #[test]
    fn branch_mode_reports_a_timing_move_over_a_flat_base_step() {
        // A timing series whose base window steps up partway and then holds exactly
        // flat. Narrowing onto that trailing regime would leave the prediction interval
        // without scatter or quantum and silence the series entirely, so the window stays
        // whole and the elevated context run is still reported.
        let mut points: Vec<(usize, f64, bool)> = (0..(COMPARE_WINDOW - MIN_REGIME - 2))
            .map(|i| (i, 100.0, false))
            .collect();
        points
            .extend(((COMPARE_WINDOW - MIN_REGIME - 2)..COMPARE_WINDOW).map(|i| (i, 118.0, false)));
        points.push((COMPARE_WINDOW, 139.0, false));
        let series = placed_series_of_kind(&points, MetricKind::WallTime);
        assert_eq!(shifted_base_regime_start(&series), 0);
        let finding = only(branch_changes(
            slice::from_ref(&series),
            Some(shifted_base_merge_base()),
        ));
        assert_eq!(finding.direction, Direction::Regression);
    }

    #[test]
    fn base_regime_split_accepts_a_minimal_before_regime() {
        // The pre-split side may hold exactly `min_regime` levels; rejecting equality
        // would prevent branch mode from recovering as soon as enough current base
        // commits exist.
        assert!(base_split_qualifies(
            100.0,
            130.0,
            MIN_REGIME,
            MIN_REGIME + 1
        ));
    }

    #[test]
    fn base_regime_split_rejects_a_short_before_regime() {
        assert!(!base_split_qualifies(
            100.0,
            130.0,
            MIN_REGIME - 1,
            MIN_REGIME + 1
        ));
    }

    #[test]
    fn base_regime_split_rejects_a_short_after_regime() {
        assert!(!base_split_qualifies(
            100.0,
            130.0,
            MIN_REGIME + 1,
            MIN_REGIME - 1
        ));
    }

    #[test]
    fn base_regime_split_accepts_the_relative_floor_boundary() {
        // A base-side step exactly at the branch practical floor is large enough to
        // define the current regime, matching the reporting floor's strictness.
        assert!(base_split_qualifies(100.0, 105.0, MIN_REGIME, MIN_REGIME));
    }

    #[test]
    fn base_regime_split_rejects_below_the_relative_floor() {
        // This step clears the absolute floor but not the branch relative floor, so it
        // is too small to justify discarding history.
        assert!(!base_split_qualifies(200.0, 209.0, MIN_REGIME, MIN_REGIME));
    }

    #[test]
    fn branch_mode_recovers_at_the_minimum_trailing_regime() {
        // With `min_regime = 5`, five current-level base commits are enough to form the
        // comparison regime, while four are not enough to accept the split and still
        // leave the mixed window too unsettled to report the same context run. Both a
        // halving of the base level and a much shallower step recover at the same
        // commit, so the recovery lag is set by the regime floor rather than by the size
        // of the base step.
        for (old, current, context) in [(200.0, 100.0, 130.0), (120.0, 100.0, 115.0)] {
            let exactly_enough = branch_after_base_shift(old, current, MIN_REGIME, context);
            assert_eq!(
                shifted_base_regime_start(&exactly_enough),
                COMPARE_WINDOW - MIN_REGIME,
                "old={old} current={current}"
            );
            assert_eq!(
                only(branch_changes(
                    &[exactly_enough],
                    Some(shifted_base_merge_base())
                ))
                .baseline,
                current,
                "old={old} current={current}"
            );

            let one_short = branch_after_base_shift(old, current, MIN_REGIME - 1, context);
            assert_eq!(
                shifted_base_regime_start(&one_short),
                0,
                "old={old} current={current}"
            );
            assert!(
                branch_changes(&[one_short], Some(shifted_base_merge_base())).is_empty(),
                "old={old} current={current}"
            );
        }
    }

    #[test]
    fn branch_mode_does_not_split_a_noise_only_base_window() {
        // A realistic 2-3% deterministic wobble is not a regime boundary. The helper
        // returning zero proves no suffix was accepted as a current-regime split, and a
        // context run that stays inside the same wobble remains silent.
        let mut points = vec![
            (0, 100.0, false),
            (1, 102.0, false),
            (2, 98.0, false),
            (3, 101.0, false),
            (4, 99.0, false),
            (5, 103.0, false),
            (6, 97.0, false),
            (7, 100.0, false),
            (8, 102.0, false),
            (9, 98.0, false),
            (10, 101.0, false),
            (11, 99.0, false),
            (12, 103.0, false),
            (13, 97.0, false),
            (14, 100.0, false),
            (15, 102.0, false),
        ];
        points.push((COMPARE_WINDOW, 101.0, false));
        let series = placed_series(&points);
        assert_eq!(shifted_base_regime_start(&series), 0);
        assert!(branch_changes(&[series], Some(shifted_base_merge_base())).is_empty());
    }

    #[test]
    fn branch_mode_pure_noise_batch_produces_no_findings() {
        // Forty independently seeded branch-mode series: a stationary wall-time level with
        // realistic scatter in the base window, and a context run drawn from that same
        // distribution. Nothing changed in any of them, so none may become a finding — and
        // in particular none may become one through a base split drawn across noise.
        //
        // The scatter is wide enough, relative to the branch floors, that a base split can
        // clear them: the controls below manufacture findings out of this very data by
        // narrowing where the gate declines to. The silence is therefore the split gate's
        // doing and not the magnitude floors rejecting everything before the split logic is
        // ever consulted.
        let batch = pure_noise_branch_batch();
        let context = branch_context(&batch, Some(NOISE_MERGE_BASE));
        let config = AnalysisConfig::default();

        let detection = find_changes(&batch, &context);
        assert_eq!(detection.census.judged(), batch.len());
        assert_eq!(detection.findings.len(), 0);

        // Stationary noise now and then throws a trailing run that is rank-separated from
        // everything before it, and such a run is not distinguishable from a level shift, so
        // the gate is entitled to narrow there. What it may not do is narrow as a rule.
        let narrowed = batch
            .iter()
            .filter(|series| noise_regime_start(series, &config) != 0)
            .count();
        assert!(
            narrowed * NOISE_SPLIT_SHARE <= batch.len(),
            "narrowing fired on {narrowed} of {} pure-noise windows",
            batch.len()
        );

        // Nor may it narrow on a window where narrowing manufactures a report. Those are the
        // windows this fixture exists to guard, so there must be some.
        let weaponisable: Vec<usize> = (0..batch.len())
            .filter(|&index| a_narrowed_split_would_report(batch.get(index).unwrap(), &config))
            .collect();
        assert!(
            !weaponisable.is_empty(),
            "no window in this fixture can be turned into a finding by narrowing, so the \
             magnitude floors rather than the split gate would be producing the silence"
        );
        for index in weaponisable {
            let series = batch.get(index).unwrap();
            assert_eq!(
                noise_regime_start(series, &config),
                0,
                "narrowing fired on pure-noise window {index}, where a narrowed base sample \
                 clears the branch floors and turns the context run into a finding"
            );
        }

        // And the silence is not an artefact of the false discovery rate control: no series
        // raises a candidate in the first place. Relaxing only the base-split separation
        // floor narrows more windows and turns some of them into candidates, which is the
        // failure this fixture stands guard over.
        let candidates = batch
            .iter()
            .filter(|series| evaluate_branch(series, &config, &mut GateLog::disabled()).is_some())
            .count();
        assert_eq!(candidates, 0);

        let permissive = AnalysisConfig {
            min_base_split_separation: 0.0,
            ..AnalysisConfig::default()
        };
        let relaxed = batch
            .iter()
            .filter(|series| noise_regime_start(series, &permissive) != 0)
            .count();
        let manufactured = batch
            .iter()
            .filter(|series| {
                evaluate_branch(series, &permissive, &mut GateLog::disabled()).is_some()
            })
            .count();
        assert!(
            relaxed > narrowed,
            "the fixture cannot expose a noise-driven split"
        );
        assert!(
            manufactured > 0,
            "a noise-driven split on this fixture cannot clear the branch floors, so the \
             floors rather than the split gate would be producing the silence"
        );
    }

    /// The level [`pure_noise_branch_batch`] wobbles around, in nanoseconds.
    ///
    /// High enough that the 5% branch relative floor sits above the 1ns wall-time absolute
    /// floor, so the relative floor is the binding one and the fixture exercises the same
    /// gate ordering a real wall-time benchmark does.
    const NOISE_LEVEL: f64 = 40.0;

    /// The coefficient of variation [`pure_noise_branch_batch`] carries: the upper end of
    /// the band this project's own wall-time benchmarks occupy.
    ///
    /// Wide enough that a value drawn from the band can sit more than 5% away from the mean
    /// of a trailing slice of that same band, which is what lets a base split drawn across
    /// noise clear the branch floors and reach a finding.
    const NOISE_CV: f64 = 0.03;

    /// The merge base of a [`pure_noise_branch_batch`] series: its last base-side commit.
    const NOISE_MERGE_BASE: usize = COMPARE_WINDOW - 1;

    /// The reciprocal of the share of [`pure_noise_branch_batch`] windows on which the base
    /// split gate may narrow.
    ///
    /// Stationary noise throws the occasional trailing run that is rank-separated from the
    /// values before it, and such a run carries the same evidence a genuine level shift
    /// would, so narrowing there is correct behaviour rather than a defect. It must stay the
    /// exception: an implementation that narrows on a large share of stationary windows is
    /// reading structure into noise.
    const NOISE_SPLIT_SHARE: usize = 8;

    /// Forty stationary wall-time series, each with its own scatter sequence, laid out as
    /// a full base window followed by a single context run.
    fn pure_noise_branch_batch() -> Vec<Series> {
        const BATCH: usize = 40;

        (0..BATCH)
            .map(|index| {
                let name = format!("noise{index}");
                let values =
                    scattered(&[NOISE_LEVEL; COMPARE_WINDOW + 1], NOISE_CV, seed_of(&name));
                let points: Vec<(usize, f64, bool)> = values
                    .iter()
                    .enumerate()
                    .map(|(offset, &value)| (offset, value, false))
                    .collect();
                let mut series = placed_series_of_kind(&points, MetricKind::WallTime);
                attach_test_base_windows(slice::from_mut(&mut series), Some(NOISE_MERGE_BASE));
                series.id = BenchmarkId::new(nonempty![name, "case".to_owned()]);
                series
            })
            .collect()
    }

    /// Where branch mode starts comparing a [`pure_noise_branch_batch`] series.
    fn noise_regime_start(series: &Series, config: &AnalysisConfig) -> usize {
        let borrowed: Vec<&SeriesPoint> = series.points.iter().collect();
        let base = recent_commits(borrowed.get(..COMPARE_WINDOW).unwrap(), COMPARE_WINDOW);
        let spans = commit_level_spans(&base);
        current_base_regime_start(series, &spans, config, config.branch_practical_relative)
    }

    /// Whether some trailing slice of a [`pure_noise_branch_batch`] base window, short of the
    /// whole window, turns the context run into a reported move.
    ///
    /// This is what an implementation that split at an arbitrary index would be doing, so a
    /// window for which this holds is one the split gate has to decline.
    fn a_narrowed_split_would_report(series: &Series, config: &AnalysisConfig) -> bool {
        let borrowed: Vec<&SeriesPoint> = series.points.iter().collect();
        let base = recent_commits(borrowed.get(..COMPARE_WINDOW).unwrap(), COMPARE_WINDOW);
        let context = borrowed.get(COMPARE_WINDOW..).unwrap().to_vec();
        (1..=base.len().saturating_sub(MIN_REGIME)).any(|start| {
            let narrowed = base.get(start..).unwrap().to_vec();
            compare_samples(
                series,
                &narrowed,
                &context,
                config,
                config.branch_practical_relative,
                None,
                &mut GateLog::disabled(),
            )
            .is_some()
        })
    }

    #[test]
    fn branch_mode_does_not_split_a_base_step_below_the_floor() {
        // The base step is rank-significant and well-separated, but it moves only four
        // instruction counts. A move below the metric's absolute floor is too small to
        // justify discarding earlier base history.
        let series = branch_after_base_shift(60.0, 64.0, MIN_REGIME, 64.0);
        assert_eq!(shifted_base_regime_start(&series), 0);
        assert!(branch_changes(&[series], Some(shifted_base_merge_base())).is_empty());
    }

    #[test]
    fn branch_mode_is_silent_when_context_matches_the_current_base_regime() {
        // The detector narrows the base to the current 100-level regime, but a branch
        // context run that agrees with it has no move to report. The unsettled mixed
        // window must not manufacture a finding.
        let series = branch_after_base_shift(200.0, 100.0, MIN_REGIME, 100.0);
        assert_eq!(
            shifted_base_regime_start(&series),
            COMPARE_WINDOW - MIN_REGIME
        );
        assert!(branch_changes(&[series], Some(shifted_base_merge_base())).is_empty());
    }

    #[test]
    fn branch_mode_reports_an_improvement_against_the_current_base_regime() {
        // The same base-regime narrowing works in the improvement direction: the recent
        // base has moved up to 200 and the context run improves it to 150.
        let series = branch_after_base_shift(100.0, 200.0, MIN_REGIME, 150.0);
        assert_eq!(
            shifted_base_regime_start(&series),
            COMPARE_WINDOW - MIN_REGIME
        );
        let finding = only(branch_changes(&[series], Some(shifted_base_merge_base())));
        assert_eq!(finding.direction, Direction::Improvement);
        assert_eq!(finding.baseline, 200.0);
        assert_eq!(finding.latest, 150.0);
    }

    #[test]
    fn branch_confidence_varies_with_the_strength_of_the_evidence() {
        // A branch finding's confidence comes from the prediction interval, so it
        // grows with the size of the move rather than being pinned to a constant.
        // The same flat base judges a 20% move less confidently than a 40% one, and
        // neither lands on `1 - change_alpha`, the fixed confidence a placeholder
        // p-value would report.
        let modest = only(branch_changes(
            &[branch_over_base(100.0, 120.0, 1)],
            Some(base_merge_base()),
        ));
        let large = only(branch_changes(
            &[branch_over_base(100.0, 140.0, 1)],
            Some(base_merge_base()),
        ));
        let placeholder = 1.0 - AnalysisConfig::default().change_alpha;
        assert!(modest.confidence < large.confidence);
        assert!(large.confidence < 1.0);
        assert!((modest.confidence - placeholder).abs() > 1e-9, "{modest:?}");
    }

    #[test]
    fn testability_agrees_with_detection_in_history_mode() {
        // Detection and family membership share one definition, so a series that is
        // not judged raises nothing and a judged one is evaluated on its merits. The
        // census reports the same verdict, so a report can never claim to have judged
        // a series the detectors declined.
        let mut short_values = vec![100.0; MIN_REGIME];
        short_values.extend(std::iter::repeat_n(130.0, MIN_REGIME - 1));
        let short = series_of(&short_values);
        let context = history_context(slice::from_ref(&short));
        assert_eq!(
            testability(&short, &context),
            Testability::Unjudged(UnjudgedReason::TooFewPoints)
        );
        let detection = find_changes(slice::from_ref(&short), &context);
        assert!(detection.findings.is_empty());
        assert_eq!(detection.census.judged(), 0);
        assert_eq!(
            detection.census.reasons().collect::<Vec<_>>(),
            vec![(UnjudgedReason::TooFewPoints, 1)]
        );

        let long = series_of(&step_values(100.0, 130.0));
        let context = history_context(slice::from_ref(&long));
        assert_eq!(testability(&long, &context), Testability::Judged);
        let detection = find_changes(slice::from_ref(&long), &context);
        assert_eq!(detection.findings.len(), 1);
        assert_eq!(detection.census.judged(), 1);
        assert_eq!(detection.census.unjudged(), 0);
    }

    #[test]
    fn a_blessing_that_leaves_too_little_evidence_is_accounted_for_separately() {
        // A blessing re-baselines the series, so only the points after it are evidence.
        // A recent blessing can therefore blind a long series, which is a different
        // (and fixable) situation from a series that is simply new — so the census
        // distinguishes the two rather than lumping both under "too few points".
        let mut series = series_of(&[100.0; MIN_SERIES_POINTS * 2]);
        series.active_start = MIN_SERIES_POINTS.saturating_add(1);
        series.blessing = Some(Blessing {
            commit: "c".repeat(40),
            commit_time: None,
        });
        let context = history_context(slice::from_ref(&series));
        assert_eq!(
            testability(&series, &context),
            Testability::Unjudged(UnjudgedReason::TooFewPointsSinceBlessing)
        );

        // The same series blessed at its very start keeps every point as evidence, so
        // it is judged: it is the truncation that blinds, not the blessing.
        let mut blessed_at_start = series;
        blessed_at_start.active_start = 0;
        let context = history_context(slice::from_ref(&blessed_at_start));
        assert_eq!(
            testability(&blessed_at_start, &context),
            Testability::Judged
        );
    }

    #[test]
    fn testability_agrees_with_detection_in_branch_mode() {
        // Branch mode needs both a branch side to judge and enough base commits to
        // judge it against; either shortfall leaves the series unjudged for its own
        // reason, and detection stays silent in step with that.
        let no_branch_side = [placed_series(&base_run(100.0))];
        let context = AnalysisContext {
            tip_index: base_merge_base().saturating_add(1),
            ..branch_context(&no_branch_side, Some(base_merge_base()))
        };
        assert_eq!(
            testability(&no_branch_side[0], &context),
            Testability::Unjudged(UnjudgedReason::NotMeasuredOnBranch)
        );
        let detection = find_changes(&no_branch_side, &context);
        assert!(detection.findings.is_empty());
        assert_eq!(detection.census.judged(), 0);

        let mut short_base = base_run(100.0);
        short_base.truncate(MIN_SERIES_POINTS - 1);
        short_base.push((MIN_SERIES_POINTS, 130.0, false));
        let short_base = [placed_series(&short_base)];
        let context = branch_context(&short_base, Some(base_merge_base()));
        assert_eq!(
            testability(&short_base[0], &context),
            Testability::Unjudged(UnjudgedReason::TooFewBaseCommits)
        );
        let detection = find_changes(&short_base, &context);
        assert!(detection.findings.is_empty());
        assert_eq!(detection.census.judged(), 0);

        let judged = [branch_over_base(100.0, 130.0, 1)];
        let context = branch_context(&judged, Some(base_merge_base()));
        assert_eq!(testability(&judged[0], &context), Testability::Judged);
        let detection = find_changes(&judged, &context);
        assert_eq!(detection.findings.len(), 1);
        assert_eq!(detection.census.judged(), 1);
    }

    #[test]
    fn the_census_accounts_for_every_series_exactly_once() {
        // The census is only readable as coverage if it is total: whatever mix of
        // judged and unjudged series a pass sees, the tallies must add back up to the
        // series it was handed.
        let mut blessed = series_of(&[100.0; MIN_SERIES_POINTS]);
        blessed.active_start = 1;
        let batch = vec![
            named_series("judged", &step_values(100.0, 130.0)),
            named_series("silent", &[100.0; MIN_SERIES_POINTS]),
            named_series("short", &[100.0; MIN_SERIES_POINTS - 1]),
            named_series("shorter", &[100.0; 1]),
            blessed,
        ];
        let census = find_changes(&batch, &history_context(&batch)).census;
        assert_eq!(census.judged(), 2);
        assert_eq!(census.unjudged(), 3);
        assert_eq!(census.total(), batch.len());
        assert_eq!(
            census.reasons().collect::<Vec<_>>(),
            vec![
                (UnjudgedReason::TooFewPoints, 2),
                (UnjudgedReason::TooFewPointsSinceBlessing, 1),
            ],
            "the breakdown is ordered and sums to the unjudged total"
        );
    }

    #[test]
    fn a_census_absorbs_another_and_ignores_an_empty_tally() {
        // Detection recombines one census per worker chunk, so merging must be total;
        // and the stages that record in bulk pass whatever count they dropped, so a
        // zero must leave no trace of a reason that accounts for nothing.
        let mut left = SeriesCensus::default();
        left.record(Testability::Judged);
        left.record(Testability::Unjudged(UnjudgedReason::TooFewPoints));
        let mut right = SeriesCensus::default();
        right.record(Testability::Judged);
        right.record(Testability::Unjudged(UnjudgedReason::TooFewPoints));
        right.record_unjudged(UnjudgedReason::Ghost, 3);
        right.record_unjudged(UnjudgedReason::NotMeasuredOnBranch, 0);

        left.merge(&right);
        assert_eq!(left.judged(), 2);
        assert_eq!(left.total(), 7);
        assert_eq!(
            left.reasons().collect::<Vec<_>>(),
            vec![
                (UnjudgedReason::Ghost, 3),
                (UnjudgedReason::TooFewPoints, 2),
            ]
        );
    }

    #[test]
    fn a_verdict_calls_itself_judged_only_when_it_is_one() {
        // Detection and the census both branch on this, so an always-true answer
        // would run the detectors on series the census reports as never tested.
        assert!(Testability::Judged.is_judged());
        for reason in UnjudgedReason::ALL {
            assert!(
                !Testability::Unjudged(reason).is_judged(),
                "{reason:?} is not a verdict"
            );
        }
    }

    #[test]
    fn every_unjudged_reason_has_a_distinct_wire_name_and_phrase() {
        // Both renderings of a reason are contracts: the wire name is read by
        // automation and the phrase by a human, so neither may collide.
        let names: Vec<&str> = UnjudgedReason::ALL
            .iter()
            .map(|reason| reason.as_str())
            .collect();
        let phrases: Vec<&str> = UnjudgedReason::ALL
            .iter()
            .map(|reason| reason.describe())
            .collect();
        for (index, name) in names.iter().enumerate() {
            assert!(
                !names[index + 1..].contains(name),
                "duplicate wire name {name}"
            );
        }
        for (index, phrase) in phrases.iter().enumerate() {
            assert!(
                !phrases[index + 1..].contains(phrase),
                "duplicate phrase {phrase}"
            );
        }

        // The declaration order is the reporting order, and a `BTreeMap` census keyed
        // by the derived `Ord` must therefore iterate in the same order.
        let mut census = SeriesCensus::default();
        for reason in UnjudgedReason::ALL.iter().rev() {
            census.record_unjudged(*reason, 1);
        }
        assert_eq!(
            census
                .reasons()
                .map(|(reason, _)| reason)
                .collect::<Vec<_>>(),
            UnjudgedReason::ALL.to_vec()
        );
    }

    #[test]
    fn the_false_discovery_family_is_every_testable_series_not_the_survivors() {
        // The correction divides by the number of hypotheses *tested*, which is the
        // number of testable series — including those that raised nothing. Feeding it
        // only its own survivors would make it a no-op, since every survivor has
        // already cleared `change_alpha`.
        //
        // The stepped series is real but modest, and its rank-test p-value falls
        // between the Benjamini-Hochberg thresholds `(1 / m) * fdr_q` for the two
        // family sizes below, so the family size alone decides its fate. Every batch
        // raises exactly the one candidate; the only thing that differs is whether the
        // silent companions join the family. Flat companions are judged and do count,
        // while companions one point too short are not judged and do not.
        const FAMILY_THAT_REPORTS: usize = 8;
        const FAMILY_THAT_REJECTS: usize = 9;

        let stepped = named_series(
            "stepped",
            &[
                98.0, 100.0, 102.0, 99.0, 101.0, 128.0, 130.0, 132.0, 129.0, 131.0,
            ],
        );
        let stepped_id =
            BenchmarkId::new(nonempty!["stepped".to_owned(), "case".to_owned()]).qualified();
        let flat_companions = |count: usize| {
            (0..count)
                .map(|index| named_series(&format!("flat{index}"), &[100.0; MIN_SERIES_POINTS]))
        };

        let mut small_family = vec![stepped.clone()];
        small_family.extend(flat_companions(FAMILY_THAT_REPORTS - 1));
        assert_eq!(
            only(changes(&small_family)).id.qualified(),
            stepped_id,
            "the candidate clears the threshold this family size sets"
        );

        let mut large_family = vec![stepped.clone()];
        large_family.extend(flat_companions(FAMILY_THAT_REJECTS - 1));
        assert!(
            changes(&large_family).is_empty(),
            "one more silent but testable companion tightens the threshold past it"
        );

        let mut unjudged_batch = vec![stepped];
        unjudged_batch.extend(
            (0..FAMILY_THAT_REJECTS - 1).map(|index| {
                named_series(&format!("short{index}"), &[100.0; MIN_SERIES_POINTS - 1])
            }),
        );
        assert_eq!(
            only(changes(&unjudged_batch)).id.qualified(),
            stepped_id,
            "companions that were never judged must not enlarge the family"
        );
    }

    /// One gate-observation scenario: a history whose evaluation a named gate ends.
    ///
    /// The catalogue holds one scenario per gate family, so the table doubles as an
    /// inventory of the shape of history each gate exists to decline.
    struct DeclinedCase {
        /// How the history reads, quoted back by a failing assertion.
        shape: &'static str,
        /// The series the detectors judge.
        series: Series,
        /// Which detector's chain the scenario makes its claim about. Every history
        /// detector runs on every series, so naming the chain is part of the claim.
        stage: GateStage,
        /// The gate expected to end that chain.
        gate: Gate,
        /// The `(value, threshold)` the gate compared, where both are worth pinning by
        /// hand. `None` where the gate carries no numbers, or where its numbers are a
        /// rank statistic no reader would recompute from the series by eye.
        compared: Option<(f64, f64)>,
    }

    /// The history-mode context a gate-observation scenario runs under.
    fn observed_context(series: &Series) -> AnalysisContext {
        history_context(slice::from_ref(series))
    }

    /// One scenario per gate family, each declining for a different reason.
    fn declined_cases() -> Vec<DeclinedCase> {
        let mut short_after = vec![100.0; MIN_SERIES_POINTS];
        short_after.extend(std::iter::repeat_n(130.0, MIN_REGIME - 1));
        let noisy_step = [
            98.0, 100.0, 102.0, 99.0, 101.0, 128.0, 130.0, 132.0, 129.0, 131.0,
        ];

        vec![
            DeclinedCase {
                shape: "a step with one point too few after it",
                series: series_of(&short_after),
                stage: GateStage::ChangePoint,
                gate: Gate::MinRegime,
                // The shorter regime holds `MIN_REGIME - 1` points against the floor.
                compared: Some((count_to_f64(MIN_REGIME - 1), count_to_f64(MIN_REGIME))),
            },
            DeclinedCase {
                shape: "a flat history with one excursion at its midpoint",
                series: series_of(&{
                    let mut values = [100.0; MIN_SERIES_POINTS];
                    // The excursion splits the history into two regimes of equal length
                    // and identical level, which is the only way a located split can
                    // carry no move at all.
                    values[MIN_REGIME] = 200.0;
                    values
                }),
                stage: GateStage::ChangePoint,
                gate: Gate::NonZeroDelta,
                // Both regimes sit at the same level, so the move is exactly nothing.
                compared: Some((0.0, 0.0)),
            },
            DeclinedCase {
                shape: "a one percent step",
                series: series_of(&step_values(100.0, 101.0)),
                stage: GateStage::ChangePoint,
                gate: Gate::RelativeFloor,
                // 1 unit on a baseline of 100 against the 3% relative floor.
                compared: Some((0.01, PRACTICAL_RELATIVE)),
            },
            DeclinedCase {
                shape: "a four-count step",
                series: series_of(&step_values(60.0, 64.0)),
                stage: GateStage::ChangePoint,
                gate: Gate::AbsoluteFloor,
                // 4 instruction counts against the metric's five-count floor; the same
                // move clears the relative floor, so this is the gate that binds.
                compared: Some((4.0, PRACTICAL_ABSOLUTE_COUNT)),
            },
            DeclinedCase {
                shape: "a step no larger than its own residual scatter",
                series: wall_series(
                    &[
                        1000.0, 1040.0, 1000.0, 1040.0, 1020.0, 1040.0, 1080.0, 1040.0, 1080.0,
                        1060.0,
                    ],
                    40.0,
                ),
                stage: GateStage::ChangePoint,
                gate: Gate::ResidualNoise,
                // A 40-unit move against three times the 20-unit median absolute residual
                // the same two-regime model leaves behind.
                compared: Some((40.0, RESIDUAL_NOISE_MULTIPLE * 20.0)),
            },
            DeclinedCase {
                shape: "a history that oscillates between two levels throughout",
                series: series_of(&STATIONARY_BIMODAL_NOISE),
                stage: GateStage::ChangePoint,
                gate: Gate::RegimeSeparation,
                compared: None,
            },
            DeclinedCase {
                shape: "a clean step under confidence intervals that overlap",
                series: wall_series(&noisy_step, 60.0),
                stage: GateStage::ChangePoint,
                gate: Gate::IntervalDisjoint,
                compared: None,
            },
            DeclinedCase {
                shape: "a flat history, judged as a trend",
                series: series_of(&[100.0; MIN_SERIES_POINTS]),
                stage: GateStage::Drift,
                gate: Gate::Significance,
                // No pair of points is ordered, so the rank test reports no trend at all.
                compared: Some((1.0, AnalysisConfig::default().drift_alpha)),
            },
            DeclinedCase {
                shape: "a climb smaller than the measurement noise band",
                series: wall_series(&ramp(100.0, 4.0, MIN_SERIES_POINTS), 20.0),
                stage: GateStage::Drift,
                gate: Gate::IntervalNoiseBand,
                // A fitted movement of 4 units per point across ten points, against
                // twice the 20-unit half-width every point carries.
                compared: Some((36.0, DRIFT_NOISE_MULTIPLE * 20.0)),
            },
        ]
    }

    #[test]
    fn each_gate_family_declines_the_history_it_exists_for() {
        // The log's whole purpose is to name the gate that ended an evaluation, so every
        // gate family needs a history it is the one to decline — and the reason recorded
        // has to be the reason that actually applied, not merely a plausible one.
        for case in declined_cases() {
            let context = observed_context(&case.series);
            let (finding, log) = evaluate_with_log(&case.series, &context);
            assert!(
                finding.is_none(),
                "{}: expected silence, got {finding:?}",
                case.shape
            );
            assert_eq!(
                log.declined_by_stage(case.stage),
                Some(case.gate),
                "{}: the {} chain declined for the wrong reason",
                case.shape,
                case.stage.label()
            );
        }
    }

    #[test]
    fn a_declining_gate_records_the_numbers_it_compared() {
        // A logged value a reader cannot reproduce from the series is worse than no log
        // at all, so each scenario pins the arithmetic its gate performed.
        for case in declined_cases() {
            let Some((value, threshold)) = case.compared else {
                continue;
            };
            let context = observed_context(&case.series);
            let (_, log) = evaluate_with_log(&case.series, &context);
            let outcome = log
                .entries()
                .iter()
                .find(|entry| entry.stage == case.stage && entry.gate == case.gate)
                .unwrap_or_else(|| panic!("{}: the gate never ran", case.shape));
            assert!(!outcome.passed, "{}", case.shape);
            assert_eq!(outcome.value, Some(value), "{}", case.shape);
            assert_eq!(outcome.threshold, Some(threshold), "{}", case.shape);
        }
    }

    #[test]
    fn a_chain_ends_at_the_gate_that_declined_it() {
        // Gates short-circuit, so a log is a prefix of the chain rather than a survey of
        // it: everything before the declining gate passed and nothing after it ran. A
        // reader who does not know this would misread a missing gate as a passing one.
        for case in declined_cases() {
            let context = observed_context(&case.series);
            let (_, log) = evaluate_with_log(&case.series, &context);
            let chain: Vec<&GateOutcome> = log
                .entries()
                .iter()
                .filter(|entry| entry.stage == case.stage)
                .collect();
            let (last, earlier) = chain.split_last().unwrap();
            assert_eq!(last.gate, case.gate, "{}", case.shape);
            assert!(!last.passed, "{}", case.shape);
            assert!(
                earlier.iter().all(|entry| entry.passed),
                "{}: a gate before the declining one is recorded as failing",
                case.shape
            );
        }
    }

    #[test]
    fn a_reported_change_point_passes_every_gate_in_its_chain() {
        // The complement of the declining scenarios: when a finding is reported, its
        // chain must show the whole gauntlet cleared, in the order the engine applies
        // it. A series without dispersion gives the interval gate nothing to judge, so
        // it abstains rather than recording a verdict it did not reach.
        let series = series_of(&step_values(100.0, 130.0));
        let (finding, log) = evaluate_with_log(&series, &observed_context(&series));
        assert_eq!(finding.unwrap().method, FindingMethod::ChangePoint);
        assert_eq!(log.declined_by_stage(GateStage::ChangePoint), None);
        let chain: Vec<Gate> = log
            .entries()
            .iter()
            .filter(|entry| entry.stage == GateStage::ChangePoint)
            .map(|entry| entry.gate)
            .collect();
        assert_eq!(
            chain,
            vec![
                Gate::SplitLocated,
                Gate::MinRegime,
                Gate::NonZeroDelta,
                Gate::Significance,
                Gate::RelativeFloor,
                Gate::AbsoluteFloor,
                Gate::ResidualNoise,
                Gate::RegimeSeparation,
            ]
        );
        assert!(log.entries().iter().all(|entry| entry.passed));
    }

    #[test]
    fn a_disabled_log_records_nothing() {
        // Production runs with recording off, so the disabled log has to stay empty
        // however much detection is put through it — otherwise every analysis would pay
        // for an observation facility only tests and figures read.
        let mut log = GateLog::disabled();
        for case in declined_cases() {
            let context = observed_context(&case.series);
            let batch = slice::from_ref(&case.series);
            let _ = detect_range(batch, 0..batch.len(), &context, &mut log);
        }
        assert!(log.entries().is_empty());
        assert_eq!(log.declined_by(), None);
    }

    #[test]
    fn recording_the_gates_does_not_change_any_verdict() {
        // The log observes detection; it must not participate in it. Every scenario the
        // suite knows about therefore has to reach the same verdict, field for field,
        // whether or not anyone is watching.
        let mut batch: Vec<(&str, Series)> = declined_cases()
            .into_iter()
            .map(|case| (case.shape, case.series))
            .collect();
        batch.push(("a clean step", series_of(&step_values(100.0, 130.0))));
        batch.push((
            "a steady climb",
            series_of(&ramp(100.0, 4.0, MIN_SERIES_POINTS)),
        ));

        for (shape, series) in batch {
            let context = observed_context(&series);
            let unobserved = find_changes(slice::from_ref(&series), &context).findings;
            let (observed, _) = evaluate_with_log(&series, &context);
            // `Finding` carries no equality contract, so compare the whole rendering
            // rather than a hand-picked subset of fields that could hide a difference.
            assert_eq!(
                format!("{:?}", unobserved.first()),
                format!("{observed:?}"),
                "{shape}"
            );
        }
    }

    /// The named example series, paired with the verdict its documentation claims.
    ///
    /// The examples are the vocabulary the documentation figures are drawn from, so a
    /// documented verdict that no longer holds is a documentation defect this catches.
    fn example_verdicts() -> Vec<ExampleVerdict> {
        vec![
            ExampleVerdict {
                name: "clean_step",
                values: examples::clean_step(),
                reported: Some(FindingMethod::ChangePoint),
            },
            ExampleVerdict {
                name: "slow_ramp",
                values: examples::slow_ramp(),
                reported: Some(FindingMethod::Drift),
            },
            ExampleVerdict {
                name: "blip",
                values: examples::blip(),
                reported: None,
            },
            ExampleVerdict {
                name: "flat_noisy",
                values: examples::flat_noisy(),
                reported: None,
            },
        ]
    }

    /// One named example paired with what history mode is documented to make of it.
    struct ExampleVerdict {
        name: &'static str,
        values: Vec<f64>,
        reported: Option<FindingMethod>,
    }

    #[test]
    fn every_named_example_reaches_the_verdict_it_documents() {
        // The figures and the prose both quote these verdicts, so the series and their
        // documentation are only worth sharing while the engine still agrees with them.
        for case in example_verdicts() {
            let series = timing_series(case.name, &case.values);
            let context = examples::history_context(&series);
            let finding = evaluate_with_log(&series, &context).0;
            assert_eq!(
                finding.as_ref().map(|finding| finding.method),
                case.reported,
                "{} does not reach its documented verdict: {finding:?}",
                case.name
            );
            if case.reported.is_some() {
                assert_eq!(
                    finding.map(|finding| finding.direction),
                    Some(Direction::Regression),
                    "{}",
                    case.name
                );
            }
        }
    }

    /// An example series as a wall-time history whose engine reports no dispersion, so
    /// the only noise in it is the scatter the example itself carries.
    fn timing_series(name: &str, values: &[f64]) -> Series {
        examples::series(name, values, MetricKind::WallTime, 0)
    }

    #[test]
    fn the_shared_example_builder_lays_a_series_out_the_way_detection_expects() {
        // Every documentation figure builds its series through this one function, so its
        // layout is a contract: consecutive topological indices from the requested start,
        // one named commit per point, and no dispersion until a caller asks for it.
        let series = examples::series("demo", &[100.0, 110.0, 120.0], MetricKind::WallTime, 7);
        assert_eq!(series.kind, MetricKind::WallTime);
        assert_eq!(
            series.id.qualified(),
            BenchmarkId::new(nonempty!["demo".to_owned(), "case".to_owned()]).qualified()
        );
        assert_eq!(series.active_start, 0);
        assert!(series.blessing.is_none());
        assert_eq!(
            series
                .points
                .iter()
                .map(|point| point.topo_index)
                .collect::<Vec<_>>(),
            vec![7, 8, 9]
        );
        assert_eq!(series.points[0].object_ordinal, 7);
        assert_eq!(series.points[2].commit.as_deref(), Some("commit9"));
        assert!(series.points.iter().all(|point| !point.dirty
            && point.interval_low.is_none()
            && point.interval_high.is_none()));
        assert_eq!(examples::history_context(&series).tip_index, 9);
    }

    #[test]
    fn attaching_intervals_is_what_lets_an_example_reach_the_interval_gates() {
        // The interval vetoes only run where the engine reports dispersion, so an example
        // that has to demonstrate them must carry intervals — and the width the caller
        // picks is what decides the verdict, which is the whole illustration.
        let bare = examples::series("step", &step_values(100.0, 130.0), MetricKind::WallTime, 0);
        assert!(
            evaluate_with_log(&bare, &examples::history_context(&bare))
                .0
                .is_some()
        );

        let narrow = examples::with_intervals(bare.clone(), 2.0);
        assert_eq!(narrow.points[0].interval_low, Some(98.0));
        assert_eq!(narrow.points[0].interval_high, Some(102.0));
        assert!(
            evaluate_with_log(&narrow, &examples::history_context(&narrow))
                .0
                .is_some()
        );

        let wide = examples::with_intervals(bare, 60.0);
        let (finding, log) = evaluate_with_log(&wide, &examples::history_context(&wide));
        assert!(finding.is_none());
        assert_eq!(
            log.declined_by_stage(GateStage::ChangePoint),
            Some(Gate::IntervalDisjoint)
        );
    }

    #[test]
    fn the_shared_branch_context_judges_the_branch_side_against_the_base() {
        // Branch-mode examples need the same single-call setup history ones have; the
        // merge base is the only thing that distinguishes the two.
        let mut values = vec![100.0; MIN_SERIES_POINTS];
        values.push(130.0);
        let series = examples::with_base_window(
            examples::series("branch", &values, MetricKind::InstructionCount, 0),
            MIN_SERIES_POINTS - 1,
        );
        let context = examples::branch_context(&series, MIN_SERIES_POINTS - 1);
        let finding = only(find_changes(slice::from_ref(&series), &context).findings);
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.latest, 130.0);
    }
}
