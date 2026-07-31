//! The standard normal distribution and the p-values derived from it.

use std::f64::consts;

use crate::clamp_p_value;

/// Magnitude below which `erf` is evaluated by its series rather than `erfc` by
/// its continued fraction.
///
/// The series stays accurate everywhere but needs ever more terms as the
/// argument grows, while the continued fraction converges ever faster; they
/// meet comfortably here, where both are converged.
const SERIES_LIMIT: f64 = 1.5;

/// Terms of the `erf` series evaluated below [`SERIES_LIMIT`].
///
/// Successive terms shrink by more than half once the running denominator
/// passes `2·SERIES_LIMIT²`, so this many terms drives the remainder far below
/// the rounding error of the sum.
const SERIES_TERMS: u32 = 28;

/// Levels of the `erfc` continued fraction evaluated above [`SERIES_LIMIT`].
///
/// The fraction is evaluated from its deepest level upwards with a zero tail,
/// and this depth converges the result to the last bit for every argument that
/// does not underflow.
const FRACTION_DEPTH: u32 = 120;

/// Difference between successive partial numerators of the fraction, which run
/// `1/2, 1, 3/2, 2, …`.
const NUMERATOR_STEP: f64 = 0.5;

/// `1/√π`, the scale factor of the `erfc` continued fraction.
const FRAC_1_SQRT_PI: f64 = consts::FRAC_2_SQRT_PI * 0.5;

/// The standard normal cumulative distribution function, `Φ(z)`.
///
/// Both tails carry the same relative accuracy — roughly thirteen correct
/// digits — so `Φ(-9)` is as trustworthy as `Φ(0)`. The value saturates to `0`
/// or `1` only once the true probability leaves the range of `f64`.
pub(crate) fn normal_cdf(z: f64) -> f64 {
    0.5 * erfc(-z / consts::SQRT_2)
}

/// The two-sided p-value for a standard-normal test statistic `z`.
///
/// # Accuracy
///
/// The relative error stays below `1e-12` over the whole reportable range, so a
/// p-value of `1e-12` carries as many correct digits as one of `0.5`. The
/// result is floored at `MIN_P_VALUE`: statistics beyond `|z| ≈ 8` all report
/// that floor, which means "overwhelming significance" and nothing more
/// precise.
pub(crate) fn two_sided_p_from_z(z: f64) -> f64 {
    // 2·Φ(−|z|) is the doubled tail beyond `z`, evaluated on the side where the
    // complementary error function computes it directly instead of as the
    // difference of two nearly equal numbers.
    clamp_p_value(2.0 * normal_cdf(-z.abs()))
}

/// The complementary error function, `erfc(x) = 1 − erf(x)`.
fn erfc(x: f64) -> f64 {
    if prefers_series(x.abs()) {
        // `erf` is odd and small here, so subtracting it from one cannot cancel.
        return 1.0 - erf_series(x);
    }
    if x.is_sign_positive() {
        tail_erfc(x)
    } else {
        // The series would overflow long before `erfc` reaches its limit of two,
        // so the reflection `erfc(−x) = 2 − erfc(x)` covers the negative tail.
        2.0 - tail_erfc(-x)
    }
}

/// Whether a magnitude is small enough for the series to be the better route.
//
// Mutation-skipped: the two evaluations agree to the last bits on both sides of
// the crossover — `erf_series_and_tail_erfc_agree_at_the_crossover` pins that —
// so moving the boundary by one value cannot change any result.
#[cfg_attr(test, mutants::skip)]
fn prefers_series(magnitude: f64) -> bool {
    magnitude < SERIES_LIMIT
}

/// The error function `erf(x)` for arguments below [`SERIES_LIMIT`].
///
/// Uses the form `erf(x) = (2/√π)·e^(−x²)·Σ 2ⁿ·x^(2n+1)/(2n+1)!!`, whose terms
/// are all of one sign, so the sum accumulates no cancellation error.
fn erf_series(x: f64) -> f64 {
    let x_squared = x * x;
    let mut term = x;
    let mut total = x;
    let mut denominator = 1.0_f64;
    for _ in 0..SERIES_TERMS {
        denominator += 2.0;
        term *= 2.0 * x_squared / denominator;
        total += term;
    }
    consts::FRAC_2_SQRT_PI * (-x_squared).exp() * total
}

/// The complementary error function for arguments at or above [`SERIES_LIMIT`].
///
/// Uses the continued fraction
/// `erfc(x) = (e^(−x²)/√π)·1/(x + (1/2)/(x + 1/(x + (3/2)/(x + …))))`, which is
/// a product of the vanishing exponential and a bounded factor and therefore
/// keeps its relative accuracy however small the result becomes.
fn tail_erfc(x: f64) -> f64 {
    // Evaluated from the deepest level upwards: every partial denominator is
    // `x + (a positive value)`, so no level can approach zero.
    let levels_below_top = FRACTION_DEPTH.saturating_sub(1);
    let mut numerator = f64::from(levels_below_top) * NUMERATOR_STEP;
    let mut fraction = 0.0_f64;
    for _ in 0..levels_below_top {
        fraction = numerator / (x + fraction);
        numerator -= NUMERATOR_STEP;
    }
    // The outermost level carries no partial numerator of its own: the fraction
    // closes as `1/(x + …)`.
    FRAC_1_SQRT_PI * (-x * x).exp() / (x + fraction)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "primitive outputs are compared against hand-computed exact values"
    )]

    use super::*;
    use crate::MIN_P_VALUE;
    use crate::test_util::{close, close_relative};

    #[test]
    fn erfc_matches_reference_values() {
        // Reference values of the complementary error function, spanning the
        // series branch, the crossover, and deep into the tail where an
        // absolute-error approximation would have nothing left but noise.
        close_relative(erfc(0.25), 7.236_736_098_317_631e-1, 1e-12);
        close_relative(erfc(1.0), 1.572_992_070_502_851e-1, 1e-12);
        close_relative(erfc(SERIES_LIMIT), 3.389_485_352_468_927_4e-2, 1e-12);
        close_relative(erfc(2.0), 4.677_734_981_047_265e-3, 1e-12);
        close_relative(erfc(3.0), 2.209_049_699_858_544e-5, 1e-12);
        close_relative(erfc(5.0), 1.537_459_794_428_035_1e-12, 1e-12);
        close_relative(erfc(10.0), 2.088_487_583_762_545e-45, 1e-12);
        close_relative(erfc(20.0), 5.395_865_611_607_901e-176, 1e-12);
    }

    #[test]
    fn erfc_matches_reference_values_for_negative_arguments() {
        close_relative(erfc(-0.5), 1.520_499_877_813_046_5, 1e-12);
        close_relative(erfc(-SERIES_LIMIT), 1.966_105_146_475_310_8, 1e-12);
        close_relative(erfc(-3.0), 1.999_977_909_503_001_2, 1e-12);
        // Far into the negative tail the complement saturates at exactly two.
        // The series overflows for arguments this large, so reaching the value
        // at all pins the reflection branch.
        assert_eq!(erfc(-30.0), 2.0);
        assert_eq!(erfc(0.0), 1.0);
    }

    #[test]
    fn erf_series_and_tail_erfc_agree_at_the_crossover() {
        // The boundary between the two evaluations sits inside a band where
        // both are fully converged, which is what makes its exact position
        // immaterial.
        for offset in -4_i32..=3_i32 {
            let x = SERIES_LIMIT + f64::from(offset) / 10.0;
            close_relative(1.0 - erf_series(x), tail_erfc(x), 1e-13);
        }
    }

    #[test]
    fn erfc_falls_monotonically() {
        let mut previous = f64::INFINITY;
        // Steps of a third of a unit cross the crossover at 1.5 exactly. The
        // range stops where the true value saturates against the limits of the
        // format: below two on the left, at zero on the right.
        for step in -15_i32..=60_i32 {
            let x = f64::from(step) / 3.0;
            let value = erfc(x);
            assert!(value < previous, "erfc({x}) = {value} did not fall");
            previous = value;
        }
    }

    #[test]
    fn normal_cdf_matches_reference_values() {
        assert_eq!(normal_cdf(0.0), 0.5);
        close_relative(normal_cdf(1.0), 8.413_447_460_685_429e-1, 1e-12);
        close_relative(normal_cdf(1.96), 9.750_021_048_517_795e-1, 1e-12);
        close_relative(normal_cdf(-1.96), 2.499_789_514_822_043_5e-2, 1e-12);
        close_relative(normal_cdf(-3.0), 1.349_898_031_630_093_3e-3, 1e-12);
        // The far tail keeps its relative accuracy rather than collapsing into
        // the rounding error of a subtraction from one.
        close_relative(normal_cdf(-5.0), 2.866_515_718_791_933e-7, 1e-12);
    }

    #[test]
    fn normal_cdf_is_symmetric() {
        close(normal_cdf(-1.0), 1.0 - normal_cdf(1.0), 1e-15);
        close(normal_cdf(-2.5), 1.0 - normal_cdf(2.5), 1e-15);
    }

    #[test]
    fn normal_cdf_saturates_far_from_the_mean() {
        assert_eq!(normal_cdf(50.0), 1.0);
        assert_eq!(normal_cdf(-50.0), 0.0);
    }

    #[test]
    fn two_sided_p_from_z_matches_reference_values() {
        assert_eq!(two_sided_p_from_z(0.0), 1.0);
        close_relative(two_sided_p_from_z(1.0), 3.173_105_078_629_141_5e-1, 1e-12);
        close_relative(two_sided_p_from_z(1.96), 4.999_579_029_644_087e-2, 1e-12);
        close_relative(two_sided_p_from_z(3.0), 2.699_796_063_260_191_8e-3, 1e-12);
        close_relative(two_sided_p_from_z(5.0), 5.733_031_437_583_891e-7, 1e-12);
        // Beyond here an approximation with a 1.5e-7 absolute error has nothing
        // left to say, yet these values are still good to twelve digits.
        close_relative(two_sided_p_from_z(6.0), 1.973_175_290_075_403_2e-9, 1e-12);
        close_relative(two_sided_p_from_z(8.0), 1.244_192_114_854_363_9e-15, 1e-12);
    }

    #[test]
    fn two_sided_p_from_z_is_symmetric() {
        // The two-sided p-value depends only on the magnitude of the statistic.
        close_relative(two_sided_p_from_z(1.96), two_sided_p_from_z(-1.96), 1e-12);
        close_relative(two_sided_p_from_z(6.0), two_sided_p_from_z(-6.0), 1e-12);
    }

    #[test]
    fn two_sided_p_from_z_never_vanishes() {
        // The statistic that used to produce an exactly zero p-value, which
        // would clear any false-discovery-rate threshold unconditionally.
        assert!(two_sided_p_from_z(9.0) > 0.0);
        assert_eq!(two_sided_p_from_z(50.0), MIN_P_VALUE);
        assert_eq!(two_sided_p_from_z(-50.0), MIN_P_VALUE);
        assert_eq!(two_sided_p_from_z(f64::INFINITY), MIN_P_VALUE);
    }

    #[test]
    fn two_sided_p_from_z_falls_monotonically_with_magnitude() {
        let mut previous = 1.0_f64;
        for step in 1_i32..=200_i32 {
            let z = f64::from(step) / 4.0;
            let p = two_sided_p_from_z(z);
            assert!(p <= previous, "p({z}) = {p} rose above {previous}");
            previous = p;
        }
        assert_eq!(previous, MIN_P_VALUE);
    }
}
