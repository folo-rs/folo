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
//! * Surviving candidates then pass a Benjamini–Hochberg false-discovery filter,
//!   taken over every series judged rather than only those that raised a candidate,
//!   so a batch of series does not manufacture spurious findings.
//!
//! A separate slow-[`Drift`](FindingMethod::Drift) finding is raised from a
//! Mann–Kendall trend test plus a Theil–Sen slope, gated by the same practical
//! floor and residual-scatter check, and is suppressed when a single step on the
//! same series already explains at least as much movement.
//!
//! Polarity: every metric is lower-is-better (instruction counts, branch counts,
//! allocations, wall and processor time), so a rise is a
//! [`Direction::Regression`] and a fall is a [`Direction::Improvement`].

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use anyspawn::Spawner;
use cbh_model::{BenchmarkId, DiscriminantSet, MetricKind};
use cbh_stats as stats;
use serde::Serialize;

use crate::detect::parallel::{balanced_chunk_sizes, worker_count};
use crate::detect::{Series, SeriesPoint, noise_gates};

/// Tunable parameters of the engine-aware analysis.
#[derive(Clone, Copy, Debug)]
pub struct AnalysisConfig {
    /// Minimum points each side of a change must have for the step to be trusted
    /// (persistence): a one-off blip on the latest point cannot flag.
    pub min_regime: usize,
    /// Minimum points a series must carry before it is evaluated at all. A shorter
    /// series raises no finding and does not count toward the false-discovery
    /// family, since no split within it can satisfy
    /// [`min_regime`](Self::min_regime) on both sides.
    pub min_series_points: usize,
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
    /// Minimum absolute magnitude, in the metric's own units, a move on an
    /// instruction or branch count must reach. Composed by conjunction with
    /// [`practical_relative`](Self::practical_relative): these counts move in whole
    /// integer units, so at a small baseline a few units of build-layout jitter is a
    /// large *percentage* move that the relative floor alone would let through.
    pub practical_absolute_count: f64,
    /// Minimum absolute magnitude, in nanoseconds, a timing move must reach. A move
    /// of under a nanosecond an iteration is not worth acting on regardless of the
    /// percentage it works out to, so on a benchmark measuring a couple of
    /// nanoseconds an iteration this is the gate that binds rather than the relative
    /// floor.
    pub practical_absolute_time: f64,
    /// Minimum absolute magnitude, in bytes or allocations, an allocation move must
    /// reach. A fraction of a byte or of an allocation cannot happen, so one whole
    /// unit is the smallest move worth reporting and the floor rejects only the
    /// sub-unit moves that amortizing across a run's iterations can manufacture.
    pub practical_absolute_alloc: f64,
    /// Smallest scatter an instruction or branch count can express, in counts. Bounds
    /// the base window's standard deviation from below in branch mode's prediction
    /// interval, so a window that repeats one integer still yields a usable standard
    /// error. See [`scatter_floor_time`](Self::scatter_floor_time) for why this is not
    /// the same quantity as an absolute magnitude floor.
    pub scatter_floor_count: f64,
    /// Smallest scatter a timing metric can express, in nanoseconds — zero, because a
    /// time is a regression slope over a run's iterations and resolves far below a
    /// clock tick.
    ///
    /// A scatter floor is the metric's *quantum*, not a statement about which moves
    /// matter: it exists only to keep a degenerate base window from collapsing the
    /// standard error. Raising it would make every timing series behave as if it
    /// wobbled by that much, imposing an absolute detection threshold in units of the
    /// standard error on top of the
    /// [`practical_absolute_time`](Self::practical_absolute_time) floor that already
    /// decides which timing moves are worth reporting.
    pub scatter_floor_time: f64,
    /// Smallest scatter an allocation metric can express, in bytes or allocations.
    /// Code that allocated nothing gives a base window of zeroes, whose scatter is
    /// exactly zero, and this is what keeps that (real and important) move judgeable.
    pub scatter_floor_alloc: f64,
    /// How many recent base-side points form the level a branch's latest state is
    /// compared against (branch mode).
    pub compare_window: usize,
    /// Minimum relative magnitude a noisy *branch* move must reach. Raised above the
    /// history floor: a feature-branch signal must be high-confidence, since we
    /// would rather miss a small move than cry wolf on a pull request.
    pub branch_practical_relative: f64,
    /// Multiple of the per-measurement noise floor a branch move must exceed where
    /// the engine reports per-point confidence intervals. An additional veto on top
    /// of the prediction-interval test, able only to suppress a candidate.
    pub branch_noise_multiple: f64,
    /// Multiple of a series' own between-commit residual scatter (median absolute
    /// residual of the fitted step or line model) that a move must exceed before it
    /// is trusted. This is the primary, series-intrinsic noise gate applied to every
    /// engine: a clean series has near-zero residual scatter, so any persistent move
    /// clears it, while a jittery series demands a move that stands out above its own
    /// run-to-run wobble. It composes with (and is independent of) the optional
    /// confidence-interval veto available on dispersion-reporting engines.
    pub residual_noise_multiple: f64,
    /// Minimum **probability of superiority** (Mann–Whitney common-language effect
    /// size) the two regimes of a level shift must reach for the shift to be trusted:
    /// the fraction of after-vs-before commit pairs that move in the finding's
    /// direction. It is the *effect-size* companion to the rank test's *significance*
    /// gate, and closes a hole the significance gate cannot: a rank test grows
    /// "significant" with sample size even for two heavily overlapping regimes, so a
    /// long but stationary series that merely oscillates between two levels — noisy
    /// yet stable — otherwise reads as a change-point. A genuine step scores ~1 here;
    /// bimodal jitter scores near ½. Because a move that already clears the residual
    /// gate is well-separated in practice, this only ever *suppresses* a candidate the
    /// median-based gates were fooled by, never creates one.
    pub min_regime_separation: f64,
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
    /// Latest-commit comparison of a feature branch's tip against its base,
    /// ignoring the intermediate commits the branch passed through.
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
    /// First-parent topological index of the analyzed tip commit (the resolved
    /// `--context`/HEAD). History-mode chart building uses it as the trailing-fill
    /// target so a series that stops short of the tip renders the data-less commits
    /// after its last observation as a gap. Consulted only in [`AnalysisMode::History`].
    pub tip_index: usize,
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

/// Why a series was left unjudged.
///
/// A detection pass reaches a verdict only on series carrying enough evidence for
/// their mode's detector; every other series is unjudged for one of these reasons.
/// The set is exhaustive over the ways the analysis declines to test a series, so a
/// silent report can say how much of the suite its silence covers.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum UnjudgedReason {
    /// The benchmark carries no measurement at the analyzed tip commit, so it is no
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
    /// Branch mode: the branch measured nothing for this series, so there is no
    /// branch state to compare against the base.
    NotMeasuredOnBranch,
    /// Branch mode: the comparison window holds fewer than
    /// [`min_series_points`](AnalysisConfig::min_series_points) base-side commits, so
    /// there is no base level to judge the branch against.
    TooFewBaseCommits,
}

impl UnjudgedReason {
    /// Every reason, in reporting order.
    ///
    /// The order runs with the pipeline: what the ghost filter dropped before
    /// detection, then the history-mode shortfalls, then the branch-mode ones.
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
            Self::Ghost => "not measured at the analyzed tip commit",
            Self::TooFewPoints => "with too few points in the analyzed window",
            Self::TooFewPointsSinceBlessing => "with too few points since being blessed",
            Self::NotMeasuredOnBranch => "not measured on the branch",
            Self::TooFewBaseCommits => "with too few base-branch commits to compare against",
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
    pub fn is_judged(self) -> bool {
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
    pub fn merge(&mut self, other: &Self) {
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

    /// The unjudged series broken down by reason, in [`UnjudgedReason::ALL`] order.
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
    /// Commit the change is attributed to, if known.
    pub commit: Option<String>,
    /// Where a recovered spike returned to baseline: set only in history mode on an
    /// inactive finding, naming the commit at which the level came back down. Branch
    /// mode never sets it — it judges the tip commit alone, with no within-branch
    /// flip to attribute.
    pub flipped_at: Option<String>,
    /// Whether the change is still reflected in the latest measured state. An active
    /// finding's current level still differs from baseline; an inactive one has
    /// since recovered (history mode only — branch always looks at the latest
    /// state, so its findings are always active).
    pub active: bool,
    /// Abbreviated commit of the blessing that re-baselined this series, if any.
    pub blessed_at: Option<String>,
    /// Effective (committer) time of the blessed commit, RFC 3339, if blessed.
    pub blessed_commit_time: Option<String>,
    /// The full underlying series, oldest-first. Retained internally so the text and
    /// Markdown reports can draw a chart; it is not part of the machine-readable JSON
    /// contract.
    pub series: Vec<SeriesValue>,
    /// First-parent topological index of the newest base-side point this finding was
    /// actually compared against — the series' *comparison base*. Set only in branch
    /// mode (`None` in history mode, where there is no single comparison base). Internal
    /// cross-crate analysis metadata that lets the analysis measure how far the
    /// comparison base sits behind the merge-base; it is not part of the JSON finding
    /// contract.
    pub comparison_base_index: Option<usize>,
    /// Trailing-fill target for the chart: the first-parent index the charted series
    /// extends to when its last observation stops short of it. `Some(tip_index)` in
    /// history mode, so the data-less commits between the last observation and the
    /// analyzed tip render as a gap; `None` in branch mode, where the tip is the
    /// always-present last column. Chart-only — like [`Finding::series`] it is not part
    /// of the JSON finding contract.
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
fn count_to_f64(count: usize) -> f64 {
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
/// the trailing-fill target, so a series that stops short of the analyzed tip renders
/// the intervening commits as the "no newer data" gap. Branch mode collapses the series
/// (see [`branch_chart_series`]).
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
/// Branch mode judges the branch's tip commit alone, so the chart keeps the base-side
/// points at their real `topo_index`, drops every interior branch commit, and
/// represents the tip by a single point carrying the finding's judged `latest` value at
/// `merge_base_index + 1`. The trailing-fill target is `None` — the tip is the
/// always-present last column. The gap between the last base point (at the comparison
/// base) and the tip therefore spans exactly `merge_base - comparison_base` (==
/// `commits_behind`) columns.
///
/// A real branch finding always carries a known merge-base and comparison base; the
/// fallback to a plain whole-series chart is defensive (never a panic) for a finding
/// that somehow lacks either.
fn branch_chart_series(
    source: &Series,
    finding: &Finding,
    context: &AnalysisContext,
) -> (Vec<SeriesValue>, Option<usize>) {
    debug_assert!(
        context.merge_base_index.is_some() && finding.comparison_base_index.is_some(),
        "a branch finding always carries a known merge-base and comparison base",
    );
    let (Some(merge_base_index), Some(_)) =
        (context.merge_base_index, finding.comparison_base_index)
    else {
        return (source.points.iter().map(series_value_of).collect(), None);
    };
    let (base, branch) = split_at_merge_base(&source.points, Some(merge_base_index));
    let mut series: Vec<SeriesValue> = base.iter().copied().map(series_value_of).collect();
    series.push(SeriesValue {
        commit: finding.commit.clone(),
        value: finding.latest,
        dirty: branch.last().is_some_and(|point| point.dirty),
        topo_index: merge_base_index.saturating_add(1),
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
fn clears_absolute_floor(series: &Series, delta: f64, config: &AnalysisConfig) -> bool {
    delta.abs() >= absolute_floor(series.kind, config)
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

/// Whether the two regimes of a `delta`-signed level shift are *separated enough*
/// to be distinct populations, rather than two windows onto one noisy distribution.
///
/// A rank test's p-value proves only that the regimes *differ*, and it grows more
/// significant with sample size even for a heavily overlapping move — so a long but
/// stationary series that oscillates between two levels (noisy yet stable) passes
/// the significance gate. This gate adds the effect-size the significance test
/// lacks: the Mann–Whitney probability of superiority (the chance a random `after`
/// point exceeds a random `before` one), oriented in the move's direction, must
/// reach `config.min_regime_separation`. A genuine step scores ~1; bimodal jitter
/// scores near ½ and is rejected. Missing statistics (`None`, from an empty sample)
/// are treated as no evidence of overlap, so the move is trusted.
fn regimes_are_separated(
    mann_whitney: Option<stats::MannWhitneyU>,
    delta: f64,
    config: &AnalysisConfig,
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
            directional >= config.min_regime_separation
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

    let mann_whitney_u = stats::MannWhitneyU::new(before, after);
    let mann_whitney = mann_whitney_u.map_or(1.0, |ranked| ranked.two_sided_p_value());
    if mann_whitney >= config.change_alpha {
        return None;
    }
    if relative_delta.abs() < config.practical_relative {
        return None;
    }
    if !clears_absolute_floor(series, delta, config) {
        return None;
    }
    if !exceeds_residual_noise(delta, step_model_residual(values, tau), config) {
        return None;
    }
    if !regimes_are_separated(mann_whitney_u, delta, config) {
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
            direction: direction_of(delta),
            baseline,
            latest,
            delta,
            relative_delta,
            confidence: (1.0 - effective_p).clamp(0.0, 1.0),
            commit,
            flipped_at: None,
            active: true,
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
    if !clears_absolute_floor(series, delta, config) {
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
            direction: direction_of(delta),
            baseline,
            latest,
            delta,
            relative_delta,
            confidence: (1.0 - trend.p_value).clamp(0.0, 1.0),
            commit,
            flipped_at: None,
            active: true,
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

/// The branch tip's latest measured state.
///
/// A feature branch's own history says nothing about what merging it into the base
/// will do — only its tip commit lands there — so branch mode judges the newest
/// commit's latest state, not a reconstructed within-branch regime. `branch` is
/// sorted by `(topo_index, dirty, object_ordinal)`, so that state is the contiguous
/// suffix sharing the last point's commit *and* dirty flag: the tip's committed
/// (clean) runs, or — when the working tree is dirty — the dirty snapshots taken on
/// top of it, which supersede the clean run as the newer state. Either way any
/// repeated (`--best-of`) observations in that cohort are kept. Mixing a clean tip
/// run with the dirty snapshots above it would blur two distinct states into one
/// spuriously noisy sample, so only the latest cohort is returned. An empty branch
/// yields no points.
fn latest_commit_points<'a>(branch: &[&'a SeriesPoint]) -> Vec<&'a SeriesPoint> {
    let Some(&last) = branch.last() else {
        return Vec::new();
    };
    branch
        .iter()
        .filter(|point| point.topo_index == last.topo_index && point.dirty == last.dirty)
        .copied()
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
fn commit_levels(points: &[&SeriesPoint]) -> Vec<f64> {
    let mut levels = Vec::new();
    let mut group: Vec<f64> = Vec::new();
    let mut current: Option<(usize, bool)> = None;
    for point in points {
        let key = (point.topo_index, point.dirty);
        if current != Some(key) {
            if let Some(level) = stats::median_in_place(&mut group) {
                levels.push(level);
            }
            group.clear();
            current = Some(key);
        }
        group.push(point.value);
    }
    if let Some(level) = stats::median_in_place(&mut group) {
        levels.push(level);
    }
    levels
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
/// Both the centre and the scale are deliberately non-robust *together*. A level
/// shift that landed on the base branch inside the window raises the sample standard
/// deviation, so the window it sits in demands a larger move before anything is
/// reported and the detector goes quiet until the step ages out. That is the correct
/// trade: making the scale robust to such a step while the centre stays the window
/// mean is strictly worse, because the mean then sits between the two levels and a
/// tip agreeing exactly with the newer level reads as displaced from it — the
/// unsettled window would manufacture findings on branches that changed nothing.
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

/// Compares a `before` sample against an `after` sample on the same series and, if
/// the noise-aware gates pass, returns a change-point [`Candidate`].
///
/// `before` is the recent base-side window and `after` the branch tip's runs. Both
/// collapse to per-commit levels first (see [`commit_levels`]), so the comparison is
/// one new commit's level against the base's commit-to-commit distribution; the
/// tip's repeated runs share a build and a runner and so cannot count as independent
/// evidence.
///
/// The base level is the window's **mean**, which is the centre
/// [`prediction_interval_p`] measures against, so the magnitude the finding reports
/// is the one its p-value describes.
///
/// The relative move must clear `practical_floor` and the metric's absolute floor,
/// stand above the base window's own residual scatter, and then be significant as a
/// Student-t prediction interval. Where the engine reports per-point confidence
/// intervals the two samples' intervals must also be disjoint and the move must
/// clear the measurement noise band; both are extra vetoes that can only *suppress*
/// a candidate the other gates would have reported — they never turn a non-finding
/// into a finding.
fn compare_samples(
    series: &Series,
    before: &[&SeriesPoint],
    after: &[&SeriesPoint],
    config: &AnalysisConfig,
    practical_floor: f64,
    commit: Option<String>,
) -> Option<Candidate> {
    let before_values = commit_levels(before);
    let after_values: Vec<f64> = after.iter().map(|point| point.value).collect();
    let baseline = stats::mean(&before_values)?;
    let latest = stats::median(&after_values)?;
    let delta = latest - baseline;
    if delta.abs() <= 0.0 {
        return None;
    }
    let relative_delta = relative_delta_of(delta, baseline);

    if before_values.len() < config.min_series_points {
        return None;
    }
    if relative_delta.abs() < practical_floor {
        return None;
    }
    if !clears_absolute_floor(series, delta, config) {
        return None;
    }
    if !exceeds_residual_noise(
        delta,
        sample_step_residual(&before_values, &after_values),
        config,
    ) {
        return None;
    }
    let effective_p =
        prediction_interval_p(&before_values, latest, scatter_floor(series.kind, config))?;
    if effective_p >= config.change_alpha {
        return None;
    }
    if let (Some(before_ci), Some(after_ci)) = (regime_interval(before), regime_interval(after))
        && !intervals_disjoint(before_ci, after_ci)
    {
        return None;
    }
    // Where per-point confidence intervals exist, require the move to also clear the
    // measurement noise band — a veto that can only suppress this candidate.
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
            flipped_at: None,
            active: true,
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

/// Evaluates a series in *branch* mode: compares the branch's tip commit against
/// the recent base level, in either direction.
///
/// The branch's intermediate commits are ignored — only its newest commit's runs
/// matter (see [`latest_commit_points`]), since that is the state a merge lands in
/// the base. A new benchmark introduced on the branch (no base-side points) or an
/// empty branch yields nothing, since there is no baseline to compare.
fn evaluate_branch(
    series: &Series,
    config: &AnalysisConfig,
    merge_base_index: Option<usize>,
) -> Option<Candidate> {
    let (base, branch) = split_at_merge_base(&series.points, merge_base_index);
    // An empty base or branch yields nothing: `compare_samples` returns `None` once
    // either sample's median is absent, so no explicit emptiness guard is needed.
    let base_window = recent_commits(&base, config.compare_window);
    let latest_points = latest_commit_points(&branch);
    let commit = branch.last().and_then(|&point| owned_commit(point));
    // The newest base-side point actually fed to the comparison is this series' comparison
    // base. Record its first-parent position so the analysis can measure how far it sits
    // behind the merge-base.
    let comparison_base_index = base_window.last().map(|point| point.topo_index);
    let mut candidate = compare_samples(
        series,
        &base_window,
        &latest_points,
        config,
        config.branch_practical_relative,
        commit,
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

/// Locates a *recovered* spike in a (re-baselined) history series: a sustained
/// interior regime that deviated from baseline and has since returned to it.
///
/// Such a change is no longer reflected in the latest state, so it is emitted as an
/// *inactive* finding (only surfaced with `--include-inactive`): `commit` names where
/// the level rose, `flipped_at` where it recovered, `baseline` the pre-spike level,
/// and `latest` the spike's own level (its magnitude is what is notable). Both the
/// rise and the recovery must be Mann–Whitney significant, the plateau must clear
/// the practical-magnitude floor (relative, plus the metric's own absolute floor),
/// and the deviation must stand above the rise's own
/// residual scatter.
fn evaluate_resolved_spike(
    series: &Series,
    values: &[f64],
    config: &AnalysisConfig,
) -> Option<Candidate> {
    let points = &series.points;
    let n = points.len();
    if n > noise_gates::RESOLVED_SPIKE_MAX_POINTS {
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
        || !clears_absolute_floor(series, deviation, config)
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
            direction: direction_of(deviation),
            baseline,
            latest: level,
            delta: deviation,
            relative_delta,
            confidence: (1.0 - effective_p).clamp(0.0, 1.0),
            commit: points.get(rise).and_then(owned_commit),
            flipped_at: points.get(recovery).and_then(owned_commit),
            active: false,
            blessed_at: None,
            blessed_commit_time: None,
            series: Vec::new(),
            comparison_base_index: None,
            chart_base_ref: None,
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
pub(super) fn find_changes(series: &[Series], context: &AnalysisContext) -> Detection {
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
            let (base, branch) = split_at_merge_base(&series.points, context.merge_base_index);
            if latest_commit_points(&branch).is_empty() {
                Testability::Unjudged(UnjudgedReason::NotMeasuredOnBranch)
            } else if commit_levels(&recent_commits(&base, config.compare_window)).len()
                < config.min_series_points
            {
                Testability::Unjudged(UnjudgedReason::TooFewBaseCommits)
            } else {
                Testability::Judged
            }
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
#[cfg(test)]
fn detect_all(series: &[Series], context: &AnalysisContext) -> (Vec<Candidate>, SeriesCensus) {
    detect_range(series, 0..series.len(), context)
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
        handles.push(spawner.spawn_blocking(move || detect_range(&chunk, start..end, &context)));
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
fn detect_range(
    series: &[Series],
    range: Range<usize>,
    context: &AnalysisContext,
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
            && let Some(candidate) = detect_one(index, one, context)
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
/// locates a change-point and a drift and keeps the better-fitting one (optionally
/// surfacing a recovered spike); branch mode delegates to its dedicated detector.
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

    use std::slice;
    use std::sync::Arc;

    use cbh_model::{DiscriminantSet, Engine, MetricKind};
    use jiff::Timestamp;
    use nonempty::nonempty;

    use super::*;
    use crate::detect::noise_gates::{
        COMPARE_WINDOW, DRIFT_MIN_POINTS, MIN_REGIME, MIN_SERIES_POINTS,
    };
    use crate::detect::{Blessing, SeriesPoint};

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

    /// Builds a Callgrind-style history with a [`MIN_REGIME`]-point plateau at
    /// `peak` bracketed by [`MIN_REGIME`]-point baseline and recovery regimes at
    /// `base`: a spike that rose and has since fully recovered, in the shortest
    /// history that can hold one.
    ///
    /// The plateau search requires all three regimes to hold at least
    /// `min_regime` points, so this is exactly `3 * MIN_REGIME` points long and
    /// admits exactly one plateau window.
    fn recovered_spike(base: f64, peak: f64) -> Series {
        series_of(&three_regimes(base, peak, base))
    }

    /// Three consecutive [`MIN_REGIME`]-point regimes at the given levels: the
    /// shortest history a recovered spike can be found in.
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
                flipped_at: None,
                active: true,
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

    /// The largest `topo_index` across every point of `series`, the realistic tip
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

    /// Runs the history-mode detector with default config, reporting both
    /// directions.
    fn changes(series: &[Series]) -> Vec<Finding> {
        find_changes(series, &history_context(series)).findings
    }

    /// The history-mode [`AnalysisContext`] the [`changes`] helper runs under.
    fn history_context(series: &[Series]) -> AnalysisContext {
        AnalysisContext {
            mode: AnalysisMode::History,
            config: AnalysisConfig::default(),
            merge_base_index: None,
            tip_index: max_topo_index(series),
            include_improvements: true,
            include_inactive: false,
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
            tip_index: max_topo_index(&series),
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
        assert!(evaluate_change_point(&series, &values_of(&series), &config).is_none());
    }

    #[test]
    fn change_point_within_its_own_residual_scatter_is_suppressed() {
        // A rank-significant step (medians 102 -> 132, delta 30) whose regimes each
        // wobble by 2. Under the default residual multiple the move stands clear of
        // that scatter and is flagged; a deliberately high multiple pushes the noise
        // band above the move, so only the residual gate rejects it (every earlier
        // gate — persistence, Mann-Whitney, practical floor — still passes).
        let series = series_of(&[
            100.0, 104.0, 100.0, 104.0, 102.0, 130.0, 134.0, 130.0, 134.0, 132.0,
        ]);
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
    fn regimes_are_separated_rejects_interleaved_levels() {
        let config = AnalysisConfig::default();
        // A clean rise: every after-point exceeds every before-point (superiority 1).
        assert!(regimes_are_separated(
            stats::MannWhitneyU::new(&[10.0, 11.0, 12.0], &[20.0, 21.0, 22.0]),
            10.0,
            &config,
        ));
        // A clean fall: judged by the complementary direction, still fully separated.
        assert!(regimes_are_separated(
            stats::MannWhitneyU::new(&[20.0, 21.0, 22.0], &[10.0, 11.0, 12.0]),
            -10.0,
            &config,
        ));
        // Two levels that recur on both sides: only 0.75 of the after-vs-before pairs
        // move in the rise's direction, below the 0.85 floor, so it is not separated.
        assert!(!regimes_are_separated(
            stats::MannWhitneyU::new(&[10.0, 10.0, 10.0, 30.0], &[30.0, 30.0, 30.0, 10.0]),
            20.0,
            &config,
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
            &config,
        ));
        // No statistics at all (an empty regime): the gate has nothing to veto on, so
        // it trusts the move rather than suppressing it.
        assert!(regimes_are_separated(None, 10.0, &config));
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
        let values = vec![
            13.26, 14.33, 13.14, 24.97, 13.2, 24.97, 13.17, 25.39, 25.54, 13.18, 13.83, 25.45,
            25.02, 25.0, 13.2, 13.22, 13.24, 13.21, 13.15, 24.97, 26.78, 13.24, 28.98, 10.5, 10.53,
            26.76, 26.74, 13.58, 13.54, 28.86, 14.15, 13.5, 26.77, 25.38, 25.0, 13.97, 26.81,
            25.54, 13.62, 13.57,
        ];
        let series = series_of(&values);
        let permissive = AnalysisConfig {
            min_regime_separation: 0.0,
            ..AnalysisConfig::default()
        };
        assert!(evaluate_change_point(&series, &values, &permissive).is_some());
        assert!(evaluate_change_point(&series, &values, &AnalysisConfig::default()).is_none());
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
        assert!(evaluate_change_point(&series, &values_of(&series), &permissive).is_some());
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
        find_changes(series, &branch_context(series, merge_base_index)).findings
    }

    /// The branch-mode [`AnalysisContext`] the [`branch_changes`] helper runs under.
    fn branch_context(series: &[Series], merge_base_index: Option<usize>) -> AnalysisContext {
        AnalysisContext {
            mode: AnalysisMode::Branch,
            config: AnalysisConfig::default(),
            merge_base_index,
            tip_index: max_topo_index(series),
            include_improvements: false,
            include_inactive: false,
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
        placed_series_of_kind(&points, kind)
    }

    /// A branch-mode fixture on an instruction count: a [`base_run`] at `base` followed
    /// by `branch_points` commits at `branch`, the first of which sits just past
    /// [`base_merge_base`].
    fn branch_over_base(base: f64, branch: f64, branch_points: usize) -> Series {
        branch_over_base_of_kind(base, branch, branch_points, MetricKind::InstructionCount)
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
        // A single sustained regime: no within-branch flip is reported.
        assert_eq!(finding.flipped_at, None);
    }

    #[test]
    fn branch_mode_reports_the_tip_state_after_an_intermediate_change() {
        // The branch first improved (80) then regressed (130): only the tip commit
        // lands in the base, so we report the tip state (worse than the 100 base)
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
        // Branch mode judges the tip commit alone, so no within-branch flip is
        // attributed.
        assert_eq!(finding.flipped_at, None);
    }

    #[test]
    fn branch_mode_is_silent_when_the_branch_matches_the_base() {
        let series = branch_over_base(100.0, 100.0, 3);
        assert!(branch_changes(&[series], Some(base_merge_base())).is_empty());
    }

    #[test]
    fn branch_mode_reports_an_improvement_over_the_base() {
        // Branch mode always reports both directions, regardless of
        // `include_improvements` (which only governs history mode).
        let series = branch_over_base(100.0, 70.0, 3);
        let finding = only(branch_changes(&[series], Some(base_merge_base())));
        assert_eq!(finding.direction, Direction::Improvement);
        assert!(!finding.is_regression());
        assert_eq!(finding.latest, 70.0);
    }

    #[test]
    fn branch_mode_below_the_absolute_floor_is_suppressed() {
        // A quantized branch tip 4 counts above a small base (60 -> 64) clears the 5%
        // branch relative floor (6.7%) and the residual gate, but not the absolute
        // floor of 5, so it is suppressed. Without the gate this single-quantum-scale
        // move would flag on the pull request. Dropping the floor far below the move
        // admits it again, so the floor is the sole reason for the silence.
        let series = branch_over_base(60.0, 64.0, 3);
        assert!(
            evaluate_branch(&series, &AnalysisConfig::default(), Some(base_merge_base())).is_none()
        );
        let permissive = AnalysisConfig {
            practical_absolute_count: 0.1,
            ..AnalysisConfig::default()
        };
        assert!(evaluate_branch(&series, &permissive, Some(base_merge_base())).is_some());
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
        assert!(evaluate_branch(&series, &config, Some(base_merge_base())).is_some());
        let without_quantum = AnalysisConfig {
            scatter_floor_count: 0.0,
            ..config
        };
        assert!(evaluate_branch(&series, &without_quantum, Some(base_merge_base())).is_none());
    }

    #[test]
    fn branch_mode_reports_a_series_that_starts_allocating() {
        // Code that allocated nothing and now allocates 48 bytes an iteration: the base
        // window is ten commits of exactly zero, so its scatter is zero and only the
        // one-byte quantum keeps the move judgeable. It is a full-scale move against a
        // zero baseline, 48 bytes clears the one-byte absolute floor, and the floored
        // scatter puts the tip 45.8 standard errors out (48 / (1 * sqrt(1 + 1/10))),
        // so it is reported decisively. Removing the quantum silences it, which is
        // exactly the regression shape this floor exists for.
        let series = branch_over_base_of_kind(0.0, 48.0, 1, MetricKind::AllocatedBytes);
        let config = AnalysisConfig::default();
        let candidate =
            evaluate_branch(&series, &config, Some(base_merge_base())).expect("48 bytes is a move");
        assert_eq!(candidate.finding.direction, Direction::Regression);
        assert_eq!(candidate.finding.latest, 48.0);
        assert!(candidate.bh_p < config.change_alpha, "{}", candidate.bh_p);
        let without_quantum = AnalysisConfig {
            scatter_floor_alloc: 0.0,
            ..config
        };
        assert!(evaluate_branch(&series, &without_quantum, Some(base_merge_base())).is_none());
    }

    #[test]
    fn branch_mode_is_silent_when_a_timing_base_carries_no_scatter() {
        // Time has no quantum — a stored time is a regression slope over a run's
        // iterations, not a counted unit — so a base window that repeats one value
        // leaves nothing to place the tip against and the standard error is degenerate.
        // A doubling from 20 ns to 40 ns clears every floor and is still not reported:
        // the degenerate case fails silent rather than manufacturing certainty. The
        // same move against a base that does scatter is reported, so the flat base is
        // the sole reason for the silence.
        let flat = branch_over_base_of_kind(20.0, 40.0, 1, MetricKind::WallTime);
        let config = AnalysisConfig::default();
        assert!(evaluate_branch(&flat, &config, Some(base_merge_base())).is_none());

        let mut points = wobbling_base_run(20.0, 0.2);
        points.push((MIN_SERIES_POINTS, 40.0, false));
        let scattering = placed_series_of_kind(&points, MetricKind::WallTime);
        assert!(evaluate_branch(&scattering, &config, Some(base_merge_base())).is_some());
    }

    #[test]
    fn branch_mode_is_silent_for_a_sub_nanosecond_timing_move() {
        // A benchmark measuring 2.49 ns an iteration whose tip reads 3.12 ns. That is a
        // 25% move on a base scattering by only 0.05 ns, so every statistical gate
        // passes decisively (the tip sits 11.4 standard errors out) — yet the move
        // itself spans 0.63 ns, under the one-nanosecond floor below which a timing
        // move is not worth acting on, so nothing is reported. Lowering that floor
        // admits it, which pins the absolute floor as the sole reason for the silence.
        let mut points = wobbling_base_run(2.49, 0.05);
        points.push((MIN_SERIES_POINTS, 3.12, false));
        let series = placed_series_of_kind(&points, MetricKind::WallTime);
        let config = AnalysisConfig::default();
        assert!(evaluate_branch(&series, &config, Some(base_merge_base())).is_none());
        let permissive = AnalysisConfig {
            practical_absolute_time: 0.1,
            ..config
        };
        assert!(evaluate_branch(&series, &permissive, Some(base_merge_base())).is_some());
    }

    #[test]
    fn branch_mode_reports_a_small_timing_regression_a_one_nanosecond_scatter_floor_would_hide() {
        // A 20 ns benchmark whose base scatters by 0.2 ns from commit to commit,
        // regressing by 8% (1.6 ns). Against its own scatter the tip sits 7.2 standard
        // errors out (1.6 / (0.2108 * sqrt(1 + 1/10))) and the move clears the
        // one-nanosecond absolute floor, so it is reported. Flooring the scatter at
        // that same nanosecond instead would put the tip only 1.5 standard errors out
        // — p = 0.16, comfortably inside the interval — and hide the regression
        // entirely, which is what makes the quantum and the magnitude floor separate
        // quantities.
        let mut points = wobbling_base_run(20.0, 0.2);
        points.push((MIN_SERIES_POINTS, 21.6, false));
        let series = placed_series_of_kind(&points, MetricKind::WallTime);
        let config = AnalysisConfig::default();
        let candidate = evaluate_branch(&series, &config, Some(base_merge_base()))
            .expect("an 8% move on a quiet base is detectable");
        assert_eq!(candidate.finding.direction, Direction::Regression);
        assert!(candidate.bh_p < config.change_alpha, "{}", candidate.bh_p);
        let floored = AnalysisConfig {
            scatter_floor_time: config.practical_absolute_time,
            ..config
        };
        assert!(evaluate_branch(&series, &floored, Some(base_merge_base())).is_none());
    }

    #[test]
    fn branch_finding_reports_the_move_from_the_centre_its_test_used() {
        // The prediction interval places the tip against the base window's *mean*, so
        // the magnitude the finding reports must be measured from that same centre. A
        // window of nine commits at 100 and one at 140 has a mean of 104 and a median
        // of 100, which a tip at 200 turns into a reported move of 96 (92.3%) rather
        // than 100 (100%) — the median would describe a move the p-value never tested.
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

        let candidate =
            evaluate_branch(&series, &AnalysisConfig::default(), Some(base_merge_base()))
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
    fn branch_mode_admits_a_dirty_snapshot_at_the_merge_base_tip() {
        // The merge-base is the branch tip; a dirty snapshot there is the branch
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
    fn history_chart_series_maps_every_observation_and_targets_the_tip() {
        // History mode keeps the series compact and 1:1 — every observation becomes one
        // chart point carrying its real topo index — and stamps the analyzed tip as the
        // trailing-fill target so a lagging series can render its "no newer data" gap.
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
    fn history_chart_base_ref_is_the_analyzed_tip_beyond_the_last_observation() {
        // When analysis reaches commits newer than the last observation, the
        // trailing-fill target is that tip, so the chart shows the lag as a trailing gap
        // — the visual form of the "lagged history" warning.
        let series = series_of(&step_values(100.0, 130.0));
        let context = AnalysisContext {
            mode: AnalysisMode::History,
            config: AnalysisConfig::default(),
            merge_base_index: None,
            tip_index: 20,
            include_improvements: true,
            include_inactive: false,
        };
        let finding = only(find_changes(slice::from_ref(&series), &context).findings);
        assert_eq!(finding.chart_base_ref, Some(20));
    }

    /// The chart series a branch fixture built by [`branch_over_base`] must collapse
    /// to: every base column, then one tip column at `merge_base + 1`.
    fn expected_branch_chart(base: f64, branch: f64) -> Vec<(usize, f64)> {
        let mut expected: Vec<(usize, f64)> =
            (0..MIN_SERIES_POINTS).map(|index| (index, base)).collect();
        expected.push((MIN_SERIES_POINTS, branch));
        expected
    }

    #[test]
    fn branch_chart_series_collapses_interior_commits_onto_the_tip() {
        // Branch mode drops every interior branch commit and represents the branch by a
        // single tip column at merge_base + 1 carrying the judged latest.
        let series = branch_over_base(100.0, 130.0, 3);
        let finding = only(branch_changes(&[series], Some(base_merge_base())));
        assert_eq!(
            finding.chart_base_ref, None,
            "the tip is the always-present last column, so no trailing fill"
        );
        assert_eq!(chart_pairs(&finding), expected_branch_chart(100.0, 130.0));
        let tip = finding.series.last().expect("the tip column is present");
        assert_eq!(
            tip.topo_index, MIN_SERIES_POINTS,
            "the tip is remapped to merge_base + 1"
        );
        assert_eq!(
            tip.value, finding.latest,
            "the tip column carries the judged latest, not a raw observation"
        );
    }

    #[test]
    fn branch_chart_series_is_unchanged_by_extra_interior_branch_commits() {
        // Interior branch commits contribute zero columns, so a branch that detoured
        // (improved, then regressed) collapses to the same compact chart series as one
        // that went straight to the tip value — the base and tip state being equal.
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

    /// Runs the history-mode detector reporting both directions *and* inactive
    /// findings, so a recovered spike surfaces.
    fn changes_with_inactive(series: &[Series]) -> Vec<Finding> {
        find_changes(
            series,
            &AnalysisContext {
                mode: AnalysisMode::History,
                config: AnalysisConfig::default(),
                merge_base_index: None,
                tip_index: max_topo_index(series),
                include_improvements: true,
                include_inactive: true,
            },
        )
        .findings
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
    fn history_stamps_blessing_provenance_on_an_active_finding() {
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
        assert!(finding.active);
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

    #[test]
    fn resolved_spike_is_detected_and_marked_inactive() {
        // A plateau at 20 between baseline regimes at 10 that has since recovered.
        // Every engine is treated as noisy, so the elevated span must clear a
        // Mann-Whitney gate on both sides; three full-size regimes make the rise and
        // the fall significant.
        let spike = recovered_spike(10.0, 20.0);
        let candidate =
            evaluate_resolved_spike(&spike, &values_of(&spike), &AnalysisConfig::default())
                .unwrap();
        assert!(!candidate.finding.active);
        assert_eq!(candidate.finding.baseline, 10.0);
        assert_eq!(candidate.finding.latest, 20.0);
        assert_eq!(candidate.finding.direction, Direction::Regression);
        // `commit` names where the median-plateau search brackets the rise,
        // `flipped_at` where it recovered.
        assert_eq!(
            candidate.finding.commit.as_deref(),
            Some(format!("commit{MIN_REGIME}").as_str())
        );
        assert_eq!(
            candidate.finding.flipped_at.as_deref(),
            Some(format!("commit{}", 2 * MIN_REGIME).as_str())
        );
    }

    #[test]
    fn history_surfaces_a_resolved_spike_only_with_include_inactive() {
        // The spike rose and recovered, so no active change remains: the default
        // history pass is silent.
        let spike = recovered_spike(10.0, 20.0);
        judged_but_silent(slice::from_ref(&spike));

        // Requesting inactive findings surfaces it as a recovered spike that is no
        // longer reflected in the latest state.
        let finding = only(changes_with_inactive(&[spike]));
        assert!(!finding.active);
        assert_eq!(finding.direction, Direction::Regression);
        assert_eq!(finding.baseline, 10.0);
        assert_eq!(finding.latest, 20.0);
        assert!(finding.flipped_at.is_some());
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
        compare_samples(&series, &before_refs, &after_refs, config, floor, None)
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
    /// leaves the prediction interval no distribution to place the tip in. Real timing
    /// series never repeat a value, so the window alternates by this much instead. A
    /// full window's sample standard deviation is then
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
    fn compare_samples_needs_the_minimum_base_commit_levels() {
        // The base sample is one *commit level* per commit, and the prediction
        // interval needs `min_series_points` of them to say anything about the base's
        // commit-to-commit scatter. One level short is refused outright; adding the
        // missing level reports the same unmistakable move.
        let after = pts(&[(130.0, 0.5)]);
        let too_short = pts(&[(100.0, 0.5); MIN_SERIES_POINTS - 1]);
        assert!(compare(&too_short, &after, 0.05).is_none());
        assert!(compare(&base_window(100.0, 0.5), &after, 0.05).is_some());
    }

    #[test]
    fn compare_samples_suppresses_a_significant_move_with_overlapping_intervals() {
        // A move far outside the base's prediction interval, but the branch side's
        // confidence interval is so wide that it overlaps the base's, so the change is
        // rejected. Deleting the `!` in the interval-overlap guard would let it
        // through.
        let before = base_window(100.0, 2.0);
        let after = pts(&[(130.0, 60.0); 5]);
        assert!(compare(&before, &after, 0.05).is_none());
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
        // Delta 8 == 2 * 4, the median confidence half-width across both samples: the
        // strict `>` noise-floor gate rejects it. A `>`->`>=`/`==`, the `*`->`+`/`/`
        // arithmetic, or an always-true guard would each flag it instead. The branch
        // side carries a tight interval so the interval-disjointness veto does not
        // pre-empt this one.
        let before = base_window(100.0, 4.0);
        assert!(compare(&before, &pts(&[(108.0, 0.1); 5]), 0.05).is_none());
        // Half a unit more clears the band and is reported.
        assert!(compare(&before, &pts(&[(108.5, 0.1); 5]), 0.05).is_some());
    }

    #[test]
    fn compare_samples_suppresses_a_tip_inside_a_bimodal_base() {
        // A base that alternates between two levels (~10 and ~30) from commit to
        // commit. A branch tip landing on the upper level moves the median by 10, but
        // that is well inside the base's own commit-to-commit scatter, so the
        // prediction interval refuses it — even with every scatter-based veto
        // relaxed, which pins the prediction interval as the sole reason for the
        // silence. A tip clear of *both* levels is reported.
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
    fn latest_commit_points_returns_only_the_newest_commit() {
        // Two branch commits (topo 3 and topo 5); the newer carries two clean runs
        // (a `--best-of` pair). Only the newest commit's runs are returned — the tip
        // is what a merge lands in the base.
        let series = placed_series(&[(3, 100.0, false), (5, 130.0, false), (5, 130.0, false)]);
        let branch: Vec<&SeriesPoint> = series.points.iter().collect();
        let latest = latest_commit_points(&branch);
        assert_eq!(latest.len(), 2);
        assert!(latest.iter().all(|point| point.topo_index == 5));
    }

    #[test]
    fn latest_commit_points_prefers_dirty_snapshots_over_the_clean_tip() {
        // The tip commit (topo 5) has a committed clean run plus two dirty snapshots
        // taken on top of it. The dirty snapshots are the newer state, so only they
        // are returned — mixing in the clean run would blur two states into one.
        let series = placed_series(&[
            (3, 100.0, false),
            (5, 130.0, false),
            (5, 131.0, true),
            (5, 131.0, true),
        ]);
        let branch: Vec<&SeriesPoint> = series.points.iter().collect();
        let latest = latest_commit_points(&branch);
        assert_eq!(latest.len(), 2);
        assert!(
            latest
                .iter()
                .all(|point| point.topo_index == 5 && point.dirty)
        );
    }

    #[test]
    fn latest_commit_points_of_an_empty_branch_is_empty() {
        assert!(latest_commit_points(&[]).is_empty());
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
        let candidate = evaluate_drift(&series, &values_of(&series), &config).unwrap();
        assert_eq!(candidate.finding.method, FindingMethod::Drift);
        assert_eq!(candidate.finding.relative_delta, config.practical_relative);
        assert!(candidate.finding.confidence < 1.0);
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
        assert!(evaluate_drift(&series, &values_of(&series), &without_absolute_floor).is_some());
        assert!(evaluate_drift(&series, &values_of(&series), &AnalysisConfig::default()).is_none());
    }

    #[test]
    fn drift_at_the_absolute_floor_is_flagged() {
        // One more commit on the same staircase carries the fitted line to exactly 5
        // counts, which clears the absolute floor and is flagged, pinning the gate's
        // `>=` boundary.
        let series = series_of(&staircase(100.0, MIN_SERIES_POINTS + 1));
        let candidate =
            evaluate_drift(&series, &values_of(&series), &AnalysisConfig::default()).unwrap();
        assert_eq!(candidate.finding.method, FindingMethod::Drift);
        assert_eq!(candidate.finding.delta, 5.0);
    }

    #[test]
    fn noisy_drift_within_the_measurement_noise_floor_is_suppressed() {
        // The same climb on a noisy engine, but the endpoints (delta 36) do not
        // separate by more than twice the confidence half-width (20): jitter, not a
        // trend. The `2.0 * half_width` floor must be a product (a `+` mutant lowers
        // the floor to 22 and would flag it).
        let series = wall_series(&ramp(100.0, 4.0, MIN_SERIES_POINTS), 20.0);
        assert!(evaluate_drift(&series, &values_of(&series), &AnalysisConfig::default()).is_none());
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
        // rejected outright, while a series of exactly that length is still evaluated
        // (so a gate mutated to reject the longer series instead is caught).
        let config = AnalysisConfig {
            practical_relative: 20.0 / 100.0,
            ..AnalysisConfig::default()
        };
        let short = series_of(&ramp(100.0, 4.0, DRIFT_MIN_POINTS - 1));
        assert!(evaluate_drift(&short, &values_of(&short), &config).is_none());
        let long = series_of(&ramp(100.0, 4.0, DRIFT_MIN_POINTS));
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
            tip_index: 0,
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
            tip_index: 0,
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
        let series = recovered_spike(10.0, 20.0);
        let candidate =
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .unwrap();
        assert_eq!(candidate.finding.delta, 10.0);
    }

    #[test]
    fn resolved_spike_reports_the_earliest_most_deviated_plateau() {
        // Several plateau windows can tie on deviation when the elevated stretch is
        // longer than `min_regime`: here [5, 10), [5, 11) and [6, 11) all sit at 200
        // over a baseline of 100. The search keeps the first such window (a strict
        // `>` against the incumbent), so the reported rise and recovery commits are
        // the earliest that explain the excursion rather than the last window the
        // scan happened to visit.
        let mut values = vec![100.0_f64; MIN_REGIME];
        values.extend(std::iter::repeat_n(200.0, MIN_REGIME + 1));
        values.extend(std::iter::repeat_n(100.0, MIN_REGIME));
        let series = series_of(&values);
        let candidate =
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .unwrap();
        assert_eq!(candidate.split, Some(MIN_REGIME));
        assert_eq!(
            candidate.finding.commit.as_deref(),
            Some(format!("commit{MIN_REGIME}").as_str())
        );
        assert_eq!(
            candidate.finding.flipped_at.as_deref(),
            Some(format!("commit{}", 2 * MIN_REGIME).as_str())
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "the 200-point quadratic spike search is slow under Miri"
    )]
    fn resolved_spike_at_the_search_size_limit_is_flagged() {
        // A 200-point history (the inclusive search ceiling) with a recovered plateau
        // still analyses: the `n > noise_gates::RESOLVED_SPIKE_MAX_POINTS` guard
        // must be a strict `>`.
        let mut values = vec![10.0_f64; noise_gates::RESOLVED_SPIKE_MAX_POINTS];
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
        // `n > noise_gates::RESOLVED_SPIKE_MAX_POINTS` guard caps the quadratic
        // plateau search.
        let mut values = vec![10.0_f64; noise_gates::RESOLVED_SPIKE_MAX_POINTS + 1];
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
        let series = recovered_spike(1000.0, 1010.0);
        assert!(
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .is_none()
        );
    }

    #[test]
    fn resolved_spike_exactly_at_the_practical_floor_is_a_spike() {
        // A plateau (1030) exactly 3% above baseline (1000) meets the floor; the
        // `relative < floor` gate must be a strict `<` (a `<=`/`==` mutant suppresses
        // it). The magnitudes are scaled well past the absolute floor so only the
        // relative gate's strictness is under test here.
        let series = recovered_spike(1000.0, 1030.0);
        let config = AnalysisConfig {
            practical_relative: 3.0 / 100.0,
            ..AnalysisConfig::default()
        };
        assert!(evaluate_resolved_spike(&series, &values_of(&series), &config).is_some());
    }

    #[test]
    fn resolved_spike_below_the_absolute_floor_is_not_a_spike() {
        // A recovered spike whose plateau rose only 4 counts above a small baseline
        // (60 -> 64 -> 60) clears the relative floor (6.7%) and the rise/recovery rank
        // tests, but not the absolute floor of 5, so it is not reported. Without the
        // gate a single-quantum blip on a tiny count would surface as an inactive spike.
        let series = recovered_spike(60.0, 64.0);
        assert!(
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .is_none()
        );
    }

    #[test]
    fn resolved_spike_at_the_absolute_floor_is_a_spike() {
        // The same spike raised to a 5-count plateau (60 -> 65 -> 60) clears the
        // absolute floor and is reported, pinning the gate's `>=` boundary.
        let series = recovered_spike(60.0, 65.0);
        assert!(
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .is_some()
        );
    }

    #[test]
    fn noisy_resolved_spike_with_significant_rise_and_recovery_is_flagged() {
        // A noisy plateau (200) between baseline/recovery regimes (100): both the
        // rise and the recovery are Mann-Whitney significant, so the recovered spike
        // is flagged, with confidence below 1.
        let series = wall_series(&three_regimes(100.0, 200.0, 100.0), 1.0);
        let candidate =
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .unwrap();
        assert!(candidate.finding.confidence < 1.0);
    }

    #[test]
    fn noisy_resolved_spike_needs_both_gates_significant() {
        // The rise is Mann-Whitney significant, but the tail keeps falling back to the
        // plateau level, so the recovery is not: `rise_p >= alpha || recovery_p >=
        // alpha` rejects it. An `&&` mutant (needing both insignificant to reject)
        // would wrongly flag it. The tail's median is still the baseline, so the
        // "has it recovered" check is satisfied and only the rank gate objects.
        let mut values = vec![100.0; MIN_REGIME];
        values.extend(std::iter::repeat_n(200.0, MIN_REGIME));
        values.extend([100.0, 200.0, 100.0, 200.0, 100.0]);
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
        let wobble = [98.0, 100.0, 102.0, 100.0, 98.0];
        let mut values = wobble.to_vec();
        values.extend(wobble.iter().map(|value| value + 100.0));
        values.extend(wobble);
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
    fn resolved_spike_shorter_than_three_regimes_is_not_a_spike() {
        // One point short of three `min_regime` regimes cannot hold a baseline, an
        // elevated middle, and a recovery, so the `n < min * 3` gate rejects it.
        let mut values = vec![10.0; MIN_REGIME];
        values.extend(std::iter::repeat_n(20.0, MIN_REGIME));
        values.extend(std::iter::repeat_n(10.0, MIN_REGIME - 1));
        let series = series_of(&values);
        assert!(
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .is_none()
        );
    }

    #[test]
    fn resolved_spike_exactly_three_regimes_long_is_a_spike() {
        // The shortest detectable spike holds exactly `3 * min_regime` points: a
        // baseline, an elevated plateau, and a recovery of `min_regime` each. The
        // `n < min * 3` gate must be a strict `<`; a `<=`/`==` slip would reject this
        // minimal spike, whose rise and recovery are both rank significant.
        let series = recovered_spike(10.0, 100.0);
        assert_eq!(series.points.len(), 3 * MIN_REGIME);
        assert!(
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .is_some()
        );
    }

    #[test]
    fn resolved_spike_with_a_still_elevated_tail_is_not_a_spike() {
        // The recovery tail (30) stays far above the baseline (10), so the series has
        // not recovered; an active change-point handles it instead.
        let series = series_of(&three_regimes(10.0, 20.0, 30.0));
        assert!(
            evaluate_resolved_spike(&series, &values_of(&series), &AnalysisConfig::default())
                .is_none()
        );
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
                // Repeated dirty snapshots of one tree: several stored runs, one level.
                points.push((commit, 100.0, true));
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
        // so a branch tip at 130 is a regression against the recent level; judged
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
        let context = branch_context(&no_branch_side, Some(base_merge_base()));
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
}
