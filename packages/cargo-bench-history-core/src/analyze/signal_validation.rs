//! Signal-validation suite: hand-curated "obvious right answer" series that guard
//! against the analysis statistics yielding illogical results.
//!
//! Each case is a data series with an unambiguous shape (an obvious step, a dead-flat
//! line) paired with the move each analysis mode's detector is expected to see. The
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
//! Every case is run through a 3 × 2 × 2 matrix:
//!
//! * **Analysis mode (dimension 1).** The three modes are *different detectors*, not
//!   one detector with a flag: [`History`](AnalysisMode::History) locates a change-point
//!   over the whole series, [`Branch`](AnalysisMode::Branch) compares the branch's
//!   latest regime against the base level across a merge-base split, and
//!   [`Tip`](AnalysisMode::Tip) compares only the newest point against its recent
//!   window. Because each inspects a different slice, the *same* series yields different
//!   verdicts per mode, so mode is a curated dimension: every case states the move each
//!   mode is expected to see. An obvious mid-series step is a rise to history and branch
//!   but invisible to tip (whose window has already caught up); a lone final-point jump
//!   is a rise to tip and branch but not a sustained historical trend. Branch mode also
//!   needs a merge-base to have a base side at all — a case without one leaves it quiet.
//! * **Polarity (dimension 2).** The codebase's only polarity lever is the metric kind,
//!   so *higher-is-worse* is modelled with a lower-is-better metric
//!   ([`MetricKind::InstructionCount`]) and *lower-is-worse* with the one
//!   higher-is-better metric ([`MetricKind::L1CacheHits`]). History and tip report the
//!   worse direction only, so a move surfaces as a finding under one polarity and is a
//!   suppressed improvement under the other; branch reports *both* directions, so a move
//!   is a finding under either polarity (only its classification differs). The expected
//!   verdict is derived from the per-mode move via this reporting contract.
//! * **Absolute scale (dimension 3).** Every case is analysed both as-is and scaled up
//!   by a large constant. All of the analysis is relative, so the absolute scale must
//!   not change the verdict: the as-is verdict is checked against the case's
//!   expectation, and every scaled verdict is checked against that as-is reference, so
//!   a scale-sensitivity regression fails here.
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

use nonempty::nonempty;

use crate::analyze::findings::find_changes;
use crate::analyze::{AnalysisConfig, AnalysisContext, AnalysisMode, Series, SeriesPoint};
use crate::model::{BenchmarkId, DiscriminantSet, MetricKind};

/// How a rise in the measured metric is judged.
///
/// This is the suite's dimension-2 lever. Both variants map to a metric kind that
/// differs only in polarity — which direction of change counts as "worse" — so a case's
/// detection is identical under both and only the reported direction changes.
#[derive(Clone, Copy, Debug)]
enum Polarity {
    /// A rise is a regression (lower-is-better metric).
    HigherIsWorse,
    /// A rise is an improvement (higher-is-better metric).
    LowerIsWorse,
}

impl Polarity {
    /// The two polarities, for matrix expansion.
    const ALL: [Self; 2] = [Self::HigherIsWorse, Self::LowerIsWorse];

    /// The metric kind that realises this polarity.
    fn metric_kind(self) -> MetricKind {
        match self {
            // Lower-is-better: a rise is a regression.
            Self::HigherIsWorse => MetricKind::InstructionCount,
            // The sole higher-is-better metric: a rise is an improvement.
            Self::LowerIsWorse => MetricKind::L1CacheHits,
        }
    }
}

/// The analysis mode a case is evaluated under — the suite's dimension-1 lever.
///
/// The three modes are genuinely different detectors, so a case declares its expected
/// move per mode rather than sharing one verdict across them.
#[derive(Clone, Copy, Debug)]
enum Mode {
    /// Change-point analysis over the whole series.
    History,
    /// The branch's latest regime against the base level, across a merge-base split.
    Branch,
    /// The newest point against its recent window.
    Tip,
}

impl Mode {
    /// The three modes, for matrix expansion.
    const ALL: [Self; 3] = [Self::History, Self::Branch, Self::Tip];

    /// Whether this mode reports improvements as findings. Only branch does: history is
    /// run here as a regressions-only drift watch (`include_improvements = false`) and
    /// tip is a regression guard, so for those an improvement is a non-finding.
    fn reports_improvements(self) -> bool {
        matches!(self, Self::Branch)
    }

    /// The analysis context this mode is evaluated under. `merge_base_index` is consulted
    /// only by branch mode; the other modes ignore it.
    fn context(self, merge_base_index: Option<usize>) -> AnalysisContext {
        let mode = match self {
            Self::History => AnalysisMode::History,
            Self::Branch => AnalysisMode::Branch,
            Self::Tip => AnalysisMode::Tip,
        };
        AnalysisContext {
            mode,
            config: AnalysisConfig::default(),
            merge_base_index,
            include_improvements: false,
            include_inactive: false,
        }
    }
}

/// The move a mode's detector is expected to see in a case — the hand-curated,
/// polarity-independent judgment about the raw series shape.
///
/// Combined with a [`Polarity`] and the mode's reporting contract this yields the
/// expected finding verdict: the raw rise/fall is classified as a regression or an
/// improvement by the metric's polarity, and reported only when the mode surfaces that
/// direction.
#[derive(Clone, Copy, Debug)]
enum Move {
    /// The values step up.
    Rise,
    /// The values step down.
    Fall,
    /// Nothing notable moves.
    Quiet,
}

impl Move {
    /// Whether this move surfaces as a finding under `polarity` in `mode`.
    fn is_finding(self, polarity: Polarity, mode: Mode) -> bool {
        match (self, polarity) {
            (Self::Quiet, _) => false,
            // Classified a regression (worse) — every mode reports it.
            (Self::Rise, Polarity::HigherIsWorse) | (Self::Fall, Polarity::LowerIsWorse) => true,
            // Classified an improvement (better) — reported only where the mode reports
            // both directions.
            (Self::Rise, Polarity::LowerIsWorse) | (Self::Fall, Polarity::HigherIsWorse) => {
                mode.reports_improvements()
            }
        }
    }
}

/// One curated series and the move each mode is expected to see in it.
struct SignalCase {
    /// Human-readable case name, surfaced in assertion failures.
    name: &'static str,
    /// The base (unscaled) series values, oldest-first.
    values: Vec<f64>,
    /// First-parent split index handed to branch mode; `None` leaves branch mode without
    /// a base side, so it stays quiet. Ignored by history and tip.
    merge_base_index: Option<usize>,
    /// The move history mode's change-point detector is expected to see.
    history: Move,
    /// The move branch mode is expected to see (given `merge_base_index`).
    branch: Move,
    /// The move tip mode is expected to see.
    tip: Move,
}

impl SignalCase {
    /// The move `mode` is expected to see in this case.
    fn expected_move(&self, mode: Mode) -> Move {
        match mode {
            Mode::History => self.history,
            Mode::Branch => self.branch,
            Mode::Tip => self.tip,
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
        // at the step) both see a rise; tip is quiet because its recent window already
        // sits at the new, higher level.
        SignalCase {
            name: "doubling_step",
            values: [run_of(100.0, 50), run_of(200.0, 50)].concat(),
            merge_base_index: Some(49),
            history: Move::Rise,
            branch: Move::Rise,
            tip: Move::Quiet,
        },
        // The mirror image: a sustained halving. Same mode geometry, opposite direction,
        // so it exercises the other polarity's finding path.
        SignalCase {
            name: "halving_step",
            values: [run_of(200.0, 50), run_of(100.0, 50)].concat(),
            merge_base_index: Some(49),
            history: Move::Fall,
            branch: Move::Fall,
            tip: Move::Quiet,
        },
        // A jump confined to the final commit. Tip and branch (split just before the
        // jump) see the rise; history does not, since one trailing point is not a
        // sustained trend.
        SignalCase {
            name: "tip_spike",
            values: [run_of(100.0, 99), run_of(200.0, 1)].concat(),
            merge_base_index: Some(98),
            history: Move::Quiet,
            branch: Move::Rise,
            tip: Move::Rise,
        },
        // The mirror image at the tip: the final commit drops.
        SignalCase {
            name: "tip_drop",
            values: [run_of(200.0, 99), run_of(100.0, 1)].concat(),
            merge_base_index: Some(98),
            history: Move::Quiet,
            branch: Move::Fall,
            tip: Move::Fall,
        },
        // A dead-flat line: nothing moved, so no mode and no polarity should ever flag it.
        SignalCase {
            name: "flat_line",
            values: run_of(100.0, 100),
            merge_base_index: Some(49),
            history: Move::Quiet,
            branch: Move::Quiet,
            tip: Move::Quiet,
        },
        // The same obvious doubling as the first case, but with no merge-base. Branch
        // mode has no base side to compare against, so it must stay quiet even though
        // history still sees the rise.
        SignalCase {
            name: "doubling_without_base",
            values: [run_of(100.0, 50), run_of(200.0, 50)].concat(),
            merge_base_index: None,
            history: Move::Rise,
            branch: Move::Quiet,
            tip: Move::Quiet,
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

/// The values of `case`, multiplied by `scale`.
fn scaled(case: &SignalCase, scale: f64) -> Vec<f64> {
    case.values.iter().map(|&value| value * scale).collect()
}

#[test]
fn curated_signals_match_expected_verdicts() {
    // Scale multiples applied on top of each as-is series; the as-is verdict is the
    // reference every scaled verdict must match. The analysis is relative, so no
    // multiple may change the outcome.
    let scale_multiples = [1000.0_f64];

    for case in cases() {
        for mode in Mode::ALL {
            let context = mode.context(case.merge_base_index);
            for polarity in Polarity::ALL {
                let expected = case.expected_move(mode).is_finding(polarity, mode);
                let kind = polarity.metric_kind();

                // Dimensions 1 & 2: the as-is verdict under this mode and polarity
                // matches the hand-picked expectation.
                let reference = raises_finding(&case.values, kind, &context);
                assert_eq!(
                    reference, expected,
                    "case '{}' mode={mode:?} polarity={polarity:?}: \
                     expected finding={expected}, got {reference}",
                    case.name,
                );

                // Dimension 3: scaling the whole series by any constant leaves the
                // verdict unchanged, because every comparison the analysis makes is
                // relative.
                for scale in scale_multiples {
                    let scaled_verdict = raises_finding(&scaled(&case, scale), kind, &context);
                    assert_eq!(
                        scaled_verdict, reference,
                        "case '{}' mode={mode:?} polarity={polarity:?}: scaling by {scale} \
                         changed the verdict (absolute scale must not matter)",
                        case.name,
                    );
                }
            }
        }
    }
}
