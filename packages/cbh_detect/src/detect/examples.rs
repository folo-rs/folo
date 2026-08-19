//! Named example series, shared by the tests and by the documentation figures.
//!
//! Detection policy is easier to state than to demonstrate, so both the test suite and
//! the generated documentation work from the same small catalogue of series whose right
//! answer a human reads off the shape without hesitation. Each series here is defined
//! once and consumed by both, which is what keeps a documented verdict and an asserted
//! verdict from drifting apart.
//!
//! The values are metric-agnostic bare levels: [`series`] builds them into a [`Series`]
//! of whichever kind and topology a caller needs. The verdict each series' documentation
//! claims is the one history mode reaches under the fixed detection policy.
//!
//! Reproducibility is the whole point, so every series is a pure function of its own
//! name: the scatter is drawn from [`seed_of`] applied to that name and is identical on
//! every platform and every run.
//!
//! [`Series`]: crate::Series

#![cfg_attr(coverage_nightly, coverage(off))]

use std::sync::Arc;
use std::{iter, slice};

use cbh_model::{BenchmarkId, DiscriminantSet, Engine, MetricKind};
use nonempty::nonempty;

use crate::detect::findings::count_to_f64;
use crate::detect::noise_gates::{COMPARE_WINDOW, MIN_REGIME};
pub use crate::detect::recorded::{
    CONTENDED_RUNNER_BASE, CONTENDED_RUNNER_EXCURSION, CONTENDED_RUNNER_LEVEL,
    CONTENDED_RUNNER_LEVEL_START, STATIONARY_BIMODAL_BASE, STATIONARY_BIMODAL_HIGH,
    STATIONARY_BIMODAL_NOISE,
};
pub use crate::detect::scatter::{TIMING_NOISE_CV, scattered, seed_of};
use crate::detect::{
    AnalysisContext, AnalysisMode, Series, SeriesPoint, attach_base_windows,
};

/// The level every example series starts from.
///
/// A round number, so a reader can convert a value to a percentage of baseline in their
/// head while reading a figure.
const BASELINE: f64 = 100.0;

/// The level a stepped example moves to.
///
/// Far enough above [`BASELINE`] to clear the relative practical-magnitude floor several
/// times over and to stand well outside the scatter [`TIMING_NOISE_CV`] produces, so an
/// example that is meant to report is never a marginal call.
const ELEVATED: f64 = 130.0;

/// How many points each regime of a stepped example holds.
///
/// Twice the persistence floor, so both regimes are comfortably above it and an example
/// that fails to report never fails on regime length.
const REGIME: usize = 2 * MIN_REGIME;

/// The fraction of baseline `slow_ramp` gains at each successive point.
///
/// Small enough that no single step between adjacent points is a level shift, while the
/// movement accumulated across the whole series is several times the relative
/// practical-magnitude floor — which is what makes the series a drift rather than a step.
const RAMP_PER_POINT: f64 = 0.003;

/// How many points `slow_ramp` climbs across.
///
/// Long enough that a straight line fits the whole history better than any single split
/// can: a trend spread over few commits is nearly as well explained by a step somewhere
/// in the middle of it, and arbitration then keeps the step. Gradualness across many
/// commits is what distinguishes a drift, so the example has to be long enough to show
/// it.
const RAMP_LENGTH: usize = 3 * REGIME;

/// A step from one settled level to another: the textbook change point.
///
/// History mode reports it as a regression by the change-point method. Both regimes carry
/// light measurement scatter, so the step is a realistic one rather than two constants.
#[must_use]
pub fn clean_step() -> Vec<f64> {
    let levels: Vec<f64> = iter::repeat_n(BASELINE, REGIME)
        .chain(iter::repeat_n(ELEVATED, REGIME))
        .collect();
    scattered(&levels, TIMING_NOISE_CV, seed_of("clean_step"))
}

/// A steady climb with no step anywhere in it.
///
/// History mode reports it as a regression by the drift method. Both detectors fire —
/// the trend is significant and a split can always be proposed — and arbitration keeps
/// whichever model leaves the smaller residual. A line wins here because the movement is
/// spread evenly across every commit: no single split separates the history into two
/// settled levels, so a step model has to absorb the climb within each of its regimes.
#[must_use]
pub fn slow_ramp() -> Vec<f64> {
    let levels: Vec<f64> = (0..RAMP_LENGTH)
        .map(|index| BASELINE * RAMP_PER_POINT.mul_add(count_to_f64(index), 1.0))
        .collect();
    scattered(&levels, TIMING_NOISE_CV, seed_of("slow_ramp"))
}

/// A flat series with one late excursion: a real move that never settles.
///
/// History mode stays quiet. The excursion is genuine and large, but too few points
/// follow it to form a regime, so the persistence gate declines it — which is the
/// distinction between a level shift and a one-off measurement.
#[must_use]
pub fn blip() -> Vec<f64> {
    let mut levels = vec![BASELINE; 2 * REGIME];
    // The excursion sits close enough to the end that fewer than `min_regime` points
    // follow it, which is precisely what makes it non-persistent.
    if let Some(last) = levels.len().checked_sub(2).and_then(|i| levels.get_mut(i)) {
        *last = ELEVATED;
    }
    levels
}

/// A stationary series carrying realistic measurement scatter.
///
/// History mode stays quiet. This is the case the noise gates exist for: the scatter
/// alone offers any split a change-point search proposes some apparent difference, and
/// the series must nevertheless report nothing.
#[must_use]
pub fn flat_noisy() -> Vec<f64> {
    let levels = vec![BASELINE; 3 * REGIME];
    scattered(&levels, TIMING_NOISE_CV, seed_of("flat_noisy"))
}

/// Builds a series called `name` carrying `values` at consecutive topological indices
/// starting at `topo_start`, tagged with `kind`.
///
/// The points carry no explicit confidence intervals: the engines these examples model
/// report a single figure per run, so the variation the analysis judges is the
/// between-commit scatter the values already carry. An example that must exercise a gate
/// reading engine-reported dispersion adds it with [`with_intervals`].
///
/// Every series is attributed to one fixed discriminant set, because detection never
/// consults the discriminant and a batch of examples is only comparable while they share
/// one.
#[must_use]
pub fn series(name: &str, values: &[f64], kind: MetricKind, topo_start: usize) -> Series {
    let points = values
        .iter()
        .enumerate()
        .map(|(offset, &value)| {
            let topo_index = topo_start
                .checked_add(offset)
                .expect("an example series is far shorter than the topological index space");
            SeriesPoint {
                topo_index,
                dirty: false,
                object_ordinal: u32::try_from(topo_index)
                    .expect("an example series holds far fewer points than `u32::MAX`"),
                commit: Some(Arc::from(format!("commit{topo_index}"))),
                value,
                interval_low: None,
                interval_high: None,
            }
        })
        .collect();
    Series {
        set: DiscriminantSet {
            engine: Engine::Callgrind,
            target_triple: "t".into(),
            machine_key: "m1".into(),
        },
        id: BenchmarkId::new(nonempty![name.to_owned(), "case".to_owned()]),
        kind,
        points,
        base_window: Vec::new(),
        active_start: 0,
        blessing: None,
    }
}

/// `series` with a confidence interval of `half_width` either side of every point's own
/// value.
///
/// The half-width is chosen by the caller and is not a measurement: no engine reports it
/// and nothing in the values implies it. Choosing it is how an example decides what the
/// gates that read dispersion see — whether two regimes' intervals separate, and whether
/// a trend clears the per-measurement noise band — which is the only way to demonstrate
/// those gates at all.
#[must_use]
pub fn with_intervals(mut series: Series, half_width: f64) -> Series {
    for point in &mut series.points {
        point.interval_low = Some(point.value - half_width);
        point.interval_high = Some(point.value + half_width);
    }
    series
}

/// `series` with a branch-mode base-ref window attached from its clean observations
/// at or before `base_ref_index`.
#[must_use]
pub fn with_base_window(mut series: Series, base_ref_index: usize) -> Series {
    let mut base = series.clone();
    base.points
        .retain(|point| !point.dirty && point.topo_index <= base_ref_index);
    attach_base_windows(
        slice::from_mut(&mut series),
        slice::from_ref(&base),
        COMPARE_WINDOW,
    );
    series
}

/// A history-mode context over `series` under the default configuration: what a
/// scheduled analysis of one benchmark's stored history runs.
///
/// History mode reports regressions only, so an example that moves downwards is a
/// non-finding here; use [`branch_context`] for an example that must be visible in
/// either direction.
#[must_use]
pub fn history_context(series: &Series) -> AnalysisContext {
    AnalysisContext {
        mode: AnalysisMode::History,
        merge_base_index: None,
        base_ref_index: None,
        tip_index: tip_index_of(series),
    }
}

/// A branch-mode context over `series` under the default configuration.
///
/// Branch mode judges the latest state against the base window attached to
/// `series`, so it reports both directions.
#[must_use]
pub fn branch_context(series: &Series, merge_base_index: usize) -> AnalysisContext {
    AnalysisContext {
        mode: AnalysisMode::Branch,
        merge_base_index: Some(merge_base_index),
        base_ref_index: Some(merge_base_index),
        tip_index: tip_index_of(series),
    }
}

/// The furthest topological index `series` reaches, which is the tip an analysis of it
/// is run against.
fn tip_index_of(series: &Series) -> usize {
    series
        .points
        .iter()
        .map(|point| point.topo_index)
        .max()
        .unwrap_or(0)
}
