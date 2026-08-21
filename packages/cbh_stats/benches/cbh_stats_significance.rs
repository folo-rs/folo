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

/// Representative of a typical benchmark history.
const LOW_SERIES_LEN: usize = 100;
/// The production persistence floor, which is the smallest reportable side.
const LOW_LEFT_LEN: usize = 5;
/// The production cap, covering the most expensive supported history.
const HIGH_SERIES_LEN: usize = 1000;
/// Short history that keeps a full-ceiling ambiguous benchmark practical.
///
/// Permutation work also scales with series length; the separate high-length case
/// measures the analytic path at the production series cap.
const AMBIGUOUS_SERIES_LEN: usize = 50;
/// The largest lopsided side that remains exactly enumerable at the production cap.
const HIGH_LEFT_LEN: usize = 6;
/// Production calibration budget for a family containing one judged series.
const PERMUTATIONS: usize = 600;
/// Production sequential exceedance limit.
const EXCEEDANCES: usize = 30;
/// Production weight of the analytic calibration component.
const ANALYTIC_WEIGHT: f64 = 0.10;
/// Rank-1 acceptance level for a family containing one judged series.
const ACCEPTANCE_LEVEL: f64 = 0.025;
/// Production maximum reached by an unresolved large-family candidate.
const MAX_PERMUTATIONS: usize = 500_000;
/// Rank-1 pre-arbitration level at the stress harness's large-family scale.
const LARGE_FAMILY_ACCEPTANCE_LEVEL: f64 = 0.000_002_5;
/// Pre-arbitration rejection boundary used by the history detector.
const REJECTION_LEVEL: f64 = 0.025;
/// Offset that makes the benchmark fixture significant but heavily overlapping.
const AMBIGUOUS_STEP_OFFSET: f64 = 9.0;
/// Multiplier that traverses the benchmark fixture's residue cycle.
const AMBIGUOUS_CYCLE_MULTIPLIER: usize = 13;
/// Modulus that gives the benchmark fixture a broad repeated-value distribution.
const AMBIGUOUS_CYCLE_MODULUS: usize = 17;

fn exact_lopsided(c: &mut Criterion) {
    let mut group = c.benchmark_group("cbh_stats_significance/exact_lopsided");

    let low = separated_samples(LOW_LEFT_LEN, LOW_SERIES_LEN);
    group.bench_function("5-vs-95", |b| {
        b.iter(|| black_box(MannWhitneyU::new(black_box(&low.0), black_box(&low.1))));
    });

    let high = separated_samples(HIGH_LEFT_LEN, HIGH_SERIES_LEN);
    group.bench_function("6-vs-994", |b| {
        b.iter(|| black_box(MannWhitneyU::new(black_box(&high.0), black_box(&high.1))));
    });

    group.finish();
}

fn selection_adjustment(c: &mut Criterion) {
    let mut group = c.benchmark_group("cbh_stats_significance/selection_adjustment");
    let budget = NonZero::new(PERMUTATIONS).expect("the production budget is nonzero");
    let calibration = SelectionCalibration {
        permutations: budget,
        exceedances: NonZero::new(EXCEEDANCES).expect("the production exceedance limit is nonzero"),
        analytic_weight: ANALYTIC_WEIGHT,
        accept_analytic_below: ACCEPTANCE_LEVEL,
        reject_at_or_above: REJECTION_LEVEL,
    };

    let low = clean_step(LOW_SERIES_LEN);
    group.bench_function("100-points", |b| {
        b.iter(|| {
            black_box(selection_adjusted_change_point(
                black_box(&low),
                LOW_LEFT_LEN,
                calibration,
            ))
        });
    });

    let high = clean_step(HIGH_SERIES_LEN);
    group.bench_function("1000-points", |b| {
        b.iter(|| {
            black_box(selection_adjusted_change_point(
                black_box(&high),
                LOW_LEFT_LEN,
                calibration,
            ))
        });
    });

    let ambiguous = ambiguous_step();
    let capped = SelectionCalibration {
        permutations: NonZero::new(MAX_PERMUTATIONS).expect("the production maximum is nonzero"),
        exceedances: NonZero::new(EXCEEDANCES).expect("the production exceedance limit is nonzero"),
        analytic_weight: ANALYTIC_WEIGHT,
        accept_analytic_below: LARGE_FAMILY_ACCEPTANCE_LEVEL,
        reject_at_or_above: REJECTION_LEVEL,
    };
    group.bench_function("50-points/capped-20000-family", |b| {
        b.iter(|| {
            black_box(selection_adjusted_change_point(
                black_box(&ambiguous),
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

fn ambiguous_step() -> Vec<f64> {
    let split = AMBIGUOUS_SERIES_LEN
        .checked_div(2)
        .expect("the divisor is nonzero");
    let before: Vec<f64> = (0..split)
        .map(|index| {
            count_f64(
                index
                    .saturating_mul(AMBIGUOUS_CYCLE_MULTIPLIER)
                    .rem_euclid(AMBIGUOUS_CYCLE_MODULUS),
            )
        })
        .collect();
    let after: Vec<f64> = before
        .iter()
        .map(|value| value + AMBIGUOUS_STEP_OFFSET)
        .collect();
    before.into_iter().chain(after).collect()
}

#[expect(
    clippy::cast_precision_loss,
    reason = "benchmark series lengths are at most the 1,000-point production cap"
)]
fn count_f64(count: usize) -> f64 {
    count as f64
}
