//! Dispersion statistics derived from an operation's recorded spans.
//!
//! Each measured span contributes one `(iterations, total_nanos)` observation.
//! Because a span corresponds to one Criterion sample (one `iter_custom`
//! closure call), the spans of an operation form the same per-sample population
//! Criterion analyzes for its own wall-clock estimates. This module reproduces
//! the parts of that analysis that the history tool consumes:
//!
//! * a through-origin OLS **slope** as the per-iteration point estimate
//!   (`slope = Σ(nᵢ·tᵢ) / Σ(nᵢ²)`), robust to fixed per-call overhead and, as a
//!   side effect, to low-iteration warm-up spans, and
//! * a **bootstrap** confidence interval of that slope (resample the spans with
//!   replacement, recompute the slope, take percentiles), matching Criterion's
//!   confidence-interval method.
//!
//! All work here runs once, when a [`Report`](crate::Report) is materialized for
//! output — never inside a measured span.

#![expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "iteration and nanosecond counts stay well below 2^53, and bootstrap \
              percentile indices are clamped into range before the cast"
)]

use rand::rngs::SmallRng;
use rand::{RngExt, SeedableRng};

/// Number of bootstrap resamples drawn when estimating the confidence interval.
///
/// Matches Criterion's default so the two engines' intervals are produced by the
/// same method at the same resolution.
const BOOTSTRAP_RESAMPLES: usize = 100_000;

/// Confidence level of the reported interval (95%, matching Criterion's default).
const CONFIDENCE_LEVEL: f64 = 0.95;

/// Fixed seed for the bootstrap RNG.
///
/// A fixed seed makes the interval a deterministic function of the recorded
/// spans, so re-running an analysis over unchanged data yields identical bounds.
const BOOTSTRAP_SEED: u64 = 0x5A11_0C8E_BEE5_F00D;

/// One recorded span: the processor time a single measured span consumed and the
/// number of iterations it covered.
///
/// The raw whole-span total is retained (not pre-divided per iteration) so that
/// the slope regression can weight each span by its iteration count.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SpanRecord {
    /// Iterations the span measured.
    pub(crate) iterations: u64,

    /// Total processor time the span consumed, in nanoseconds.
    pub(crate) total_nanos: u64,
}

/// Dispersion statistics for one operation, derived from its spans.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OperationStatistics {
    /// Number of spans recorded (distinct from the total iteration count).
    pub(crate) span_count: u64,

    /// Through-origin OLS slope: the per-iteration processor time point estimate.
    pub(crate) slope_nanos: f64,

    /// Sample standard deviation of the per-iteration values across spans.
    pub(crate) std_dev_nanos: f64,

    /// Lower bound of the slope's bootstrap confidence interval.
    pub(crate) interval_low_nanos: f64,

    /// Upper bound of the slope's bootstrap confidence interval.
    pub(crate) interval_high_nanos: f64,

    /// Smallest per-iteration value observed across spans.
    pub(crate) min_nanos: f64,

    /// Largest per-iteration value observed across spans.
    pub(crate) max_nanos: f64,
}

/// Computes the dispersion statistics for a non-empty set of spans.
///
/// Returns `None` when `spans` is empty, since an operation with no measured
/// work has no statistics to report.
pub(crate) fn compute_statistics(spans: &[SpanRecord]) -> Option<OperationStatistics> {
    if spans.is_empty() {
        return None;
    }

    let slope_nanos = slope_nanos(spans);
    let (interval_low_nanos, interval_high_nanos) = bootstrap_interval(spans);
    let (min_nanos, max_nanos) = min_max(spans);

    Some(OperationStatistics {
        span_count: spans.len() as u64,
        slope_nanos,
        std_dev_nanos: std_dev_nanos(spans),
        interval_low_nanos,
        interval_high_nanos,
        min_nanos,
        max_nanos,
    })
}

/// Per-iteration value of a span: its total processor time divided by iterations.
fn per_iteration_nanos(span: SpanRecord) -> f64 {
    let iterations = span.iterations as f64;
    if iterations == 0.0 {
        0.0
    } else {
        span.total_nanos as f64 / iterations
    }
}

/// Through-origin OLS slope `Σ(nᵢ·tᵢ) / Σ(nᵢ²)` over the spans.
fn slope_nanos(spans: &[SpanRecord]) -> f64 {
    let mut numerator = 0.0_f64;
    let mut denominator = 0.0_f64;
    for span in spans {
        let iterations = span.iterations as f64;
        let total = span.total_nanos as f64;
        numerator += iterations * total;
        denominator += iterations * iterations;
    }

    if denominator == 0.0 {
        0.0
    } else {
        numerator / denominator
    }
}

/// Sample standard deviation (Bessel-corrected) of the per-iteration values.
///
/// Returns zero for fewer than two spans, where dispersion is undefined.
fn std_dev_nanos(spans: &[SpanRecord]) -> f64 {
    let count = spans.len() as f64;
    if count < 2.0 {
        return 0.0;
    }

    let mut sum = 0.0_f64;
    for span in spans {
        sum += per_iteration_nanos(*span);
    }
    let mean = sum / count;

    let mut sum_squared_deviation = 0.0_f64;
    for span in spans {
        let deviation = per_iteration_nanos(*span) - mean;
        sum_squared_deviation += deviation * deviation;
    }

    (sum_squared_deviation / (count - 1.0)).sqrt()
}

/// Smallest and largest per-iteration values across the spans.
fn min_max(spans: &[SpanRecord]) -> (f64, f64) {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for span in spans {
        let value = per_iteration_nanos(*span);
        min = min.min(value);
        max = max.max(value);
    }
    (min, max)
}

/// Bootstrap percentile confidence interval of the slope.
///
/// Resamples the spans with replacement [`BOOTSTRAP_RESAMPLES`] times, recomputes
/// the slope of each resample, and returns the lower and upper percentiles for
/// [`CONFIDENCE_LEVEL`]. With a single span the interval collapses to the point
/// estimate.
fn bootstrap_interval(spans: &[SpanRecord]) -> (f64, f64) {
    if spans.len() < 2 {
        let slope = slope_nanos(spans);
        return (slope, slope);
    }

    let count = spans.len();
    let mut rng = SmallRng::seed_from_u64(BOOTSTRAP_SEED);
    let mut slopes = Vec::with_capacity(BOOTSTRAP_RESAMPLES);

    for _ in 0..BOOTSTRAP_RESAMPLES {
        let mut numerator = 0.0_f64;
        let mut denominator = 0.0_f64;
        for _ in 0..count {
            let index = rng.random_range(0..count);
            let span = *spans
                .get(index)
                .expect("random_range yields an in-bounds index");
            let iterations = span.iterations as f64;
            let total = span.total_nanos as f64;
            numerator += iterations * total;
            denominator += iterations * iterations;
        }
        slopes.push(if denominator == 0.0 {
            0.0
        } else {
            numerator / denominator
        });
    }

    slopes.sort_unstable_by(f64::total_cmp);

    let lower_fraction = (1.0 - CONFIDENCE_LEVEL) / 2.0;
    let upper_fraction = 1.0 - lower_fraction;
    (
        percentile(&slopes, lower_fraction),
        percentile(&slopes, upper_fraction),
    )
}

/// Linearly interpolated percentile of a sorted slice (type-7 quantile).
fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    match sorted {
        [] => 0.0,
        _ => {
            let max_index = sorted.len().saturating_sub(1);
            let rank = fraction * max_index as f64;
            let lower_index = rank.floor() as usize;
            let upper_index = lower_index.saturating_add(1).min(max_index);
            let interpolation = rank - rank.floor();

            let lower = *sorted
                .get(lower_index)
                .expect("lower index is clamped within bounds");
            let upper = *sorted
                .get(upper_index)
                .expect("upper index is clamped within bounds");
            lower + interpolation * (upper - lower)
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "statistics are exact integer-derived values in these fixtures"
    )]

    use super::*;

    fn span(iterations: u64, total_nanos: u64) -> SpanRecord {
        SpanRecord {
            iterations,
            total_nanos,
        }
    }

    #[test]
    fn empty_spans_have_no_statistics() {
        assert!(compute_statistics(&[]).is_none());
    }

    #[test]
    fn constant_iterations_make_slope_equal_the_mean() {
        // Every span runs the same iteration count, so the slope degenerates to
        // the plain per-iteration mean: (10 + 20 + 30) / 3 = 20.
        let spans = [span(1, 10), span(1, 20), span(1, 30)];
        let stats = compute_statistics(&spans).unwrap();
        assert_eq!(stats.slope_nanos, 20.0);
    }

    #[test]
    fn slope_weights_spans_by_iteration_count() {
        // Two spans on a perfectly linear series (5 ns/iter): the slope recovers
        // exactly 5 regardless of the differing iteration counts.
        let spans = [span(2, 10), span(8, 40)];
        let stats = compute_statistics(&spans).unwrap();
        assert_eq!(stats.slope_nanos, 5.0);
    }

    #[test]
    fn low_iteration_outlier_barely_moves_the_slope() {
        // A noisy single-iteration warm-up span (1000 ns) alongside many
        // high-iteration spans at 5 ns/iter is down-weighted by iters², so the
        // slope stays close to 5 rather than being dragged toward 1000.
        let spans = [span(1, 1000), span(1000, 5000), span(1000, 5000)];
        let stats = compute_statistics(&spans).unwrap();
        assert!(
            stats.slope_nanos < 6.0,
            "slope should resist the warm-up outlier: {}",
            stats.slope_nanos
        );
    }

    #[test]
    fn span_count_is_the_number_of_spans() {
        let spans = [span(1, 10), span(1, 20), span(1, 30), span(1, 40)];
        let stats = compute_statistics(&spans).unwrap();
        assert_eq!(stats.span_count, 4);
    }

    #[test]
    fn standard_deviation_of_a_single_span_is_zero() {
        let stats = compute_statistics(&[span(1, 42)]).unwrap();
        assert_eq!(stats.std_dev_nanos, 0.0);
    }

    #[test]
    fn standard_deviation_matches_a_hand_computed_value() {
        // Per-iteration values 10, 20, 30: mean 20, variance (100+0+100)/2 = 100,
        // standard deviation 10.
        let spans = [span(1, 10), span(1, 20), span(1, 30)];
        let stats = compute_statistics(&spans).unwrap();
        assert_eq!(stats.std_dev_nanos, 10.0);
    }

    #[test]
    fn min_and_max_track_the_per_iteration_extremes() {
        let spans = [span(2, 20), span(1, 5), span(4, 200)];
        // Per-iteration values: 10, 5, 50.
        let stats = compute_statistics(&spans).unwrap();
        assert_eq!(stats.min_nanos, 5.0);
        assert_eq!(stats.max_nanos, 50.0);
    }

    #[test]
    fn single_span_interval_collapses_to_the_point_estimate() {
        let stats = compute_statistics(&[span(4, 80)]).unwrap();
        assert_eq!(stats.slope_nanos, 20.0);
        assert_eq!(stats.interval_low_nanos, 20.0);
        assert_eq!(stats.interval_high_nanos, 20.0);
    }

    #[test]
    fn interval_brackets_the_point_estimate() {
        let spans = [
            span(1, 18),
            span(1, 20),
            span(1, 22),
            span(1, 19),
            span(1, 21),
            span(1, 20),
        ];
        let stats = compute_statistics(&spans).unwrap();
        assert!(
            stats.interval_low_nanos <= stats.slope_nanos
                && stats.slope_nanos <= stats.interval_high_nanos,
            "point {} must lie within [{}, {}]",
            stats.slope_nanos,
            stats.interval_low_nanos,
            stats.interval_high_nanos
        );
    }

    #[test]
    fn wider_spread_yields_a_wider_interval() {
        let tight = [
            span(1, 19),
            span(1, 20),
            span(1, 21),
            span(1, 20),
            span(1, 19),
            span(1, 21),
        ];
        let wide = [
            span(1, 5),
            span(1, 20),
            span(1, 35),
            span(1, 8),
            span(1, 32),
            span(1, 20),
        ];

        let tight_stats = compute_statistics(&tight).unwrap();
        let wide_stats = compute_statistics(&wide).unwrap();

        let tight_width = tight_stats.interval_high_nanos - tight_stats.interval_low_nanos;
        let wide_width = wide_stats.interval_high_nanos - wide_stats.interval_low_nanos;
        assert!(
            wide_width > tight_width,
            "wide interval ({wide_width}) should exceed tight interval ({tight_width})"
        );
    }

    #[test]
    fn bootstrap_is_deterministic_across_runs() {
        let spans = [
            span(1, 18),
            span(1, 20),
            span(1, 22),
            span(1, 19),
            span(1, 21),
        ];
        let first = compute_statistics(&spans).unwrap();
        let second = compute_statistics(&spans).unwrap();
        assert_eq!(first.interval_low_nanos, second.interval_low_nanos);
        assert_eq!(first.interval_high_nanos, second.interval_high_nanos);
    }

    #[test]
    fn two_spans_have_a_bessel_corrected_standard_deviation() {
        // Two spans is the smallest sample with defined dispersion: per-iteration
        // values 10 and 30, mean 20, sum of squared deviations 200, divided by the
        // Bessel-corrected (n - 1 = 1) denominator, square-rooted.
        let stats = compute_statistics(&[span(1, 10), span(1, 30)]).unwrap();
        assert_eq!(stats.std_dev_nanos, 200.0_f64.sqrt());
    }

    #[test]
    fn two_distinct_spans_widen_the_interval_beyond_the_point() {
        // With two differing spans the bootstrap resamples take three values
        // (both-low, mixed, both-high), so the interval spans a real range rather
        // than collapsing to the point estimate.
        let stats = compute_statistics(&[span(1, 10), span(1, 30)]).unwrap();
        assert!(
            stats.interval_low_nanos < stats.interval_high_nanos,
            "interval [{}, {}] should be non-degenerate",
            stats.interval_low_nanos,
            stats.interval_high_nanos
        );
    }

    #[test]
    fn identical_multi_iteration_spans_collapse_to_the_true_slope() {
        // Two identical spans (2 iterations, 80 ns total → 40 ns/iter) enter the
        // bootstrap loop, but every resample is identical, so the interval
        // collapses onto the true slope. The differing-from-one iteration count
        // makes the resampled slope sensitive to the weighting arithmetic.
        let stats = compute_statistics(&[span(2, 80), span(2, 80)]).unwrap();
        assert_eq!(stats.slope_nanos, 40.0);
        assert_eq!(stats.interval_low_nanos, 40.0);
        assert_eq!(stats.interval_high_nanos, 40.0);
    }

    #[test]
    fn bootstrap_interval_uses_the_two_and_a_half_percent_tails() {
        // A deterministic six-span fixture pins the exact 2.5%/97.5% bootstrap
        // bounds. Shifting the tail fractions (e.g. to 5%/95%) would land the
        // percentiles on different resampled slopes, changing these values.
        let spans = [
            span(1, 10),
            span(1, 14),
            span(1, 18),
            span(1, 22),
            span(1, 26),
            span(1, 30),
        ];
        let stats = compute_statistics(&spans).unwrap();
        assert_eq!(stats.interval_low_nanos, 14.666_666_666_666_666);
        assert_eq!(stats.interval_high_nanos, 25.333_333_333_333_332);
    }

    #[test]
    fn percentile_of_an_empty_slice_is_zero() {
        assert_eq!(percentile(&[], 0.5), 0.0);
    }

    #[test]
    fn percentile_interpolates_between_neighbors() {
        // fraction 0.625 over five points (max index 4) gives rank 2.5: halfway
        // between sorted[2] = 30 and sorted[3] = 40, i.e. 35.
        let sorted = [10.0, 20.0, 30.0, 40.0, 50.0];
        assert_eq!(percentile(&sorted, 0.625), 35.0);
    }

    #[test]
    fn per_iteration_value_of_a_zero_iteration_span_is_zero() {
        // A span recording zero iterations cannot be divided by its count, so its
        // per-iteration value degrades to zero rather than producing NaN.
        assert_eq!(per_iteration_nanos(span(0, 1000)), 0.0);
    }

    #[test]
    fn slope_of_zero_iteration_spans_is_zero() {
        // With every span at zero iterations the weighted denominator Σ(nᵢ²) is
        // zero, so the slope collapses to zero instead of dividing by zero.
        assert_eq!(slope_nanos(&[span(0, 1000), span(0, 2000)]), 0.0);
    }

    #[test]
    fn bootstrap_interval_of_zero_iteration_spans_is_zero() {
        // Two zero-iteration spans clear the `< 2` early return and enter the
        // resample loop, where every resample's denominator Σ(nᵢ²) is zero, so each
        // resampled slope — and thus both interval bounds — is zero.
        assert_eq!(
            bootstrap_interval(&[span(0, 100), span(0, 200)]),
            (0.0, 0.0)
        );
    }
}
