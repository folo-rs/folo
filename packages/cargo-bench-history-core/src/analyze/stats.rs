//! Pure statistical primitives for the analysis detectors.
//!
//! Everything here is deterministic, allocation-light, and free of I/O or the
//! wall clock, so it runs under Miri and is unit-tested with named,
//! value-asserting cases on hand-computable inputs. The detectors in
//! [`findings`](super::findings) compose these primitives; keeping the math
//! isolated keeps both halves easy to reason about.

use std::cmp::Ordering;
use std::f64::consts;

/// Casts a small count to `f64`. Series lengths are far below 2^53, so the
/// conversion is exact.
#[expect(
    clippy::cast_precision_loss,
    reason = "series lengths are far below 2^53, so the cast is exact"
)]
fn count_to_f64(count: usize) -> f64 {
    count as f64
}

/// Whether two finite values are bit-for-bit equal (tie detection for ranks).
fn same(left: f64, right: f64) -> bool {
    left.total_cmp(&right) == Ordering::Equal
}

/// The median of `values`, or `None` if empty.
///
/// Uses [`f64::total_cmp`] so `NaN` cannot corrupt the ordering, and computes the
/// midpoint index with checked integer arithmetic to satisfy the workspace lints.
///
/// This copies `values` into a scratch buffer first; a caller that already owns a
/// buffer it no longer needs in input order should call [`median_in_place`] to
/// avoid the copy.
pub(crate) fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    median_in_place(&mut sorted)
}

/// The median of `values`, sorting them in place rather than copying.
///
/// The allocation-free core of [`median`]: it sorts `values` with
/// [`f64::total_cmp`] (so `NaN` cannot corrupt the ordering) and reads the
/// midpoint, leaving the slice sorted. Returns `None` for an empty slice.
pub(crate) fn median_in_place(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let len = values.len();
    let mid = len.checked_div(2)?;
    if len.checked_rem(2) == Some(1) {
        values.get(mid).copied()
    } else {
        let lower = mid.checked_sub(1)?;
        let low = *values.get(lower)?;
        let high = *values.get(mid)?;
        Some(f64::midpoint(low, high))
    }
}

/// The standard normal cumulative distribution function, `Φ(z)`.
///
/// Evaluated via the Abramowitz & Stegun 7.1.26 rational approximation of `erf`
/// (absolute error below 1.5e-7), which is ample for deriving p-values.
pub(crate) fn normal_cdf(z: f64) -> f64 {
    0.5 * (1.0 + erf(z / consts::SQRT_2))
}

/// The error function `erf(x)` (Abramowitz & Stegun 7.1.26).
fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let poly = ((((1.061_405_429 * t - 1.453_152_027) * t + 1.421_413_741) * t - 0.284_496_736)
        * t
        + 0.254_829_592)
        * t;
    let y = 1.0 - poly * (-x * x).exp();
    sign * y
}

/// The two-sided p-value for a standard-normal test statistic `z`.
pub(crate) fn two_sided_p_from_z(z: f64) -> f64 {
    let p = 2.0 * (1.0 - normal_cdf(z.abs()));
    p.clamp(0.0, 1.0)
}

/// Average (fractional) ranks of `values`, 1-based, with ties sharing the mean
/// of the ranks they span.
fn average_ranks(values: &[f64]) -> Vec<f64> {
    let mut indexed: Vec<(usize, f64)> = values.iter().copied().enumerate().collect();
    indexed.sort_by(|left, right| left.1.total_cmp(&right.1));

    let mut ranks = vec![0.0_f64; values.len()];
    let mut start = 0_usize;
    for group in indexed.chunk_by(|left, right| same(left.1, right.1)) {
        let end = start.saturating_add(group.len());
        // The 1-based ranks spanned by the tie run are `start+1 ..= end`; their
        // mean is the midpoint of the first and last.
        let first = count_to_f64(start.saturating_add(1));
        let last = count_to_f64(end);
        let average = f64::midpoint(first, last);
        for &(original_index, _) in group {
            if let Some(slot) = ranks.get_mut(original_index) {
                *slot = average;
            }
        }
        start = end;
    }
    ranks
}

/// Sizes of the tie groups in `values` (groups of one are omitted as they do not
/// affect any tie correction).
fn tie_group_sizes(values: &[f64]) -> Vec<usize> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted
        .chunk_by(|left, right| same(*left, *right))
        .map(<[f64]>::len)
        .filter(|&size| size > 1)
        .collect()
}

/// A located level shift in a series.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ChangePoint {
    /// Index where the *after* regime begins (`points[index..]`), in `1..n`.
    pub(crate) index: usize,
    /// The Pettitt `K` statistic (larger means a more pronounced split).
    pub(crate) k_statistic: f64,
    /// The approximate two-sided significance of the split.
    pub(crate) p_value: f64,
}

/// Locates the single most likely level shift with the **Pettitt** nonparametric
/// change-point test.
///
/// Returns `None` for fewer than two points. The statistic is the rank form
/// `U_t = 2·R_t − t·(n+1)` (with `R_t` the sum of the first `t` average ranks);
/// `K = max_t |U_t|`, the change index is `argmax`, and the significance is
/// `p ≈ 2·exp(−6K²/(n³+n²))` (clamped to `[0, 1]`). The first maximizing `t`
/// wins, so a perfectly flat series reports a degenerate split at index 1 that the
/// caller rejects on a zero median difference.
pub(crate) fn pettitt(values: &[f64]) -> Option<ChangePoint> {
    let n = values.len();
    if n < 2 {
        return None;
    }
    let ranks = average_ranks(values);
    let n_f = count_to_f64(n);

    let mut prefix_rank_sum = 0.0_f64;
    let mut best_index = 1_usize;
    let mut best_abs = -1.0_f64;
    let mut best_u = 0.0_f64;
    // `t` runs over the candidate split positions `1 ..= n-1`.
    let last = n.checked_sub(1)?;
    for (position, &rank) in ranks.iter().enumerate() {
        prefix_rank_sum += rank;
        let t = position.saturating_add(1);
        if t > last {
            break;
        }
        let t_f = count_to_f64(t);
        let u = 2.0 * prefix_rank_sum - t_f * (n_f + 1.0);
        let abs = u.abs();
        if abs > best_abs {
            best_abs = abs;
            best_index = t;
            best_u = u;
        }
    }

    let k = best_u.abs();
    let denominator = n_f * n_f * n_f + n_f * n_f;
    let p_value = (2.0 * (-6.0 * k * k / denominator).exp()).clamp(0.0, 1.0);
    Some(ChangePoint {
        index: best_index,
        k_statistic: k,
        p_value,
    })
}

/// The two-sided p-value of the **Mann–Whitney U** test that `left` and `right`
/// are drawn from the same distribution, via the tie- and continuity-corrected
/// normal approximation.
///
/// Returns `1.0` (no evidence of a difference) when either sample is empty or the
/// corrected variance is zero.
pub(crate) fn mann_whitney_u_pvalue(left: &[f64], right: &[f64]) -> f64 {
    let n1 = left.len();
    let n2 = right.len();
    if n1 == 0 {
        return 1.0;
    }
    if n2 == 0 {
        return 1.0;
    }
    let mut combined = Vec::with_capacity(n1.saturating_add(n2));
    combined.extend_from_slice(left);
    combined.extend_from_slice(right);
    let ranks = average_ranks(&combined);

    let rank_sum_left: f64 = ranks.iter().take(n1).sum();
    let n1_f = count_to_f64(n1);
    let n2_f = count_to_f64(n2);
    let n_f = n1_f + n2_f;

    let u1 = rank_sum_left - n1_f * (n1_f + 1.0) / 2.0;
    let u2 = n1_f * n2_f - u1;
    let u = u1.min(u2);
    let mean_u = n1_f * n2_f / 2.0;

    let tie_term: f64 = tie_group_sizes(&combined)
        .into_iter()
        .map(|size| {
            let t = count_to_f64(size);
            t * t * t - t
        })
        .sum();
    let variance = (n1_f * n2_f / 12.0) * ((n_f + 1.0) - tie_term / (n_f * (n_f - 1.0)));
    if variance <= 0.0 {
        return 1.0;
    }

    // Continuity-corrected z; `u` is the smaller statistic so `mean_u - u >= 0`.
    let z = ((mean_u - u) - 0.5).max(0.0) / variance.sqrt();
    two_sided_p_from_z(z)
}

/// The outcome of a Mann–Kendall trend test.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MannKendall {
    /// The `S` statistic (positive for an upward trend, negative for downward).
    pub(crate) s: f64,
    /// The two-sided significance of the trend.
    pub(crate) p_value: f64,
}

/// The **Mann–Kendall** test for a monotonic trend in `values` (time order is the
/// slice order), tie-corrected with a continuity correction on `Z`.
///
/// Returns `S = 0` and `p = 1.0` for fewer than three points or zero variance.
pub(crate) fn mann_kendall(values: &[f64]) -> MannKendall {
    let n = values.len();
    if n < 3 {
        return MannKendall {
            s: 0.0,
            p_value: 1.0,
        };
    }
    let mut s = 0.0_f64;
    for (i, &earlier) in values.iter().enumerate() {
        for &later in values.iter().skip(i.saturating_add(1)) {
            s += match later.total_cmp(&earlier) {
                Ordering::Greater => 1.0,
                Ordering::Less => -1.0,
                Ordering::Equal => 0.0,
            };
        }
    }

    let n_f = count_to_f64(n);
    let tie_term: f64 = tie_group_sizes(values)
        .into_iter()
        .map(|size| {
            let t = count_to_f64(size);
            t * (t - 1.0) * (2.0 * t + 5.0)
        })
        .sum();
    let variance = (n_f * (n_f - 1.0) * (2.0 * n_f + 5.0) - tie_term) / 18.0;
    if variance <= 0.0 {
        return MannKendall { s, p_value: 1.0 };
    }

    let z = if s > 0.0 {
        (s - 1.0) / variance.sqrt()
    } else if s < 0.0 {
        (s + 1.0) / variance.sqrt()
    } else {
        0.0
    };
    MannKendall {
        s,
        p_value: two_sided_p_from_z(z),
    }
}

/// The **Theil–Sen** robust line `(slope, intercept)` fitted to `values` against
/// their integer positions, or `None` for fewer than two points.
///
/// The slope is the median of all pairwise slopes; the intercept is the median of
/// `value_i − slope·i`, so the fitted endpoints are `intercept` and
/// `intercept + slope·(n−1)`.
pub(crate) fn theil_sen_line(values: &[f64]) -> Option<(f64, f64)> {
    let n = values.len();
    if n < 2 {
        return None;
    }
    let mut slopes = Vec::new();
    for (i, &earlier) in values.iter().enumerate() {
        let i_f = count_to_f64(i);
        for (j, &later) in values.iter().enumerate().skip(i.saturating_add(1)) {
            let span = count_to_f64(j) - i_f;
            slopes.push((later - earlier) / span);
        }
    }
    let slope = median_in_place(&mut slopes)?;

    let mut intercepts: Vec<f64> = values
        .iter()
        .enumerate()
        .map(|(i, &value)| value - slope * count_to_f64(i))
        .collect();
    let intercept = median_in_place(&mut intercepts)?;
    Some((slope, intercept))
}

/// Applies the **Benjamini–Hochberg** procedure to `p_values` at false-discovery
/// rate `q`, returning a keep-mask (in the input order) of the rejected (kept)
/// hypotheses.
///
/// Finds the largest rank `k` whose ordered p-value satisfies `p_(k) ≤ (k/m)·q`
/// and rejects every hypothesis of rank `≤ k` (the step-up property: an
/// intermediate rank that fails its own threshold is still rejected when a later
/// rank passes).
pub(crate) fn benjamini_hochberg(p_values: &[f64], q: f64) -> Vec<bool> {
    let m = p_values.len();
    if m == 0 {
        return Vec::new();
    }
    let m_f = count_to_f64(m);

    let mut ordered: Vec<(usize, f64)> = p_values.iter().copied().enumerate().collect();
    ordered.sort_by(|left, right| left.1.total_cmp(&right.1));

    // The largest 1-based rank whose ordered p-value clears `(k/m)·q`.
    let mut max_rank = 0_usize;
    for (position, &(_, p)) in ordered.iter().enumerate() {
        let rank = position.saturating_add(1);
        let threshold = count_to_f64(rank) / m_f * q;
        if p <= threshold {
            max_rank = rank;
        }
    }

    let mut keep = vec![false; m];
    for &(original_index, _) in ordered.iter().take(max_rank) {
        if let Some(slot) = keep.get_mut(original_index) {
            *slot = true;
        }
    }
    keep
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "primitive outputs are compared against hand-computed exact values"
    )]
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use super::*;

    /// Asserts `actual` is within `tolerance` of `expected`.
    fn close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {expected} (±{tolerance}), got {actual}"
        );
    }

    #[test]
    fn median_of_odd_and_even_counts() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), Some(2.5));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn normal_cdf_at_known_points() {
        close(normal_cdf(0.0), 0.5, 1e-6);
        // The A&S erf approximation makes erf(0) a hair positive, so Φ(0) sits
        // just above 0.5; this pins the sign branch in `erf` (a flipped sign would
        // push Φ(0) below 0.5).
        assert!(normal_cdf(0.0) > 0.5);
        close(normal_cdf(1.96), 0.975, 1e-3);
        close(normal_cdf(-1.96), 0.025, 1e-3);
        // Symmetry: Φ(-z) == 1 − Φ(z).
        close(normal_cdf(-1.0), 1.0 - normal_cdf(1.0), 1e-9);
    }

    #[test]
    fn average_ranks_share_ranks_across_ties() {
        // Three 1s span ranks 1..3 → 2.0; three 5s span 4..6 → 5.0.
        assert_eq!(
            average_ranks(&[1.0, 1.0, 1.0, 5.0, 5.0, 5.0]),
            vec![2.0, 2.0, 2.0, 5.0, 5.0, 5.0]
        );
        // Distinct values keep position-independent ranks.
        assert_eq!(average_ranks(&[30.0, 10.0, 20.0]), vec![3.0, 1.0, 2.0]);
    }

    #[test]
    fn tie_group_sizes_omits_singletons() {
        assert_eq!(tie_group_sizes(&[1.0, 1.0, 2.0, 3.0, 3.0, 3.0]), vec![2, 3]);
        assert_eq!(tie_group_sizes(&[1.0, 2.0, 3.0]), Vec::<usize>::new());
    }

    #[test]
    fn pettitt_locates_a_clean_step() {
        // A clean step from 1 to 5 after three points: K = 9 at index 3.
        let change = pettitt(&[1.0, 1.0, 1.0, 5.0, 5.0, 5.0]).unwrap();
        assert_eq!(change.index, 3);
        assert_eq!(change.k_statistic, 9.0);
        // p ≈ 2·exp(−6·81/252) ≈ 0.291 — deliberately not tiny, which is why
        // deterministic series must not gate a real step on significance.
        close(change.p_value, 0.291, 1e-3);
    }

    #[test]
    fn pettitt_step_is_at_the_boundary_for_an_asymmetric_split() {
        // Step after the second point: before [10,10], after [40,40,40,40].
        let change = pettitt(&[10.0, 10.0, 40.0, 40.0, 40.0, 40.0]).unwrap();
        assert_eq!(change.index, 2);
    }

    #[test]
    fn pettitt_flat_series_degenerates_at_index_one() {
        let change = pettitt(&[5.0, 5.0, 5.0, 5.0]).unwrap();
        assert_eq!(change.index, 1);
        assert_eq!(change.k_statistic, 0.0);
        assert_eq!(change.p_value, 1.0);
    }

    #[test]
    fn pettitt_needs_two_points() {
        assert_eq!(pettitt(&[]), None);
        assert_eq!(pettitt(&[1.0]), None);
    }

    #[test]
    fn pettitt_handles_the_two_point_minimum() {
        // Two ascending points are the smallest series Pettitt accepts: the only
        // split is at index 1 with U_1 = 2·1 − 1·3 = −1, so K = 1 (and p clamps to
        // 1.0). This pins both the `n < 2` lower bound and the `best_abs` seed.
        let change = pettitt(&[1.0, 2.0]).unwrap();
        assert_eq!(change.index, 1);
        assert_eq!(change.k_statistic, 1.0);
        assert_eq!(change.p_value, 1.0);
    }

    #[test]
    fn mann_whitney_separates_disjoint_samples() {
        // Fully separated samples of five: U = 0, z ≈ 2.507, p ≈ 0.0122.
        let p = mann_whitney_u_pvalue(&[1.0, 2.0, 3.0, 4.0, 5.0], &[11.0, 12.0, 13.0, 14.0, 15.0]);
        close(p, 0.0122, 2e-3);
    }

    #[test]
    fn mann_whitney_identical_samples_are_indistinguishable() {
        // Equal samples → variance zero → p = 1.0.
        let p = mann_whitney_u_pvalue(&[5.0, 5.0, 5.0], &[5.0, 5.0, 5.0]);
        assert_eq!(p, 1.0);
    }

    #[test]
    fn mann_whitney_empty_sample_is_one() {
        assert_eq!(mann_whitney_u_pvalue(&[], &[1.0, 2.0]), 1.0);
        assert_eq!(mann_whitney_u_pvalue(&[1.0, 2.0], &[]), 1.0);
    }

    #[test]
    fn mann_whitney_with_ties_uses_the_smaller_u_statistic() {
        // `right` sits below `left`, so U2 (0.5) is the smaller statistic, and the
        // repeated 4s/2s exercise the tie correction. The exact tie- and
        // continuity-corrected p (z ≈ 2.0578) is pinned tightly so the U2,
        // tie-term, variance, and continuity arithmetic all have to be exact.
        let p = mann_whitney_u_pvalue(&[3.0, 4.0, 4.0, 5.0], &[1.0, 2.0, 2.0, 3.0]);
        close(p, 0.039_608_571_971_576_41, 1e-9);
    }

    #[test]
    fn mann_kendall_detects_a_monotonic_increase() {
        // Strictly increasing six points: S = 15, z ≈ 2.630, p ≈ 0.0085.
        let result = mann_kendall(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(result.s, 15.0);
        close(result.p_value, 0.0085, 1e-3);
    }

    #[test]
    fn mann_kendall_is_sign_symmetric() {
        let up = mann_kendall(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let down = mann_kendall(&[6.0, 5.0, 4.0, 3.0, 2.0, 1.0]);
        assert_eq!(down.s, -15.0);
        close(up.p_value, down.p_value, 1e-9);
    }

    #[test]
    fn mann_kendall_flat_series_has_no_trend() {
        let result = mann_kendall(&[5.0, 5.0, 5.0, 5.0]);
        assert_eq!(result.s, 0.0);
        assert_eq!(result.p_value, 1.0);
    }

    #[test]
    fn mann_kendall_needs_three_points() {
        assert_eq!(
            mann_kendall(&[1.0, 2.0]),
            MannKendall {
                s: 0.0,
                p_value: 1.0
            }
        );
    }

    #[test]
    fn mann_kendall_scores_the_three_point_minimum() {
        // Three ascending points are the smallest series the trend test scores:
        // all three pairs rise, so S = 3 (a flipped `n < 3` bound would short out
        // to S = 0).
        let result = mann_kendall(&[1.0, 2.0, 3.0]);
        assert_eq!(result.s, 3.0);
        close(result.p_value, 0.296_269_924_336_563_4, 1e-9);
    }

    #[test]
    fn mann_kendall_zero_trend_with_variance_is_insignificant() {
        // A non-monotonic series with no ties has S = 0 but positive variance, so
        // it reaches the sign branch with z = 0 → p ≈ 1.0. A `>`/`<` boundary slip
        // on the sign test would feed z = ∓1/σ and collapse the p-value.
        let result = mann_kendall(&[2.0, 4.0, 1.0, 3.0]);
        assert_eq!(result.s, 0.0);
        close(result.p_value, 1.0, 1e-6);
    }

    #[test]
    fn mann_kendall_tie_correction_shrinks_variance() {
        // Repeated endpoints (two 1s, two 3s) engage the tie correction; the exact
        // tie-corrected p pins the `t·(t−1)·(2t+5)` term.
        let result = mann_kendall(&[1.0, 1.0, 2.0, 3.0, 3.0]);
        assert_eq!(result.s, 8.0);
        close(result.p_value, 0.067_577_148_830_175_5, 1e-9);
    }

    #[test]
    fn theil_sen_fits_a_line() {
        // y = x: slope 1, intercept 1 (positions 0..4, values 1..5).
        assert_eq!(theil_sen_line(&[1.0, 2.0, 3.0, 4.0, 5.0]), Some((1.0, 1.0)));
        // Decreasing by two each step.
        assert_eq!(
            theil_sen_line(&[10.0, 8.0, 6.0, 4.0, 2.0]),
            Some((-2.0, 10.0))
        );
    }

    #[test]
    fn theil_sen_resists_a_single_outlier() {
        // One wild point cannot drag the median-of-slopes off the true unit slope.
        let (slope, _intercept) = theil_sen_line(&[1.0, 2.0, 3.0, 999.0, 5.0]).unwrap();
        assert_eq!(slope, 1.0);
    }

    #[test]
    fn theil_sen_needs_two_points() {
        assert_eq!(theil_sen_line(&[1.0]), None);
    }

    #[test]
    fn theil_sen_fits_the_two_point_minimum() {
        // Two points are the smallest line the fit accepts: slope (5−2)/1 = 3,
        // intercept 2. This pins the `n < 2` lower bound (a slipped `==`/`<=`
        // boundary would reject this valid two-point series).
        assert_eq!(theil_sen_line(&[2.0, 5.0]), Some((3.0, 2.0)));
    }

    #[test]
    fn benjamini_hochberg_keeps_the_significant_prefix() {
        // sorted [0.01,0.02,0.5], q=0.1: ranks 1,2 clear k/m·q; rank 3 fails.
        assert_eq!(
            benjamini_hochberg(&[0.01, 0.02, 0.5], 0.1),
            vec![true, true, false]
        );
    }

    #[test]
    fn benjamini_hochberg_step_up_rejects_through_a_failing_rank() {
        // sorted [0.001,0.03,0.031,0.049], q=0.05: rank 2 (0.03 > 0.025) fails its
        // own threshold, but rank 4 (0.049 ≤ 0.05) passes, so ALL are rejected.
        assert_eq!(
            benjamini_hochberg(&[0.001, 0.03, 0.031, 0.049], 0.05),
            vec![true, true, true, true]
        );
    }

    #[test]
    fn benjamini_hochberg_rejects_none_when_nothing_clears() {
        assert_eq!(benjamini_hochberg(&[0.2], 0.1), vec![false]);
    }

    #[test]
    fn benjamini_hochberg_preserves_input_order() {
        // The single significant value is in the middle of the input.
        assert_eq!(
            benjamini_hochberg(&[0.9, 0.001, 0.8], 0.1),
            vec![false, true, false]
        );
    }

    #[test]
    fn benjamini_hochberg_handles_an_empty_family() {
        assert_eq!(benjamini_hochberg(&[], 0.1), Vec::<bool>::new());
    }

    #[test]
    fn two_sided_p_is_symmetric_and_bounded() {
        close(two_sided_p_from_z(1.96), 0.05, 1e-3);
        close(two_sided_p_from_z(0.0), 1.0, 1e-6);
        close(two_sided_p_from_z(-1.96), 0.05, 1e-3);
    }
}
