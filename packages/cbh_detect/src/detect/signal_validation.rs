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
//! Every case is run through a 2 × 2 matrix:
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
//!   base side leaves it quiet.
//! * **Absolute scale (dimension 2).** Every case is analysed both as-is and scaled up
//!   by a large constant. All of the analysis is relative, so the absolute scale must
//!   not change the verdict: the as-is verdict is checked against the case's
//!   expectation, and every scaled verdict is checked against that as-is reference, so
//!   a scale-sensitivity regression fails here.
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
//! The analysis treats every metric as noise-aware, so the curated series carry no
//! within-regime dispersion (each regime is a run of identical values) and every step
//! is large and well above the practical-magnitude floors. That keeps the verdict
//! unambiguous under the noisy gates: a step between two zero-variance regimes is
//! maximally significant, so detection turns purely on the mode's slice and floor
//! rather than on any noise model.

#![cfg_attr(coverage_nightly, coverage(off))]

use std::sync::Arc;

use cbh_model::{BenchmarkId, DiscriminantSet, MetricKind};
use nonempty::nonempty;

use crate::detect::findings::find_changes;
use crate::detect::{AnalysisConfig, AnalysisContext, AnalysisMode, Series, SeriesPoint};

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

    /// The analysis context this mode is evaluated under. `merge_base_index` is consulted
    /// only by branch mode; history ignores it.
    ///
    /// `include_improvements` is set from [`reports_improvements`](Self::reports_improvements)
    /// so the context matches the mode's intended reporting semantics: branch (which
    /// reports both directions) opts in, history opts out. Branch mode ignores the
    /// flag today, but pinning it consistently keeps the context correct if that changes.
    fn context(self, merge_base_index: Option<usize>) -> AnalysisContext {
        let mode = match self {
            Self::History => AnalysisMode::History,
            Self::Branch => AnalysisMode::Branch,
        };
        AnalysisContext {
            mode,
            config: AnalysisConfig::default(),
            merge_base_index,
            include_improvements: self.reports_improvements(),
            include_inactive: false,
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

/// One curated series — its base and branch sides — and the outcome each mode is
/// expected to see in it.
struct SignalCase {
    /// Human-readable case name, surfaced in assertion failures.
    name: &'static str,
    /// The base-side (unscaled) series values, oldest-first: the commits at or before
    /// the merge-base. May be empty, which leaves branch mode without a base side to
    /// compare against, so it stays quiet. The base/branch split matters only to branch
    /// mode; history sees the whole concatenated series and reads these values as
    /// ordinary leading points, indifferent to which side they came from.
    base: Vec<f64>,
    /// The branch-side (unscaled) series values, oldest-first: the commits past the
    /// merge-base. May be empty.
    branch: Vec<f64>,
    /// The outcome history mode's change-point detector is expected to see.
    expected_history: Outcome,
    /// The outcome branch mode is expected to see.
    expected_branch: Outcome,
}

impl SignalCase {
    /// The whole series, the base side followed by the branch side, oldest-first.
    fn values(&self) -> Vec<f64> {
        [self.base.as_slice(), self.branch.as_slice()].concat()
    }

    /// The first-parent merge-base split index handed to branch mode: the last base-side
    /// point, or `None` when there is no base side (branch mode then has nothing to
    /// compare against and stays quiet). History ignores it.
    fn merge_base_index(&self) -> Option<usize> {
        self.base.len().checked_sub(1)
    }

    /// The outcome `mode` is expected to see in this case.
    fn expected_outcome(&self, mode: Mode) -> Outcome {
        match mode {
            Mode::History => self.expected_history,
            Mode::Branch => self.expected_branch,
        }
    }
}

/// `count` copies of `value`, as a run of series points.
fn run_of(value: f64, count: usize) -> Vec<f64> {
    vec![value; count]
}

/// The hand-curated cases. New "obvious answer" series are added as one row each.
fn cases() -> Vec<SignalCase> {
    vec![
        // An unmistakable sustained doubling halfway through. History and branch (split
        // at the step) both see a rise.
        SignalCase {
            name: "doubling_step",
            base: run_of(100.0, 50),
            branch: run_of(200.0, 50),
            expected_history: Outcome::Rise,
            expected_branch: Outcome::Rise,
        },
        // The mirror image: a sustained halving. Same mode geometry, opposite direction,
        // so it exercises the improvement-reporting path (surfaced only by branch mode).
        SignalCase {
            name: "halving_step",
            base: run_of(200.0, 50),
            branch: run_of(100.0, 50),
            expected_history: Outcome::Fall,
            expected_branch: Outcome::Fall,
        },
        // A jump confined to the final commit. Branch (split just before the jump) sees
        // the rise; history does not, since one trailing point is not a sustained trend.
        SignalCase {
            name: "tip_spike",
            base: run_of(100.0, 99),
            branch: run_of(200.0, 1),
            expected_history: Outcome::Quiet,
            expected_branch: Outcome::Rise,
        },
        // The mirror image at the tip: the final commit drops.
        SignalCase {
            name: "tip_drop",
            base: run_of(200.0, 99),
            branch: run_of(100.0, 1),
            expected_history: Outcome::Quiet,
            expected_branch: Outcome::Fall,
        },
        // A dead-flat line: nothing moved, so no mode should ever flag it.
        SignalCase {
            name: "flat_line",
            base: run_of(100.0, 50),
            branch: run_of(100.0, 50),
            expected_history: Outcome::Quiet,
            expected_branch: Outcome::Quiet,
        },
        // The same obvious doubling as the first case, but with no base side. Branch
        // mode has nothing to compare the branch against, so it must stay quiet even
        // though history still sees the rise over the whole series.
        SignalCase {
            name: "doubling_without_base",
            base: Vec::new(),
            branch: [run_of(100.0, 50), run_of(200.0, 50)].concat(),
            expected_history: Outcome::Rise,
            expected_branch: Outcome::Quiet,
        },
    ]
}

/// Builds a noise-free series carrying `values` in topological order, tagged with
/// `kind`.
///
/// The points carry no confidence intervals and each curated regime is a run of
/// identical values, so the series has zero within-regime dispersion. The analysis is
/// noise-aware for every metric, but a step between two zero-variance regimes is
/// unambiguous under those gates, so the verdict turns on the mode and the step
/// magnitude rather than on a noise model.
fn noise_free_series(values: &[f64], kind: MetricKind) -> Series {
    let points = values
        .iter()
        .enumerate()
        .map(|(index, &value)| SeriesPoint {
            topo_index: index,
            dirty: false,
            object_ordinal: u32::try_from(index).unwrap(),
            commit: Some(Arc::from(format!("commit{index}"))),
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
        id: BenchmarkId::new(nonempty!["signal".to_owned(), "case".to_owned()]),
        kind,
        points,
        active_start: 0,
        blessing: None,
    }
}

/// Runs the serial detection oracle on a single series under `context` and reports
/// whether it raised any finding.
fn raises_finding(values: &[f64], kind: MetricKind, context: &AnalysisContext) -> bool {
    let series = noise_free_series(values, kind);
    let findings = find_changes(&[series], context);
    !findings.is_empty()
}

/// `values`, each multiplied by `scale`.
fn scaled(values: &[f64], scale: f64) -> Vec<f64> {
    values.iter().map(|&value| value * scale).collect()
}

#[test]
fn curated_signals_match_expected_verdicts() {
    // Scale multiples applied on top of each as-is series; the as-is verdict is the
    // reference every scaled verdict must match. The analysis is relative, so no
    // multiple may change the outcome.
    let scale_multiples = [1000.0_f64];

    for case in cases() {
        let values = case.values();
        for mode in Mode::ALL {
            let context = mode.context(case.merge_base_index());
            let expected = case.expected_outcome(mode).is_finding(mode);
            // Every metric is lower-is-better; a curated fall only surfaces where the
            // mode reports improvements. Instruction count is a representative kind.
            let kind = MetricKind::InstructionCount;

            // Dimension 1: the as-is verdict under this mode matches the hand-picked
            // expectation.
            let reference = raises_finding(&values, kind, &context);
            assert_eq!(
                reference, expected,
                "case '{}' mode={mode:?}: expected finding={expected}, got {reference}",
                case.name,
            );

            // Dimension 2: scaling the whole series by any constant leaves the verdict
            // unchanged, because every comparison the analysis makes is relative.
            for scale in scale_multiples {
                let scaled_verdict = raises_finding(&scaled(&values, scale), kind, &context);
                assert_eq!(
                    scaled_verdict, reference,
                    "case '{}' mode={mode:?}: scaling by {scale} changed the verdict \
                     (absolute scale must not matter)",
                    case.name,
                );
            }
        }
    }
}
