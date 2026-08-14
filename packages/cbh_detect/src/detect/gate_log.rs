//! Observation of the gates detection applies: which ran, what each computed, what it
//! compared that against, and which one declined the candidate.
//!
//! A verdict says only that a series was reported or was not. That is enough to act on
//! but not enough to explain, and an unexplained silence is indistinguishable from a
//! broken detector. A [`GateLog`] is the explanation: the detectors record every gate
//! decision into one as they make it, from the same expression the decision is branched
//! on, so a log describes what detection actually did rather than a reconstruction of it.
//!
//! Recording is opt-in per evaluation. Production passes a [`disabled`](GateLog::disabled)
//! log, which discards every recording and never allocates; tests and the documentation
//! figures pass a [`recording`](GateLog::recording) one. The gates themselves run
//! identically either way.

/// What a detection pass decided, gate by gate, about one series.
///
/// The log is the observable form of the gate chain described in `docs/design.md`
/// ("Analysis") — the ordered sequence of independent checks a candidate must survive.
/// Each check appends one [`GateOutcome`] as it is made, so
/// [`entries`](Self::entries) reads as the narrative of an evaluation and
/// [`declined_by`](Self::declined_by) names its conclusion.
///
/// Gates short-circuit: the first one to decline ends its detector's evaluation, so a log
/// ends at that gate and says nothing about the gates that would have followed. This is
/// the log being faithful — those gates genuinely did not run — rather than a limitation.
/// A log therefore proves what declined a candidate, never what else would have.
///
/// One log covers one evaluated series, across every detector that series ran. Both
/// history detectors run on the same series before arbitration picks between them, so
/// each outcome carries the [`GateStage`] that produced it and a reader must filter by
/// stage (see [`declined_by_stage`](Self::declined_by_stage)) to follow a single detector.
#[derive(Debug)]
pub struct GateLog {
    /// The outcomes recorded so far, or `None` for a disabled log.
    ///
    /// Distinguishing the two states by the `Option` rather than by a flag beside a
    /// `Vec` keeps a disabled log free of an allocation it would never fill.
    entries: Option<Vec<GateOutcome>>,
}

impl GateLog {
    /// A log that records nothing — what a production analysis pass carries.
    #[must_use]
    pub fn disabled() -> Self {
        Self { entries: None }
    }

    /// A recorder bound to `stage`, for the detector currently evaluating.
    pub(crate) fn stage(&mut self, stage: GateStage) -> StageLog<'_> {
        StageLog { log: self, stage }
    }

    /// Appends `outcome` when recording; a disabled log discards it.
    fn record(&mut self, outcome: GateOutcome) {
        if let Some(entries) = &mut self.entries {
            entries.push(outcome);
        }
    }
}

/// Turning recording on and reading back what was recorded.
///
/// Recording is an inspection facility, so it is offered only to in-workspace consumers:
/// this crate's tests and the documentation figures. The recording *calls* are compiled
/// unconditionally — every gate reports its decision on every build — so the chain an
/// observer reads is the chain production executed, which is the only reason reading it
/// proves anything.
#[cfg(any(test, feature = "private-test-util"))]
impl GateLog {
    /// A log that records every gate decision the evaluation makes.
    #[must_use]
    pub fn recording() -> Self {
        Self {
            entries: Some(Vec::new()),
        }
    }

    /// The gate decisions, in the order the detectors made them.
    ///
    /// Empty for a disabled log, and empty for a recording log whose evaluation never
    /// reached a gate.
    #[must_use]
    pub fn entries(&self) -> &[GateOutcome] {
        self.entries.as_deref().unwrap_or_default()
    }

    /// The first gate that declined, across every stage.
    ///
    /// `None` means no recorded gate declined, which for a recording log is how a
    /// reported finding looks.
    #[must_use]
    pub fn declined_by(&self) -> Option<Gate> {
        self.entries()
            .iter()
            .find(|outcome| !outcome.passed)
            .map(|outcome| outcome.gate)
    }

    /// The first gate `stage` declined at.
    ///
    /// Two history detectors run on every series, so asking the log as a whole answers
    /// for whichever of them declined first. This asks one detector's question.
    #[must_use]
    pub fn declined_by_stage(&self, stage: GateStage) -> Option<Gate> {
        self.entries()
            .iter()
            .find(|outcome| outcome.stage == stage && !outcome.passed)
            .map(|outcome| outcome.gate)
    }
}

impl Default for GateLog {
    /// A [`disabled`](GateLog::disabled) log, so a caller that does not ask to observe
    /// gates does not pay to.
    fn default() -> Self {
        Self::disabled()
    }
}

/// One gate's decision: which stage applied it, what it computed, what it compared that
/// against, and whether the candidate survived.
///
/// `value` and `threshold` are both absent for a gate that is boolean-shaped: interval
/// disjointness compares two ranges for overlap and yields no single number, and a gate
/// asking whether a statistic could be formed at all has nothing to measure. The pair is
/// `Option<f64>` rather than an enum of numeric and boolean shapes because every consumer
/// — an assertion, a rendered table — treats the two side by side, so "this gate has no
/// number to show" is naturally the empty case of that pair rather than a separate shape
/// each consumer must match on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateOutcome {
    /// Which detector applied the gate.
    pub stage: GateStage,
    /// The gate applied.
    pub gate: Gate,
    /// What the gate computed from the series — a magnitude in the metric's own units, a
    /// p-value, a probability of superiority, or a point count, depending on the gate.
    pub value: Option<f64>,
    /// What [`value`](Self::value) was compared against.
    pub threshold: Option<f64>,
    /// Whether the candidate survived this gate.
    pub passed: bool,
}

/// Which detector a [`GateOutcome`] came from.
///
/// Several gates are shared — the absolute floor and the residual-scatter band apply in
/// every mode — and both history detectors run on the same series before arbitration
/// chooses between them, so the gate alone does not identify who asked. The stage does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateStage {
    /// The change-point detector (history mode).
    ChangePoint,
    /// The slow-drift detector (history mode).
    Drift,
    /// The branch-comparison detector.
    Branch,
}

impl GateStage {
    /// A stable identifier for display, matching the detector's name in the report.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ChangePoint => "change_point",
            Self::Drift => "drift",
            Self::Branch => "branch",
        }
    }
}

/// A single check a candidate must survive.
///
/// The variants name every condition on which a detector abandons a candidate, so a
/// [`GateLog`] can attribute a silence to a specific policy rather than to detection as a
/// whole. A gate that several detectors apply is one variant, distinguished at the
/// outcome by its [`GateStage`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Gate {
    /// The history series holds enough points to evaluate.
    MinSeriesPoints,
    /// The base-side window holds enough commit levels for a branch comparison.
    MinBaseCommits,
    /// A candidate split was located at all.
    SplitLocated,
    /// Both regimes either side of the split hold enough points to be persistent.
    MinRegime,
    /// The move is non-zero.
    NonZeroDelta,
    /// The move reaches the relative practical-magnitude floor.
    RelativeFloor,
    /// The move reaches the metric's absolute practical-magnitude floor.
    AbsoluteFloor,
    /// The move stands above the series' own between-commit residual scatter.
    ResidualNoise,
    /// The comparison sample carries enough scatter for a prediction interval to be
    /// formed at all.
    BaseScatter,
    /// The move is statistically significant: a Mann–Whitney, Mann–Kendall, or
    /// prediction-interval p-value against its configured alpha.
    Significance,
    /// The two regimes separate as populations rather than interleaving — the
    /// Mann–Whitney probability of superiority against its floor.
    RegimeSeparation,
    /// The two regimes' confidence intervals do not overlap, where the engine reports
    /// dispersion.
    IntervalDisjoint,
    /// The move exceeds the per-measurement noise band, where the engine reports
    /// dispersion.
    IntervalNoiseBand,
}

impl Gate {
    /// A stable identifier for display.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::MinSeriesPoints => "min_series_points",
            Self::MinBaseCommits => "min_base_commits",
            Self::SplitLocated => "split_located",
            Self::MinRegime => "min_regime",
            Self::NonZeroDelta => "non_zero_delta",
            Self::RelativeFloor => "relative_floor",
            Self::AbsoluteFloor => "absolute_floor",
            Self::ResidualNoise => "residual_noise",
            Self::BaseScatter => "base_scatter",
            Self::Significance => "significance",
            Self::RegimeSeparation => "regime_separation",
            Self::IntervalDisjoint => "interval_disjoint",
            Self::IntervalNoiseBand => "interval_noise_band",
        }
    }

    /// Every gate, for tests that must stay exhaustive as the enum grows.
    #[cfg(test)]
    pub(crate) const ALL: [Self; 13] = [
        Self::MinSeriesPoints,
        Self::MinBaseCommits,
        Self::SplitLocated,
        Self::MinRegime,
        Self::NonZeroDelta,
        Self::RelativeFloor,
        Self::AbsoluteFloor,
        Self::ResidualNoise,
        Self::BaseScatter,
        Self::Significance,
        Self::RegimeSeparation,
        Self::IntervalDisjoint,
        Self::IntervalNoiseBand,
    ];
}

/// A [`GateLog`] bound to the detector currently evaluating.
///
/// Every gate decision belongs to the stage that made it, and the shared gate helpers are
/// called from several stages, so binding the stage once where a detector starts keeps
/// each outcome correctly attributed without threading the stage through every helper.
///
/// Both recording methods return the verdict they were given, so a call site branches on
/// the same value it recorded and the two cannot drift apart:
/// `if !log.numeric(gate, value, threshold, value < threshold) { return None; }`.
#[derive(Debug)]
pub(crate) struct StageLog<'a> {
    log: &'a mut GateLog,
    stage: GateStage,
}

impl StageLog<'_> {
    /// Records a gate that compared `value` against `threshold`, yielding `passed`.
    pub(crate) fn numeric(&mut self, gate: Gate, value: f64, threshold: f64, passed: bool) -> bool {
        self.log.record(GateOutcome {
            stage: self.stage,
            gate,
            value: Some(value),
            threshold: Some(threshold),
            passed,
        });
        passed
    }

    /// Records a gate with no number to report, yielding `passed`.
    pub(crate) fn boolean(&mut self, gate: Gate, passed: bool) -> bool {
        self.log.record(GateOutcome {
            stage: self.stage,
            gate,
            value: None,
            threshold: None,
            passed,
        });
        passed
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn a_disabled_log_records_nothing() {
        let mut log = GateLog::disabled();
        log.stage(GateStage::Drift)
            .numeric(Gate::Significance, 0.9, 0.05, false);
        log.stage(GateStage::Drift)
            .boolean(Gate::SplitLocated, true);

        assert!(log.entries().is_empty());
        assert_eq!(log.declined_by(), None);
    }

    #[test]
    fn a_default_log_is_disabled() {
        let mut log = GateLog::default();
        log.stage(GateStage::ChangePoint)
            .boolean(Gate::SplitLocated, false);

        assert!(log.entries().is_empty());
    }

    #[test]
    fn a_recording_log_keeps_every_outcome_in_order() {
        let mut log = GateLog::recording();
        log.stage(GateStage::ChangePoint)
            .numeric(Gate::MinRegime, 5.0, 5.0, true);
        log.stage(GateStage::ChangePoint)
            .boolean(Gate::IntervalDisjoint, false);

        assert_eq!(
            log.entries(),
            [
                GateOutcome {
                    stage: GateStage::ChangePoint,
                    gate: Gate::MinRegime,
                    value: Some(5.0),
                    threshold: Some(5.0),
                    passed: true,
                },
                GateOutcome {
                    stage: GateStage::ChangePoint,
                    gate: Gate::IntervalDisjoint,
                    value: None,
                    threshold: None,
                    passed: false,
                },
            ]
        );
    }

    #[test]
    fn recording_yields_the_verdict_it_was_given() {
        let mut log = GateLog::recording();

        assert!(
            log.stage(GateStage::Branch)
                .numeric(Gate::RelativeFloor, 0.2, 0.05, true)
        );
        assert!(
            !log.stage(GateStage::Branch)
                .boolean(Gate::BaseScatter, false)
        );
    }

    #[test]
    fn declined_by_names_the_first_failing_gate() {
        let mut log = GateLog::recording();
        log.stage(GateStage::ChangePoint)
            .numeric(Gate::MinRegime, 5.0, 5.0, true);
        log.stage(GateStage::ChangePoint)
            .numeric(Gate::Significance, 0.4, 0.05, false);
        log.stage(GateStage::Drift)
            .numeric(Gate::RelativeFloor, 0.001, 0.02, false);

        assert_eq!(log.declined_by(), Some(Gate::Significance));
    }

    #[test]
    fn declined_by_stage_follows_one_detector() {
        let mut log = GateLog::recording();
        log.stage(GateStage::ChangePoint)
            .numeric(Gate::Significance, 0.4, 0.05, false);
        log.stage(GateStage::Drift)
            .numeric(Gate::RelativeFloor, 0.001, 0.02, false);

        assert_eq!(
            log.declined_by_stage(GateStage::Drift),
            Some(Gate::RelativeFloor)
        );
        assert_eq!(log.declined_by_stage(GateStage::Branch), None);
    }

    #[test]
    fn every_gate_and_stage_carries_a_distinct_label() {
        let gates: HashSet<&'static str> = Gate::ALL.iter().map(|gate| gate.label()).collect();
        assert_eq!(gates.len(), Gate::ALL.len());

        let stages = [
            GateStage::ChangePoint,
            GateStage::Drift,
            GateStage::Branch,
        ];
        let labels: HashSet<&'static str> = stages.iter().map(|stage| stage.label()).collect();
        assert_eq!(labels.len(), stages.len());
    }
}
