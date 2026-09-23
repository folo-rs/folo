//! Benchmarks change-point significance scoring and selection adjustment.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::num::NonZero;

use cbh_stats::{MannWhitneyU, SelectionCalibration, selection_adjusted_change_point};
use criterion::{Criterion, criterion_group, criterion_main};

criterion_group!(benches, exact_lopsided, selection_adjustment);
criterion_main!(benches);

/// A short history that still exercises lopsided tails and the analytic split scan.
const LOW_SERIES_LEN: usize = 100;
/// The production persistence floor, which is the smallest reportable side.
const LOW_LEFT_LEN: usize = 5;
/// Extends the rank tables and analytic scan without a production-cap workload.
const HIGH_SERIES_LEN: usize = 200;
/// Increases the exact subset size as well as the total history length.
const HIGH_LEFT_LEN: usize = 6;
/// Production minimum calibration budget for a small analysis family.
const PERMUTATION_ORDER_BUDGET: usize = 259_200;
/// Production weight of the analytic calibration component.
const ANALYTIC_WEIGHT: f64 = 0.10;
/// Rank-1 acceptance level for a family containing one judged series.
const ACCEPTANCE_LEVEL: f64 = 0.025;
/// Rank-1 pre-arbitration level for the seven-series crowded fixture.
const CROWDED_ACCEPTANCE_LEVEL: f64 = 0.1 / 14.0;
/// Production maximum reached by an unresolved large-family candidate.
const MAX_PERMUTATION_ORDER: usize = 500_000;
/// Rank-1 pre-arbitration level at the stress harness's large-family scale.
const LARGE_FAMILY_ACCEPTANCE_LEVEL: f64 = 0.000_002_5;
/// Pre-arbitration rejection boundary used by the history detector.
const REJECTION_LEVEL: f64 = 0.025;
/// Separates noisy regimes enough to require a completed exact subgroup.
const CLEAR_STEP_OFFSET: f64 = 50.0;
/// Overlaps the regimes so permutation calibration can reject before exhaustion.
const OVERLAPPING_STEP_OFFSET: f64 = 2.0;

fn exact_lopsided(c: &mut Criterion) {
    let mut group = c.benchmark_group("cbh_stats_significance/exact_lopsided");

    let low = separated_samples(LOW_LEFT_LEN, LOW_SERIES_LEN);
    let low_name = format!("{LOW_LEFT_LEN}-vs-{}", LOW_SERIES_LEN - LOW_LEFT_LEN);
    group.bench_function(low_name, |b| {
        b.iter(|| black_box(MannWhitneyU::new(black_box(&low.0), black_box(&low.1))));
    });

    let high = separated_samples(HIGH_LEFT_LEN, HIGH_SERIES_LEN);
    let high_name = format!("{HIGH_LEFT_LEN}-vs-{}", HIGH_SERIES_LEN - HIGH_LEFT_LEN);
    group.bench_function(high_name, |b| {
        b.iter(|| black_box(MannWhitneyU::new(black_box(&high.0), black_box(&high.1))));
    });

    group.finish();
}

fn selection_adjustment(c: &mut Criterion) {
    let mut group = c.benchmark_group("cbh_stats_significance/selection_adjustment");
    let budget = NonZero::new(PERMUTATION_ORDER_BUDGET).expect("the production budget is nonzero");
    let calibration = SelectionCalibration {
        permutation_order_budget: budget,
        analytic_weight: ANALYTIC_WEIGHT,
        accept_analytic_below: ACCEPTANCE_LEVEL,
        reject_at_or_above: REJECTION_LEVEL,
    };

    let low = clean_step(LOW_SERIES_LEN);
    group.bench_function(format!("{LOW_SERIES_LEN}-points"), |b| {
        b.iter(|| {
            black_box(selection_adjusted_change_point(
                black_box(&low),
                LOW_LEFT_LEN,
                calibration,
            ))
        });
    });

    let short = noisy_short_step(CLEAR_STEP_OFFSET);
    let short_calibration = SelectionCalibration {
        accept_analytic_below: CROWDED_ACCEPTANCE_LEVEL,
        ..calibration
    };
    group.bench_function("12-points/exact-subgroup", |b| {
        b.iter(|| {
            black_box(selection_adjusted_change_point(
                black_box(&short),
                LOW_LEFT_LEN,
                short_calibration,
            ))
        });
    });

    let high = clean_step(HIGH_SERIES_LEN);
    group.bench_function(format!("{HIGH_SERIES_LEN}-points"), |b| {
        b.iter(|| {
            black_box(selection_adjusted_change_point(
                black_box(&high),
                LOW_LEFT_LEN,
                calibration,
            ))
        });
    });

    let overlapping = noisy_short_step(OVERLAPPING_STEP_OFFSET);
    let capped = SelectionCalibration {
        permutation_order_budget: NonZero::new(MAX_PERMUTATION_ORDER)
            .expect("the production maximum is nonzero"),
        analytic_weight: ANALYTIC_WEIGHT,
        accept_analytic_below: LARGE_FAMILY_ACCEPTANCE_LEVEL,
        reject_at_or_above: REJECTION_LEVEL,
    };
    // Preserve production calibration settings, but measure a bounded rejection.
    // Full large-family orbit exhaustion is not a routine microbenchmark workload.
    // Check that the fixture reaches calibration instead of the unadjusted-score gate,
    // and distinguishes decision-based rejection from the completed calibration.
    let rejected = selection_adjusted_change_point(&overlapping, LOW_LEFT_LEN, capped)
        .expect("the overlapping fixture has a reportable split");
    let completed = selection_adjusted_change_point(
        &overlapping,
        LOW_LEFT_LEN,
        SelectionCalibration {
            // Disable decision-based rejection to distinguish its sentinel from the
            // completed calibration result, while retaining the same orbit and budget.
            reject_at_or_above: 1.0,
            ..capped
        },
    )
    .expect("the overlapping fixture has a reportable split");
    assert!(rejected.tainted_p < REJECTION_LEVEL, "{rejected:?}");
    assert!(completed.adjusted_p >= REJECTION_LEVEL, "{completed:?}");
    assert!(
        rejected.adjusted_p > completed.adjusted_p,
        "{rejected:?} must reject before the completed result {completed:?}"
    );

    group.bench_function("12-points/large-family-early-rejection", |b| {
        b.iter(|| {
            black_box(selection_adjusted_change_point(
                black_box(&overlapping),
                LOW_LEFT_LEN,
                capped,
            ))
        });
    });

    group.finish();
}

fn separated_samples(left_len: usize, series_len: usize) -> (Vec<f64>, Vec<f64>) {
    let left = (0..left_len).map(count_f64).collect();
    let right = (left_len..series_len).map(count_f64).collect();
    (left, right)
}

fn clean_step(series_len: usize) -> Vec<f64> {
    let split = series_len.checked_div(2).expect("the divisor is nonzero");
    [
        vec![10.0; split],
        vec![20.0; series_len.saturating_sub(split)],
    ]
    .concat()
}

fn noisy_short_step(offset: f64) -> Vec<f64> {
    // Repeated but nonconstant ranks force subgroup calibration rather than the
    // small complete orbit of a two-level tied step.
    let before = [98.0, 100.0, 102.0, 99.0, 101.0, 100.0];
    before
        .into_iter()
        .chain(before.map(|value| value + offset))
        .collect()
}

#[expect(
    clippy::cast_precision_loss,
    reason = "bounded benchmark series lengths are exactly representable as f64"
)]
fn count_f64(count: usize) -> f64 {
    count as f64
}
