//! Benchmarks whole-report branch evaluation at representative evidence and suite sizes.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::mem;

use cbh_detect::{AnalysisContext, Series, examples, find_changes};
use cbh_model::MetricKind;
use criterion::{Criterion, criterion_group, criterion_main};

/// A short but useful branch history, representative of sparse repositories.
const LOW_BASE_COMMITS: usize = 20;
/// A small report that still exercises cross-series family scoring.
const LOW_SERIES: usize = 20;
/// The production branch-history cap.
const HIGH_BASE_COMMITS: usize = 128;
/// A large report that exercises report-wide scoring at the production history cap.
const HIGH_SERIES: usize = 1_000;
/// One selector and one reference observation retained or removed together.
const LANE_PAIR_SIZE: usize = 2;
/// Stable base value shared by every synthetic benchmark.
const BASE_VALUE: f64 = 100.0;
/// Branch value that produces a clear excursion for every synthetic benchmark.
const BRANCH_VALUE: f64 = 130.0;

fn whole_report(c: &mut Criterion) {
    let mut group = c.benchmark_group("cbh_detect_branch/whole_report");

    let (low, low_context) = branch_suite(LOW_BASE_COMMITS, LOW_SERIES);
    group.bench_function("20-commits/20-series", |b| {
        b.iter(|| black_box(find_changes(black_box(&low), black_box(&low_context))));
    });

    let (high, high_context) = branch_suite(HIGH_BASE_COMMITS, HIGH_SERIES);
    group.bench_function("128-commits/1000-series", |b| {
        b.iter(|| black_box(find_changes(black_box(&high), black_box(&high_context))));
    });

    let (mut diverse, diverse_context) = branch_suite(HIGH_BASE_COMMITS, HIGH_SERIES);
    diversify_base_evidence(&mut diverse);
    group.bench_function("128-commits/1000-diverse-series", |b| {
        b.iter(|| {
            black_box(find_changes(
                black_box(&diverse),
                black_box(&diverse_context),
            ))
        });
    });

    group.finish();
}

fn branch_suite(base_commits: usize, series_count: usize) -> (Vec<Series>, AnalysisContext) {
    let values: Vec<f64> = std::iter::repeat_n(BASE_VALUE, base_commits)
        .chain(std::iter::once(BRANCH_VALUE))
        .collect();
    let merge_base = base_commits.saturating_sub(1);
    let suite: Vec<Series> = (0..series_count)
        .map(|index| {
            let series = examples::series(
                &format!("branch_benchmark_{index}"),
                &values,
                MetricKind::WallTime,
                0,
            );
            examples::with_base_window(series, merge_base)
        })
        .collect();
    let context = suite
        .first()
        .map(|series| examples::branch_context(series, merge_base))
        .expect("a benchmark suite always contains at least one series");
    (suite, context)
}

fn diversify_base_evidence(suite: &mut [Series]) {
    let reference_commits = HIGH_BASE_COMMITS.div_ceil(LANE_PAIR_SIZE);
    for (series_index, series) in suite.iter_mut().enumerate() {
        let first_removed = series_index
            .checked_rem(reference_commits)
            .expect("the production branch window has reference commits");
        let second_removed = series_index
            .checked_div(reference_commits)
            .expect("the reference-commit count is nonzero");
        let levels = mem::take(&mut series.base_window);
        series.base_window = levels
            .into_iter()
            .enumerate()
            .filter(|(position, _)| {
                let pair = position
                    .checked_div(LANE_PAIR_SIZE)
                    .expect("a lane pair has observations");
                pair != first_removed && pair != second_removed
            })
            .map(|(_, level)| level)
            .collect();
    }
}

criterion_group!(benches, whole_report);
criterion_main!(benches);
