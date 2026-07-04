//! Signal-validation suite: hand-curated "obvious right answer" series that guard
//! against the analysis statistics yielding illogical results.
//!
//! Each case is a data series with an unambiguous shape (an obvious step, a dead-flat
//! line) paired with, per polarity, whether the analysis *should* report a finding.
//! The point is not to exercise a particular detector but to pin the end-to-end
//! verdict of the analysis on inputs a human would answer without hesitation — so a
//! future change to the math that starts calling a doubling "no change", or a flat
//! line "a regression", fails here loudly.
//!
//! The verdict is taken through the serial detection oracle [`find_changes`], the same
//! spawner-free entry the rest of the [`findings`](super::findings) unit tests use;
//! `find_changes_spawned_matches_the_serial_pass` proves it produces exactly the
//! findings the spawner-distributed production path
//! ([`find_changes_spawned`](super::find_changes_spawned)) does.
//!
//! Every case is run through a 2×2 matrix:
//!
//! * **Polarity (dimension 1).** The codebase's only polarity lever is the metric
//!   kind, so *higher-is-worse* is modelled with a lower-is-better metric
//!   ([`MetricKind::InstructionCount`]: a rise is a regression) and *lower-is-worse*
//!   with the one higher-is-better metric ([`MetricKind::L1CacheHits`]: a rise is an
//!   improvement). The analysis is asked to report the *worse* direction only
//!   (`include_improvements = false`), which is what makes this dimension meaningful:
//!   an obvious doubling is a finding under higher-is-worse but a (suppressed)
//!   improvement under lower-is-worse. The expected verdict therefore depends on the
//!   series and is stated case by case.
//! * **Absolute scale (dimension 2).** Every case is analysed both as-is and scaled up
//!   by a large constant. All of the analysis is relative, so the absolute scale must
//!   not change the verdict: the as-is verdict is checked against the case's
//!   expectation, and every scaled verdict is checked against that as-is reference, so
//!   a scale-sensitivity regression fails here.
//!
//! The check itself is deliberately coarse — "did the analysis report any finding?" —
//! because these inputs are chosen so the *presence* of a finding is the whole
//! question. Detector internals, confidence, and magnitude are covered by the
//! finer-grained unit tests in [`findings`](super::findings).

#![cfg_attr(coverage_nightly, coverage(off))]

use std::sync::Arc;

use nonempty::nonempty;

use crate::analyze::findings::find_changes;
use crate::analyze::{AnalysisConfig, AnalysisContext, AnalysisMode, Series, SeriesPoint};
use crate::model::{BenchmarkId, DiscriminantSet, MetricKind};

/// How a rise in the measured metric is judged.
///
/// This is the suite's dimension-1 lever. Both variants map to a *deterministic*
/// (noise-free) metric kind, so detection is exact and the two differ only in which
/// direction of change counts as "worse".
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

/// One curated series and its expected verdict under each polarity.
struct SignalCase {
    /// Human-readable case name, surfaced in assertion failures.
    name: &'static str,
    /// The base (unscaled) series values, oldest-first.
    values: Vec<f64>,
    /// Whether a finding is expected under [`Polarity::HigherIsWorse`].
    expect_higher_is_worse: bool,
    /// Whether a finding is expected under [`Polarity::LowerIsWorse`].
    expect_lower_is_worse: bool,
}

impl SignalCase {
    /// The expected verdict for `polarity`.
    fn expected(&self, polarity: Polarity) -> bool {
        match polarity {
            Polarity::HigherIsWorse => self.expect_higher_is_worse,
            Polarity::LowerIsWorse => self.expect_lower_is_worse,
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
        // An unmistakable doubling of the metric halfway through: a regression when a
        // rise is worse, and merely an (unreported) improvement when a rise is better.
        SignalCase {
            name: "doubling_step",
            values: [run_of(100.0, 50), run_of(200.0, 50)].concat(),
            expect_higher_is_worse: true,
            expect_lower_is_worse: false,
        },
        // A dead-flat line: nothing moved, so no polarity should ever flag it.
        SignalCase {
            name: "flat_line",
            values: run_of(100.0, 100),
            expect_higher_is_worse: false,
            expect_lower_is_worse: false,
        },
    ]
}

/// Builds a deterministic (noise-free) series carrying `values` in topological order,
/// tagged with `kind`.
fn deterministic_series(values: &[f64], kind: MetricKind) -> Series {
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

/// The history-mode context the suite analyses under: the worse direction only, so an
/// improvement is a non-finding rather than a flipped finding.
fn history_context() -> AnalysisContext {
    AnalysisContext {
        mode: AnalysisMode::History,
        config: AnalysisConfig::default(),
        merge_base_index: None,
        include_improvements: false,
        include_inactive: false,
    }
}

/// Runs the serial detection oracle on a single series and reports whether it raised
/// any finding.
fn raises_finding(values: &[f64], kind: MetricKind) -> bool {
    let series = deterministic_series(values, kind);
    let findings = find_changes(&[series], &history_context());
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
        for polarity in Polarity::ALL {
            let expected = case.expected(polarity);
            let kind = polarity.metric_kind();

            // Dimension 1: the as-is verdict matches the hand-picked expectation.
            let reference = raises_finding(&case.values, kind);
            assert_eq!(
                reference, expected,
                "case '{}' under {polarity:?}: expected finding={expected}, got {reference}",
                case.name,
            );

            // Dimension 2: scaling the whole series by any constant leaves the verdict
            // unchanged, because every comparison the analysis makes is relative.
            for scale in scale_multiples {
                let scaled_verdict = raises_finding(&scaled(&case, scale), kind);
                assert_eq!(
                    scaled_verdict, reference,
                    "case '{}' under {polarity:?}: scaling by {scale} changed the verdict \
                     (absolute scale must not matter)",
                    case.name,
                );
            }
        }
    }
}
