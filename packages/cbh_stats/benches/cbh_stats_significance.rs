//! Benchmarks change-point significance scoring and selection adjustment.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::num::NonZero;

use cbh_stats::{MannWhitneyU, selection_adjusted_change_point};
use criterion::{Criterion, criterion_group, criterion_main};

criterion_group!(benches, exact_lopsided, selection_adjustment);
criterion_main!(benches);

/// Representative of a typical benchmark history.
const LOW_SERIES_LEN: usize = 100;
/// The production persistence floor, which is the smallest reportable side.
const LOW_LEFT_LEN: usize = 5;
/// The production cap, covering the most expensive supported history.
const HIGH_SERIES_LEN: usize = 1000;
/// The largest lopsided side that remains exactly enumerable at the production cap.
const HIGH_LEFT_LEN: usize = 6;
/// Production calibration budget for a family containing one judged series.
const PERMUTATIONS: usize = 600;
/// Pre-arbitration rejection boundary used by the history detector.
const REJECTION_LEVEL: f64 = 0.025;

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

    let low = clean_step(LOW_SERIES_LEN);
    group.bench_function("100-points", |b| {
        b.iter(|| {
            black_box(selection_adjusted_change_point(
                black_box(&low),
                LOW_LEFT_LEN,
                budget,
                REJECTION_LEVEL,
            ))
        });
    });

    let high = clean_step(HIGH_SERIES_LEN);
    group.bench_function("1000-points", |b| {
        b.iter(|| {
            black_box(selection_adjusted_change_point(
                black_box(&high),
                LOW_LEFT_LEN,
                budget,
                REJECTION_LEVEL,
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

#[expect(
    clippy::cast_precision_loss,
    reason = "benchmark series lengths are at most the 1,000-point production cap"
)]
fn count_f64(count: usize) -> f64 {
    count as f64
}
