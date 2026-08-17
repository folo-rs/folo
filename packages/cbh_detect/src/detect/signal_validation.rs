//! Signal-validation suite: hand-curated "obvious right answer" series that guard
//! against the analysis statistics yielding illogical results.
//!
//! Each case is a data series with an unambiguous shape (an obvious step, a dead-flat
//! line) paired with the outcome each analysis mode's detector is expected to see. The
//! point is not to exercise a particular detector but to pin the end-to-end verdict of
//! the analysis on inputs a human would answer without hesitation — so a future change
//! to the math that starts calling a doubling "no change", or a flat line "a
//! regression", fails here loudly.
//!
//! The verdict is taken through the serial detection oracle [`find_changes`], the same
//! spawner-free entry the rest of the [`findings`](super::findings) unit tests use;
//! `find_changes_spawned_matches_the_serial_pass` proves it produces exactly the
//! findings the spawner-distributed production path
//! ([`find_changes_spawned`](super::find_changes_spawned)) does.
//!
//! Every case is run through a 2 × 2 × 2 matrix:
//!
//! * **Analysis mode (dimension 1).** The two modes are *different detectors*, not
//!   one detector with a flag: [`History`](AnalysisMode::History) locates a change-point
//!   over the whole series, and [`Branch`](AnalysisMode::Branch) compares the branch's
//!   latest regime against the base level across a merge-base split. Because each
//!   inspects a different slice, the *same* series yields different
//!   verdicts per mode, so mode is a curated dimension: every case states the outcome
//!   each mode is expected to see. An obvious mid-series step is a rise to both history
//!   and branch; a lone final-point jump is a rise to branch (a single elevated regime
//!   past the split) but not a sustained historical trend to history.
//!   Branch mode also needs a base side to compare against at all — a case with an empty
//!   base side, or with fewer base-side commits than the evidence floor, leaves it
//!   quiet.
//! * **Absolute scale (dimension 2).** Every case is analysed as-is and scaled up by a
//!   large constant. Every curated move already stands far above its metric kind's
//!   absolute floor, so scaling cannot carry a verdict across that floor and every
//!   scaled verdict must match its as-is reference. Two dedicated tests pin the
//!   opposite: on a small instruction count and on a sub-nanosecond timing move,
//!   scaling promotes a move by carrying its absolute delta across the floor.
//! * **Family size (dimension 3).** Every case is analysed alone and again embedded in
//!   a crowd of companion series. The Benjamini–Hochberg false-discovery
//!   filter sizes its family from the number of series the pass judged, and its
//!   rank-one threshold is `(1/m)·fdr_q`. At a family of one that threshold *is*
//!   `fdr_q`, which every candidate reaching the filter has already cleared by passing
//!   the stricter `change_alpha` — so a single-series batch leaves the multiplicity
//!   stage mathematically inert, and a suite built only of those would pin the detectors
//!   rather than the analysis. The crowd supplies the family the correction needs.
//!   A crowd raises `m`, and
//!   [`companion_crowds_report_nothing_of_their_own`] holds it to reporting nothing in
//!   the direction the mode surfaces, so the only finding a crowded run can carry is the
//!   case's own. Whether it carries it is stated per case as one
//!   [`survives_crowd`](SignalCase::survives_crowd) flag rather than a second full set of
//!   per-mode outcomes, and the matrix pins that flag against the alone verdict in both
//!   directions: a crowd may take a finding away, never add one. For an obvious-answer
//!   case the two verdicts agree, and that invariance is itself the contract — a doubling
//!   is still a doubling in a crowd.

//!
//! Each case also declares its **metric kind**, because the practical-magnitude and
//! scatter floors are per kind: a count must move by whole instructions, a time by a
//! whole nanosecond, an allocation by a whole byte. The matrix therefore carries
//! timing, counter, and allocation cases, each with a move far above its own kind's
//! floors so the scale-invariance dimension holds for all of them.
//!
//! Both directions are still exercised without a polarity dimension: every metric is
//! lower-is-better, so a curated rise is a regression (reported by every mode) and a
//! curated fall is an improvement (reported only by branch mode, which surfaces both
//! directions). That is how the suite still pins the improvement-suppression contract.
//!
//! The check itself is deliberately coarse — "did the analysis report any finding?" —
//! because these inputs are chosen so the *presence* of a finding is the whole
//! question. Detector internals, confidence, and magnitude are covered by the
//! finer-grained unit tests in [`findings`](super::findings).
//!
//! Curated series carry the measurement scatter their metric kind actually shows.
//! Timing metrics run at [`TIMING_NOISE_CV`], the middle of the band the wall-time
//! benchmarks in this project's own stored history occupy; Callgrind counters and
//! allocation counters carry none at all, because those engines re-measure identical
//! code to the same value and their scatter floors keep a zero-scatter window from
//! collapsing the prediction interval. Detection has to be judged against data with as
//! much spread as the real thing, because on near-perfect series every "stays quiet" case
//! is trivial: the prediction interval collapses onto the scatter floor and the rank tests
//! see an unambiguous ordering. The scatter is drawn from a fixed-seed generator keyed by
//! each series' own name, so every run sees identical data while a batch of companions
//! stays independent of one another rather than carrying copies of one sequence.
//!
//! What the generator supplies is realistic *spread*, not realistic *shape*: its deviates
//! are bounded, symmetric, and light-tailed (see [`scattered`]), while real timing noise
//! is right-skewed with occasional large excursions and can settle into distinct modes.
//! Shapes like that reach this suite through recorded series instead.
//!
//! Two cases are exempt from the model: the `stationary_bimodal_noise` rows are verbatim
//! recordings of one real series and carry the dispersion it was measured with — flatly
//! bimodal, oscillating between two levels, which is the pathological shape the generator
//! cannot produce and the one the noise gates are most easily fooled by. A human
//! reading its chart answers "noisy, but nothing changed" without hesitation, so it is
//! exactly the kind of obvious-answer input this suite exists to pin — and it guards the
//! noise gates against reading structured jitter as a step. The pair puts it in front of
//! both detectors: one row hands the whole recording to history mode, the other cuts it
//! into a base window and a tip so branch mode judges it too.

#![cfg_attr(coverage_nightly, coverage(off))]

use std::slice;

use cbh_model::MetricKind;

use crate::detect::findings::find_changes;
use crate::detect::noise_gates::{MIN_REGIME, MIN_SERIES_POINTS};
use crate::detect::recorded::{
    STATIONARY_BIMODAL_BASE, STATIONARY_BIMODAL_HIGH, STATIONARY_BIMODAL_NOISE,
};
use crate::detect::scatter::{TIMING_NOISE_CV, scattered, seed_of};
use crate::detect::{
    AnalysisConfig, AnalysisContext, AnalysisMode, Direction, Series, UnjudgedReason, examples,
};

/// The analysis mode a case is evaluated under — the suite's dimension-1 lever.
///
/// The two modes are genuinely different detectors, so a case declares its expected
/// move per mode rather than sharing one verdict across them.
#[derive(Clone, Copy, Debug)]
enum Mode {
    /// Change-point analysis over the whole series.
    History,
    /// The branch's latest regime against the base level, across a merge-base split.
    Branch,
}

impl Mode {
    /// The two modes, for matrix expansion.
    const ALL: [Self; 2] = [Self::History, Self::Branch];

    /// Whether this mode reports improvements as findings. Only branch does: history is
    /// run here as a regressions-only drift watch (`include_improvements = false`), so
    /// for it an improvement is a non-finding.
    fn reports_improvements(self) -> bool {
        matches!(self, Self::Branch)
    }

    /// The analysis context this mode is evaluated under. `merge_base_index` and
    /// `tip_index` are consulted only by branch mode; history ignores them.
    ///
    /// `include_improvements` is set from [`reports_improvements`](Self::reports_improvements)
    /// so the context matches the mode's intended reporting semantics: branch (which
    /// reports both directions) opts in, history opts out. Branch mode ignores the
    /// flag today, but pinning it consistently keeps the context correct if that changes.
    ///
    fn context(self, merge_base_index: Option<usize>, tip_index: usize) -> AnalysisContext {
        let mode = match self {
            Self::History => AnalysisMode::History,
            Self::Branch => AnalysisMode::Branch,
        };
        AnalysisContext {
            mode,
            config: AnalysisConfig::default(),
            merge_base_index,
            base_ref_index: merge_base_index,
            tip_index,
            include_improvements: self.reports_improvements(),
        }
    }
}

/// The outcome a mode's detector is expected to see in a case — the hand-curated
/// judgment about the raw series shape.
///
/// Combined with the mode's reporting contract this yields the expected finding verdict.
/// Every metric is lower-is-better, so a rise is classified as a regression and a fall as
/// an improvement; the improvement is reported only when the mode surfaces that direction.
#[derive(Clone, Copy, Debug)]
enum Outcome {
    /// The values step up.
    Rise,
    /// The values step down.
    Fall,
    /// Nothing notable moves.
    Quiet,
}

impl Outcome {
    /// Whether this move surfaces as a finding in `mode`.
    fn is_finding(self, mode: Mode) -> bool {
        match self {
            Self::Quiet => false,
            // A rise is a regression (lower-is-better) — every mode reports it.
            Self::Rise => true,
            // A fall is an improvement — reported only where the mode reports both
            // directions.
            Self::Fall => mode.reports_improvements(),
        }
    }
}

/// Where a case's dispersion comes from.
#[derive(Clone, Copy, Debug)]
enum Scatter {
    /// The declared values are flat regime levels, and the suite adds the measurement
    /// scatter the case's metric kind actually shows (see [`with_noise`]).
    Modelled,
    /// The declared values were recorded from a real series and already carry their own
    /// dispersion, so they are analysed exactly as given.
    Recorded,
}

/// One curated series — its base and branch sides — and the outcome each mode is
/// expected to see in it.
struct SignalCase {
    /// Human-readable case name, surfaced in assertion failures. It also seeds the
    /// case's scatter, so each case draws its own sequence.
    name: &'static str,
    /// The metric kind the case is analysed as. It selects both the practical-magnitude
    /// and scatter floors the verdict turns on and the measurement scatter the values
    /// carry.
    kind: MetricKind,
    /// The base-side (unscaled) regime levels, oldest-first: the commits at or before
    /// the merge-base. May be empty, which leaves branch mode without a base side to
    /// compare against, so it stays quiet. The base/branch split matters only to branch
    /// mode; history sees the whole concatenated series and reads these values as
    /// ordinary leading points, indifferent to which side they came from.
    base: Vec<f64>,
    /// The branch-side (unscaled) regime levels, oldest-first: the commits past the
    /// merge-base. May be empty.
    branch: Vec<f64>,
    /// The outcome history mode's change-point detector is expected to see.
    expected_history: Outcome,
    /// The outcome branch mode is expected to see.
    expected_branch: Outcome,
    /// Whether the case's finding still stands once the false-discovery family grows to
    /// [`CROWD_COMPANIONS`] companions.
    ///
    /// A crowd reports nothing of its own — [`companion_crowds_report_nothing_of_their_own`]
    /// enforces that — and the matrix asserts that a crowded verdict never exceeds the
    /// alone verdict, so per mode the crowd verdict is either the alone verdict or
    /// silence. That is what reduces the crowd outcome to a boolean rather than a second
    /// [`Outcome`]. The boolean is shared across modes because every curated case
    /// answers the crowd the same way in both; a case that survived in one mode and was
    /// crowded out in the other would be stated as two rows, one per mode. Every
    /// obvious-answer case keeps its verdict, and that invariance is the contract; the
    /// deliberately marginal case does not, which is what proves the dimension is
    /// load-bearing.
    survives_crowd: bool,
    /// Where the case's dispersion comes from.
    scatter: Scatter,
}

impl SignalCase {
    /// A case on `kind` that both modes are expected to stay quiet on, whose declared
    /// levels carry modelled scatter, and whose verdict survives the crowd — the common
    /// shape every case is stated as a deviation from.
    fn new(name: &'static str, kind: MetricKind) -> Self {
        Self {
            name,
            kind,
            base: Vec::new(),
            branch: Vec::new(),
            expected_history: Outcome::Quiet,
            expected_branch: Outcome::Quiet,
            survives_crowd: true,
            scatter: Scatter::Modelled,
        }
    }

    /// The base-side levels, oldest-first.
    fn base(mut self, values: Vec<f64>) -> Self {
        self.base = values;
        self
    }

    /// The branch-side levels, oldest-first.
    fn branch(mut self, values: Vec<f64>) -> Self {
        self.branch = values;
        self
    }

    /// The outcomes history mode and branch mode are respectively expected to see.
    fn expects(mut self, history: Outcome, branch: Outcome) -> Self {
        self.expected_history = history;
        self.expected_branch = branch;
        self
    }

    /// Declares that the crowd's tighter false-discovery threshold rejects this case's
    /// finding.
    fn crowded_out(mut self) -> Self {
        self.survives_crowd = false;
        self
    }

    /// Declares the values a verbatim recording, to be analysed without added scatter.
    fn recorded(mut self) -> Self {
        self.scatter = Scatter::Recorded;
        self
    }

    /// The whole series as the analysis receives it: the base side followed by the
    /// branch side, oldest-first, carrying the dispersion the case declares.
    fn values(&self) -> Vec<f64> {
        let levels = [self.base.as_slice(), self.branch.as_slice()].concat();
        match self.scatter {
            Scatter::Modelled => with_noise(&levels, self.kind, seed_of(self.name)),
            Scatter::Recorded => levels,
        }
    }

    /// The first-parent merge-base split index handed to branch mode: the last base-side
    /// point, or `None` when there is no base side (branch mode then has nothing to
    /// compare against and stays quiet). History ignores it.
    fn merge_base_index(&self) -> Option<usize> {
        self.base.len().checked_sub(1)
    }

    /// The context commit index for branch mode: the newest declared point.
    fn tip_index(&self) -> usize {
        if self.branch.is_empty() {
            return self.base.len();
        }
        self.base
            .len()
            .checked_add(self.branch.len())
            .and_then(|points| points.checked_sub(1))
            .unwrap_or(0)
    }

    /// The outcome `mode` is expected to see in this case.
    fn expected_outcome(&self, mode: Mode) -> Outcome {
        match mode {
            Mode::History => self.expected_history,
            Mode::Branch => self.expected_branch,
        }
    }
}

/// The coefficient of variation of a metric that re-measures identical code to
/// identical values.
const EXACT: f64 = 0.0;

/// The coefficient of variation a curated series of `kind` carries.
///
/// Callgrind simulates the processor rather than timing it, and the allocation tracker
/// counts whole events, so both reproduce a value exactly across runs of unchanged
/// code — zero scatter *is* the realistic model for them. That costs the branch-mode
/// prediction interval nothing, because both kinds carry a scatter floor
/// (`SCATTER_FLOOR_COUNT`, `SCATTER_FLOOR_ALLOC`) that bounds the standard error from
/// below when a window repeats one value. Timing metrics have no such floor, and no
/// quantum to justify one, so their realistic scatter is what keeps the interval
/// usable.
fn noise_cv(kind: MetricKind) -> f64 {
    match kind {
        MetricKind::WallTime | MetricKind::ProcessorTime => TIMING_NOISE_CV,
        MetricKind::InstructionCount
        | MetricKind::ConditionalBranches
        | MetricKind::IndirectBranches
        | MetricKind::AllocatedBytes
        | MetricKind::AllocationCount => EXACT,
    }
}

/// `values` carrying `kind`'s realistic measurement scatter, drawn from `seed`.
///
/// The scatter is relative to each point's own level, so scaling a whole series scales
/// its scatter with it and the suite's scale-invariance dimension stays exact.
fn with_noise(values: &[f64], kind: MetricKind, seed: u64) -> Vec<f64> {
    scattered(values, noise_cv(kind), seed)
}

/// A regime of `count` points at exactly `value`.
///
/// Measurement scatter is added when the case's values are assembled (see
/// [`SignalCase::values`]), so a case declares the levels it means and nothing else.
fn run_of(value: f64, count: usize) -> Vec<f64> {
    vec![value; count]
}

/// How many companion series join a case in the crowd of dimension 3.
///
/// The count is bounded on both sides by the cases themselves, and both bounds are
/// measured rather than predicted. The deliberately marginal case survives up to seven
/// companions and falls silent at eight, so eight is the floor below which dimension 3
/// stops discriminating anything. The weakest genuine case — the context run above a
/// freshly shifted base, whose branch-side evidence is a single point — survives up to
/// 309 companions and falls silent at 310, so 309 is the ceiling above which the suite
/// starts denying real signals. Neither edge is a place to sit, so the crowd is the
/// geometric midpoint of the admissible window, `floor(sqrt(8 * 309))`, which leaves a
/// factor of roughly six of margin on each side.
const CROWD_COMPANIONS: usize = 49;

/// The crowd size of a case analysed on its own, with no companions at all.
const ALONE: usize = 0;

/// The benchmark name the curated series of every case is stored under.
const CURATED_NAME: &str = "curated";

/// The scale multiple dimension 2 applies on top of each as-is series.
///
/// Large enough to carry a move sitting just under an absolute floor well across it,
/// which is what the two dedicated absolute-floor tests need. Every matrix case already
/// clears its floor, so there the multiple only has to leave the verdict alone.
const SCALE_MULTIPLE: f64 = 1000.0;

/// The length of each of the two regimes the dedicated absolute-floor tests are built
/// from.
///
/// Long enough that the step between them is statistically unmistakable in both modes,
/// so the verdict turns on the absolute floor and on nothing else.
const FLOOR_REGIME_POINTS: usize = 50;

/// The merge base the dedicated absolute-floor tests split at: the last commit of the
/// first regime, so branch mode sees the whole second regime as the branch side.
const FLOOR_MERGE_BASE: usize = FLOOR_REGIME_POINTS - 1;

/// The hand-curated cases. New "obvious answer" series are added as one row each.
fn cases() -> Vec<SignalCase> {
    vec![
        // An unmistakable sustained doubling halfway through. History and branch (split
        // at the step) both see a rise.
        SignalCase::new("doubling_step", MetricKind::WallTime)
            .base(run_of(100.0, 50))
            .branch(run_of(200.0, 50))
            .expects(Outcome::Rise, Outcome::Rise),
        // The same obvious doubling as the first case, but with no base side. Branch
        // mode has nothing to compare the branch against, so it must stay quiet even
        // though history still sees the rise over the whole series.
        SignalCase::new("doubling_without_base", MetricKind::WallTime)
            .branch([run_of(100.0, 50), run_of(200.0, 50)].concat())
            .expects(Outcome::Rise, Outcome::Quiet),
        // The mirror image: a sustained halving. Same mode geometry, opposite direction,
        // so it exercises the improvement-reporting path (surfaced only by branch mode).
        SignalCase::new("halving_step", MetricKind::WallTime)
            .base(run_of(200.0, 50))
            .branch(run_of(100.0, 50))
            .expects(Outcome::Fall, Outcome::Fall),
        // A jump confined to the final commit. Branch (split just before the jump) sees
        // the rise; history does not, since one trailing point is not a sustained trend.
        SignalCase::new("tip_spike", MetricKind::WallTime)
            .base(run_of(100.0, 99))
            .branch(run_of(200.0, 1))
            .expects(Outcome::Quiet, Outcome::Rise),
        // The mirror image at the tip: the final commit drops.
        SignalCase::new("tip_drop", MetricKind::WallTime)
            .base(run_of(200.0, 99))
            .branch(run_of(100.0, 1))
            .expects(Outcome::Quiet, Outcome::Fall),
        // A dead-flat line: nothing moved, so no mode should ever flag it.
        SignalCase::new("flat_line", MetricKind::WallTime)
            .base(run_of(100.0, 50))
            .branch(run_of(100.0, 50)),
        // A stationary but very noisy real-world series (a wall-time metric whose value
        // oscillates between ~13 and ~25-29 across its whole history). A human reads the
        // chart as "noisy, nothing changed", yet a naive change-point split lands on the
        // dominant mode of each side and — because the median-absolute residual then
        // collapses — reads as a regression. The regime-separation gate rejects it: the
        // two levels overlap far too much to be distinct populations. History sees the
        // whole series and must stay quiet; branch has no branch side.
        SignalCase::new("stationary_bimodal_noise", MetricKind::WallTime)
            .base(STATIONARY_BIMODAL_NOISE.to_vec())
            .recorded(),
        // The same recording judged by branch mode, which reads only the recent base
        // window and one context commit. The window is cut where the recording happens to
        // end on five consecutive low-mode commits, and the tip sits at the high mode the
        // series reaches on roughly half of its commits — an entirely ordinary value.
        // Accepting that trailing run as the current base regime would discard the rest
        // of the window and collapse the scatter estimate, turning the ordinary tip into
        // a large and near-certain regression, so branch mode must decline the split and
        // stay quiet. History reads twenty points of the same oscillation and is quiet
        // for the reason the row above is.
        SignalCase::new("stationary_bimodal_noise_branch_tip", MetricKind::WallTime)
            .base(
                STATIONARY_BIMODAL_NOISE
                    .get(..STATIONARY_BIMODAL_BASE)
                    .unwrap()
                    .to_vec(),
            )
            .branch(vec![STATIONARY_BIMODAL_HIGH])
            .recorded(),
        // A branch that got slower but was fixed in the last commit.
        // History sees the regression, but branch sees only the final commit and must stay quiet.
        SignalCase::new("branch_with_regression_then_fix", MetricKind::WallTime)
            .base(run_of(100.0, 50))
            .branch([run_of(200.0, 49), run_of(100.0, 1)].concat())
            .expects(Outcome::Rise, Outcome::Quiet),
        // A short history whose final commit sits a little high — the shape the batch
        // false positives of issue #428 take. Both modes stay quiet, for two unrelated
        // reasons worth having pinned together: history rejects a one-point regime
        // (`MIN_REGIME` demands five), and branch mode sees only nine base-side commit
        // levels, under the `MIN_SERIES_POINTS` evidence floor, so it declines to test
        // the series at all. This is deliberately distinct from `tip_spike`, which has a
        // long base and where branch mode legitimately does report.
        SignalCase::new(
            "a_lone_elevated_final_point_is_not_a_step",
            MetricKind::WallTime,
        )
        .base(run_of(100.0, MIN_SERIES_POINTS - 1))
        .branch(run_of(110.0, 1)),
        // A clean 30% step over the shortest regimes both modes will judge. Far more
        // marginal than the doubling above, so it is a genuine test of crowd survival
        // rather than a formality: it must keep its verdict in a crowd.
        SignalCase::new("a_clean_step_survives_a_crowd", MetricKind::WallTime)
            .base(run_of(100.0, MIN_SERIES_POINTS))
            .branch(run_of(130.0, MIN_SERIES_POINTS))
            .expects(Outcome::Rise, Outcome::Rise),
        // A real but small step over the shortest regimes a change-point can be built
        // from. This case exists to prove the family dimension is load-bearing: without
        // at least one case whose verdict flips, a regression that ignored family size
        // entirely would still satisfy the whole dimension. History is the discriminating
        // mode — it reports the step alone and is talked out of it by the crowd's tighter
        // threshold — while branch mode stays quiet either way, since five base-side
        // commit levels are under its evidence floor.
        SignalCase::new(
            "a_marginal_step_does_not_survive_a_crowd",
            MetricKind::WallTime,
        )
        .base(run_of(100.0, MIN_REGIME))
        .branch(run_of(108.0, MIN_REGIME))
        .expects(Outcome::Rise, Outcome::Quiet)
        .crowded_out(),
        // Matched pair, part one. The base branch itself stepped down recently, and the
        // context run sits above the level it stepped down to. Branch mode narrows its
        // comparison to the current base regime and reports the rise; measured against
        // the whole stale window the tip would be lost under the old high level. History
        // reads the series as one large fall — an improvement it does not report.
        SignalCase::new(
            "a_branch_tip_above_a_freshly_shifted_base_is_reported",
            MetricKind::WallTime,
        )
        .base([run_of(200.0, 11), run_of(100.0, MIN_REGIME)].concat())
        .branch(run_of(130.0, 1))
        .expects(Outcome::Fall, Outcome::Rise),
        // Matched pair, part two: the same freshly shifted base, with a context run that
        // agrees with the current base regime. Branch mode must stay quiet. Together the
        // pair states that narrowing the comparison to the current base regime restores
        // sensitivity without manufacturing findings on branches that changed nothing —
        // either half alone would be satisfied by a detector that had lost one of those
        // two properties.
        SignalCase::new(
            "a_branch_tip_matching_a_freshly_shifted_base_is_quiet",
            MetricKind::WallTime,
        )
        .base([run_of(200.0, 11), run_of(100.0, MIN_REGIME)].concat())
        .branch(run_of(100.0, 1))
        .expects(Outcome::Fall, Outcome::Quiet),
        // A counter metric stepping by 200 instructions, forty times the five-count
        // absolute floor and far above the relative floors. Callgrind
        // reproduces identical code exactly, so the series carries no scatter and the
        // count scatter floor is what keeps the branch-mode prediction interval usable.
        SignalCase::new(
            "a_counter_step_far_above_the_count_floors_is_reported",
            MetricKind::InstructionCount,
        )
        .base(run_of(1000.0, MIN_SERIES_POINTS))
        .branch(run_of(1200.0, MIN_SERIES_POINTS))
        .expects(Outcome::Rise, Outcome::Rise),
        // An allocation metric stepping by a kilobyte, three orders of magnitude above
        // the one-byte absolute floor. An allocator hands out whole bytes and repeats
        // the same requests run after run, so this series is exact too and leans on the
        // allocation scatter floor in branch mode.
        SignalCase::new(
            "an_allocation_step_far_above_the_allocation_floors_is_reported",
            MetricKind::AllocatedBytes,
        )
        .base(run_of(4096.0, MIN_SERIES_POINTS))
        .branch(run_of(5120.0, MIN_SERIES_POINTS))
        .expects(Outcome::Rise, Outcome::Rise),
    ]
}

/// `count` flat companion series at `level`, laid out to enlarge the false-discovery
/// family of a case whose merge base sits at `merge_base`.
///
/// A companion counts towards the family only if it is *judged*, which is why each
/// carries as much evidence as its position allows: the base-side commits ending at the
/// merge base, capped at `MIN_SERIES_POINTS`, plus a branch-side tip that pads the
/// series out whenever the base side alone falls short of that floor. History mode
/// therefore always judges a companion. Branch mode judges one exactly when the merge
/// base sits at least `MIN_SERIES_POINTS` commits in, since a companion cannot reach
/// further back than the shared merge-base split allows — so for a case whose own base
/// side is under that floor, and which branch mode consequently declines to test, the
/// crowd is a history-mode family only.
///
/// Holding them at the floor is deliberate rather than incidental: the crowd is rebuilt
/// for every case, mode, and scale, so its cost must not grow with the case's own
/// length. Each draws its metric kind's realistic scatter from its own seed, so the
/// crowd is a set of independent noisy series rather than an artificially clean backdrop.
fn companions(
    count: usize,
    kind: MetricKind,
    level: f64,
    merge_base: Option<usize>,
    tip_index: usize,
) -> Vec<Series> {
    let branch_start = merge_base.map_or(0, |index| index.checked_add(1).unwrap());
    let base_points = branch_start.min(MIN_SERIES_POINTS);
    let topo_start = branch_start.checked_sub(base_points).unwrap();
    let branch_points = merge_base.map_or(MIN_SERIES_POINTS, |index| {
        tip_index.saturating_sub(index).max(1)
    });
    let points = base_points.checked_add(branch_points).unwrap();

    (0..count)
        .map(|index| {
            let name = format!("companion{index}");
            let values = with_noise(&run_of(level, points), kind, seed_of(&name));
            let series = examples::series(&name, &values, kind, topo_start);
            match merge_base {
                Some(base_ref) => examples::with_base_window(series, base_ref),
                None => series,
            }
        })
        .collect()
}

/// The arithmetic mean of `values` — the level companions sit at, so a crowd shares the
/// order of magnitude of the case it accompanies and scales along with it.
fn mean_of(values: &[f64]) -> f64 {
    let count = u32::try_from(values.len()).unwrap();
    if count == 0 {
        return 0.0;
    }
    let total: f64 = values.iter().sum();
    total / f64::from(count)
}

/// Runs the serial detection oracle on the curated series — alone when `crowd` is
/// [`ALONE`], otherwise joined by that many flat companions — and reports whether the
/// curated series raised a finding.
///
/// Every companion is flat, so any surviving finding belongs to the curated series. That
/// is asserted rather than assumed: a companion that started reporting moves of its own
/// would silently change what this suite measures.
///
/// The size of the family the false-discovery filter divides by is asserted too, in both
/// modes, because a crowd that failed to enlarge the family would leave dimension 3 inert
/// without failing anything — the precise blind spot the dimension exists to close. See
/// [`expected_judged`] for what each mode is entitled to judge.
fn raises_finding(
    values: &[f64],
    kind: MetricKind,
    context: &AnalysisContext,
    crowd: usize,
) -> bool {
    let curated = examples::series(CURATED_NAME, values, kind, 0);
    let curated = match (context.mode, context.merge_base_index) {
        (AnalysisMode::Branch, Some(base_ref)) => examples::with_base_window(curated, base_ref),
        _ => curated,
    };
    let curated_id = curated.id.qualified();
    let mut batch = vec![curated];
    batch.extend(companions(
        crowd,
        kind,
        mean_of(values),
        context.merge_base_index,
        context.tip_index,
    ));

    let detection = find_changes(&batch, context);
    assert_eq!(
        detection.census.judged(),
        expected_judged(values, context, batch.len()),
        "the family the false-discovery correction divides by is not the one the batch \
         and the merge base imply"
    );
    for finding in &detection.findings {
        assert_eq!(
            finding.id.qualified(),
            curated_id,
            "a companion series raised a finding of its own"
        );
    }
    !detection.findings.is_empty()
}

/// How many series of a [`raises_finding`] batch of `batch` series the pass is expected to
/// judge — the family the false-discovery correction divides by.
///
/// History mode judges all of them: every curated series and every companion carries at
/// least [`MIN_SERIES_POINTS`] points. Branch mode judges a series only when the shared
/// merge base leaves it a base window of at least that many commits and at least one
/// commit past the split. Companions are built to that floor, so one merge base decides
/// them all at once — and it decides the curated series' base side with them, since that
/// side is the case's own leading `merge_base + 1` points. Below the floor the family is
/// therefore empty rather than merely smaller.
fn expected_judged(values: &[f64], context: &AnalysisContext, batch: usize) -> usize {
    if context.mode == AnalysisMode::History {
        return batch;
    }
    let Some(merge_base) = context.merge_base_index else {
        return 0;
    };
    let base_points = merge_base.checked_add(1).unwrap();
    if base_points < MIN_SERIES_POINTS {
        return 0;
    }
    // Companions always carry a commit past the merge base; the curated series carries one
    // only when the case declares a branch side.
    let curated = usize::from(values.len() > base_points);
    batch.checked_sub(1).unwrap().checked_add(curated).unwrap()
}

/// `values`, each multiplied by `scale`.
fn scaled(values: &[f64], scale: f64) -> Vec<f64> {
    values.iter().map(|&value| value * scale).collect()
}

#[test]
fn curated_signals_match_expected_verdicts() {
    for case in cases() {
        let values = case.values();
        let kind = case.kind;
        for mode in Mode::ALL {
            let context = mode.context(case.merge_base_index(), case.tip_index());
            let expected = case.expected_outcome(mode).is_finding(mode);

            // Dimension 1: the as-is verdict under this mode matches the hand-picked
            // expectation.
            let alone = raises_finding(&values, kind, &context, ALONE);
            assert_eq!(
                alone, expected,
                "case '{}' mode={mode:?}: expected finding={expected}, got {alone}",
                case.name,
            );

            // Dimension 2: scaling a series that already clears its kind's absolute
            // floor leaves the verdict unchanged.
            let scaled_alone =
                raises_finding(&scaled(&values, SCALE_MULTIPLE), kind, &context, ALONE);
            assert_eq!(
                scaled_alone, alone,
                "case '{}' mode={mode:?}: scaling by {SCALE_MULTIPLE} changed the verdict",
                case.name,
            );

            // Dimension 3: a crowd of judged companions that report nothing of their own
            // keeps every genuine verdict and rejects only what the case declares
            // marginal.
            let expected_in_crowd = expected && case.survives_crowd;
            let crowded = raises_finding(&values, kind, &context, CROWD_COMPANIONS);
            assert!(
                !crowded || alone,
                "case '{}' mode={mode:?}: a crowd manufactured a finding the case does not \
                 raise on its own",
                case.name,
            );
            assert_eq!(
                crowded, expected_in_crowd,
                "case '{}' mode={mode:?}: in a crowd expected finding={expected_in_crowd}, \
                 got {crowded}",
                case.name,
            );

            // The scale and family dimensions are independent: scale invariance holds in
            // a crowd too.
            let scaled_crowded = raises_finding(
                &scaled(&values, SCALE_MULTIPLE),
                kind,
                &context,
                CROWD_COMPANIONS,
            );
            assert_eq!(
                scaled_crowded, crowded,
                "case '{}' mode={mode:?}: scaling by {SCALE_MULTIPLE} changed the crowd verdict",
                case.name,
            );
        }
    }
}

#[test]
fn scaling_a_quantized_move_can_clear_the_absolute_floor() {
    // A 60 -> 64 instruction-count step clears both relative floors but not the
    // five-count absolute floor. Scaling preserves its shape and relative magnitude
    // while lifting the absolute delta above the floor, so both analysis modes may
    // legitimately change from quiet to finding.
    let kind = MetricKind::InstructionCount;
    let values = [
        run_of(60.0, FLOOR_REGIME_POINTS),
        run_of(64.0, FLOOR_REGIME_POINTS),
    ]
    .concat();
    let scaled_values = scaled(&values, SCALE_MULTIPLE);

    for mode in Mode::ALL {
        let context = mode.context(Some(FLOOR_MERGE_BASE), values.len().saturating_sub(1));
        assert!(!raises_finding(&values, kind, &context, ALONE));
        assert!(raises_finding(&scaled_values, kind, &context, ALONE));
    }
}

#[test]
fn scaling_a_sub_nanosecond_move_can_clear_the_absolute_floor() {
    // A 2.0 -> 2.4 ns step is a 20% move, clearing every relative floor comfortably, but
    // spans only 0.4 ns and so falls under `PRACTICAL_ABSOLUTE_TIME`. Timing metrics are
    // floored on absolute magnitude exactly as counted ones are: a move of well under a
    // nanosecond an iteration is not worth acting on whatever percentage it works out
    // to. Scaling preserves the shape and the relative magnitude while carrying the
    // absolute delta over the floor, so both modes legitimately change from quiet to
    // finding.
    //
    // This deliberately breaks the matrix's scale-invariance assertion, which is why it
    // is a dedicated test rather than a matrix row.
    let kind = MetricKind::WallTime;
    let values = with_noise(
        &[
            run_of(2.0, FLOOR_REGIME_POINTS),
            run_of(2.4, FLOOR_REGIME_POINTS),
        ]
        .concat(),
        kind,
        seed_of("sub_nanosecond_move"),
    );
    let scaled_values = scaled(&values, SCALE_MULTIPLE);

    for mode in Mode::ALL {
        let context = mode.context(Some(FLOOR_MERGE_BASE), values.len().saturating_sub(1));
        assert!(!raises_finding(&values, kind, &context, ALONE));
        assert!(raises_finding(&scaled_values, kind, &context, ALONE));
    }
}

#[test]
fn companion_crowds_report_nothing_of_their_own() {
    // Dimension 3 collapses the crowd outcome to a single boolean per case, which is only
    // meaningful if a crowd contributes no finding of its own. That is enforced here
    // rather than assumed: every crowd the matrix builds is analysed as its own batch,
    // under both modes and both scales, and must come back empty.
    //
    // The stricter reading — that companions raise no *candidate* at all — is not a
    // property flat noisy series can have. Fifty of them tested at `change_alpha` will now
    // and then throw one that clears it, which is what that threshold means. What they may
    // not do is throw one in the direction the mode reports, so the second assertion pins
    // the crowd's whole contribution to the improvement side, where a regressions-only
    // drift watch discards it.
    for case in cases() {
        let values = case.values();
        for scale in [1.0, SCALE_MULTIPLE] {
            let level = mean_of(&scaled(&values, scale));
            let crowd = companions(
                CROWD_COMPANIONS,
                case.kind,
                level,
                case.merge_base_index(),
                case.tip_index(),
            );
            for mode in Mode::ALL {
                let context = mode.context(case.merge_base_index(), case.tip_index());
                let detection = find_changes(&crowd, &context);
                assert!(
                    detection.findings.is_empty(),
                    "case '{}' mode={mode:?} scale={scale}: the crowd reported {} findings \
                     of its own",
                    case.name,
                    detection.findings.len(),
                );

                let both_directions = AnalysisContext {
                    include_improvements: true,
                    ..mode.context(case.merge_base_index(), case.tip_index())
                };
                for finding in find_changes(&crowd, &both_directions).findings {
                    assert_eq!(
                        finding.direction,
                        Direction::Improvement,
                        "case '{}' mode={mode:?} scale={scale}: companion '{}' raised a \
                         regression of its own",
                        case.name,
                        finding.id.qualified(),
                    );
                }
            }
        }
    }
}

#[test]
fn a_batch_of_flat_noisy_series_raises_nothing() {
    // The direct analogue of issue #428: a batch of roughly 300 real series reported 17
    // "regressions" and — every single time — exactly zero improvements, a rotating cast
    // of benchmarks that had not changed. A one-sided finding list over a stationary
    // suite is the signature of a multiplicity failure rather than of real regressions,
    // so a whole batch of flat series at production scatter must come back empty in both
    // directions and in both modes.
    //
    // Each series is independently seeded, so this is a batch of unrelated noisy
    // benchmarks rather than one series repeated. The census assertion matters as much as
    // the finding assertion: silence proves something about the gates only when the
    // series reached them.
    //
    // The batch is sized so that the false-discovery correction is what produces the
    // silence. Two of these series wander far enough for the per-series gates to raise a
    // candidate — a drift and a change point, both below `change_alpha` — so both are
    // reported by a detector judging each series on its own. A judged family of forty puts
    // the rank-one threshold at 0.0025 and rejects them. The positive control below pins
    // that: shrink the family and this fixture reports two regressions that never happened.
    //
    // Silence here is a property of this fixture rather than a universal guarantee: the
    // correction bounds the false-discovery rate at `fdr_q` instead of driving it to
    // zero, so a stationary batch may legitimately surface a discovery, and varying this
    // fixture's series length finds lengths where one does. What this test pins is that
    // the correction is what suppresses the candidates above — not that noise can never
    // produce a finding.
    const FLAT_SERIES: usize = 40;
    const POINTS: usize = 20;
    const MERGE_BASE: usize = 15;
    const LEVEL: f64 = 100.0;

    let kind = MetricKind::WallTime;
    let batch: Vec<Series> = (0..FLAT_SERIES)
        .map(|index| {
            let name = format!("flat{index}");
            let values = with_noise(&run_of(LEVEL, POINTS), kind, seed_of(&name));
            examples::with_base_window(examples::series(&name, &values, kind, 0), MERGE_BASE)
        })
        .collect();

    for mode in Mode::ALL {
        let context = mode.context(Some(MERGE_BASE), POINTS.saturating_sub(1));
        let detection = find_changes(&batch, &context);
        assert_eq!(
            detection.census.judged(),
            FLAT_SERIES,
            "mode={mode:?}: every flat series must be judged, or the silence proves nothing",
        );
        assert!(
            detection.findings.is_empty(),
            "mode={mode:?}: a stationary batch reported {} findings",
            detection.findings.len(),
        );

        // The positive control: the same series judged one at a time, where the family is
        // one and the correction is inert. Two of them then report a regression that never
        // happened, which is what makes the silence above a property of the correction
        // rather than of series that never reached it.
        let solo: Vec<(Direction, String)> = batch
            .iter()
            .flat_map(|series| find_changes(slice::from_ref(series), &context).findings)
            .map(|finding| (finding.direction, finding.id.qualified()))
            .collect();
        let expected: &[(Direction, &str)] = match mode {
            Mode::History => &[
                (Direction::Regression, "flat3/case"),
                (Direction::Regression, "flat25/case"),
            ],
            Mode::Branch => &[],
        };
        assert_eq!(
            solo.len(),
            expected.len(),
            "mode={mode:?}: judged one at a time these series reported {solo:?}",
        );
        for (found, want) in solo.iter().zip(expected) {
            assert_eq!(found.0, want.0, "mode={mode:?}");
            assert_eq!(found.1, want.1, "mode={mode:?}");
        }
    }
}

#[test]
fn evidence_below_the_minimum_is_not_judged() {
    // Two regimes, each one point short of `MIN_REGIME`, stepping by an unmistakable
    // 30%. The step is as obvious as any in the matrix and the analysis still says
    // nothing about it — because it never tested it. "We have no evidence" and "we
    // looked and nothing moved" are very different statements to someone reading a
    // report, so the census is asserted alongside the empty finding list: silence on this
    // series is accounted for as missing evidence rather than as a verdict.
    let kind = MetricKind::WallTime;
    let values = with_noise(
        &[run_of(100.0, MIN_REGIME - 1), run_of(130.0, MIN_REGIME - 1)].concat(),
        kind,
        seed_of("below_minimum_evidence"),
    );
    let series = examples::series(CURATED_NAME, &values, kind, 0);

    let detection = find_changes(
        slice::from_ref(&series),
        &Mode::History.context(None, values.len().saturating_sub(1)),
    );

    assert!(detection.findings.is_empty());
    assert_eq!(detection.census.judged(), 0);
    assert_eq!(
        detection.census.reasons().collect::<Vec<_>>(),
        vec![(UnjudgedReason::TooFewPoints, 1)],
    );
}
