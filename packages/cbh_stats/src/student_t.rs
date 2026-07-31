//! The Student-t distribution and the p-values derived from it.

use crate::{NO_EVIDENCE, clamp_p_value};

/// Fewest degrees of freedom for which a t statistic carries information.
const MIN_DEGREES_OF_FREEDOM: f64 = 1.0;

/// Pairs of levels in the incomplete beta continued fraction.
///
/// The fraction is evaluated from its deepest level upwards with a zero tail.
/// Its levels come in pairs sharing an index, and this many pairs converges the
/// result to the last bit across the whole range of shape parameters this crate
/// puts to it.
const FRACTION_PAIRS: u32 = 60;

/// Smallest argument the log-gamma approximation below is valid for.
const LN_GAMMA_DOMAIN_MIN: f64 = 0.5;

/// `ln(√(2π))`, the constant term of the log-gamma approximation.
const LN_SQRT_TAU: f64 = 0.918_938_533_204_672_7;

/// The shift applied to the argument of the Lanczos approximation, fixed by the
/// choice of coefficients below.
const LANCZOS_SHIFT: f64 = 7.5;

/// Leading term of the Lanczos series.
const LANCZOS_BASE: f64 = 0.999_999_999_999_809_9;

/// Coefficients of the Lanczos series, divided by successive integer offsets.
const LANCZOS_COEFFICIENTS: [f64; 8] = [
    676.520_368_121_885_1,
    -1_259.139_216_722_402_8,
    771.323_428_777_653_1,
    -176.615_029_162_140_6,
    12.507_343_278_686_905,
    -0.138_571_095_265_720_12,
    9.984_369_578_019_572e-6,
    1.505_632_735_149_311_6e-7,
];

/// The two-sided p-value for a Student-t statistic.
///
/// Answers "how likely is a t statistic at least this extreme, in either
/// direction, when the null hypothesis holds?" for `degrees_of_freedom` degrees
/// of freedom, which need not be a whole number. The p-value is continuous in
/// both arguments, so it composes with a false-discovery-rate correction
/// instead of merely deciding against a fixed significance level.
///
/// The result lies in `(0.0, 1.0]`: it is symmetric in the sign of `t`, falls
/// as `|t|` grows, and equals `1.0` at `t = 0`.
///
/// A degenerate test reports `1.0`, the p-value of a test that found no
/// evidence, so no degenerate input can be turned into a significant result.
/// Fewer than one degree of freedom describes no usable distribution, and a
/// statistic that is not finite carries no information about how extreme it is
/// — an infinite statistic marks a comparison against no scatter at all, not an
/// infinitely strong finding.
///
/// # Accuracy
///
/// The relative error stays below `1e-10` for up to a thousand degrees of
/// freedom, and holds across the whole reportable range of p-values rather than
/// decaying in the tail: a p-value of `1e-12` carries as many correct digits as
/// one of `0.5`. Beyond a thousand degrees of freedom the error grows slowly,
/// reaching roughly `1e-5` at a billion — by which point the distribution is
/// itself within `1e-9` of the standard normal it converges to, so the error is
/// far smaller than the difference between the two distributions.
///
/// The result is never exactly zero. Statistics extreme enough to underflow
/// report a small positive floor instead, which conveys overwhelming
/// significance and nothing more precise.
#[must_use]
pub fn student_t_two_sided_p(t: f64, degrees_of_freedom: f64) -> f64 {
    if !t.is_finite() {
        return NO_EVIDENCE;
    }
    if !degrees_of_freedom.is_finite() {
        return NO_EVIDENCE;
    }
    if degrees_of_freedom < MIN_DEGREES_OF_FREEDOM {
        return NO_EVIDENCE;
    }
    // The two tails of the t distribution beyond ±t, expressed as an incomplete
    // beta so that no cancellation occurs however far out the statistic lies.
    let squared = t * t;
    clamp_p_value(regularized_incomplete_beta(
        degrees_of_freedom * 0.5,
        0.5,
        degrees_of_freedom / (degrees_of_freedom + squared),
    ))
}

/// The regularized incomplete beta function, `I_x(a, b)`.
///
/// Defined for positive shape parameters and for `x` in `[0, 1]`, where it
/// rises from zero to one.
fn regularized_incomplete_beta(a: f64, b: f64, x: f64) -> f64 {
    // The factor common to both tails, formed in logarithms because its parts
    // overflow long before their product does.
    let scale = (ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (-x).ln_1p()).exp();
    // The continued fraction converges quickly only on the near side of the
    // distribution's mode, so the far side is evaluated through the reflection
    // `I_x(a, b) = 1 − I_(1−x)(b, a)`.
    let near_side = (a + 1.0) / (a + b + 2.0);
    if converges_directly(x, near_side) {
        scale * beta_continued_fraction(a, b, x) / a
    } else {
        1.0 - scale * beta_continued_fraction(b, a, 1.0 - x) / b
    }
}

/// Whether `x` lies on the side of the mode where the fraction converges fast.
//
// Mutation-skipped: the two routes are the same function evaluated two ways and
// agree to the last bits at the boundary — `regularized_incomplete_beta_is_
// symmetric_under_reflection` pins that — so moving the boundary by one value
// cannot change any result.
#[cfg_attr(test, mutants::skip)]
fn converges_directly(x: f64, near_side: f64) -> bool {
    x < near_side
}

/// The continued fraction of the incomplete beta function.
///
/// Evaluates `1/(1 + d₁/(1 + d₂/(1 + …)))`, whose partial numerators alternate
/// between two families indexed by their depth.
fn beta_continued_fraction(a: f64, b: f64, x: f64) -> f64 {
    // Evaluated from the deepest level upwards, each level shrinking towards
    // zero, so the innermost levels contribute nothing but cost.
    let mut fraction = 0.0_f64;
    let mut index = f64::from(FRACTION_PAIRS);
    for _ in 0..FRACTION_PAIRS {
        let doubled = 2.0 * index;
        let deeper = -((a + index) * (a + b + index) * x) / ((a + doubled) * (a + doubled + 1.0));
        fraction = deeper / (1.0 + fraction);
        let shallower = index * (b - index) * x / ((a + doubled - 1.0) * (a + doubled));
        fraction = shallower / (1.0 + fraction);
        index -= 1.0;
    }
    // The topmost level belongs to neither family, being the only one whose
    // index is zero.
    let top = -((a + b) * x) / (a + 1.0);
    1.0 / (1.0 + top / (1.0 + fraction))
}

/// The natural logarithm of the gamma function, `ln Γ(x)`.
///
/// Uses the Lanczos approximation, which is accurate to roughly fifteen digits
/// for arguments of at least [`LN_GAMMA_DOMAIN_MIN`]. Smaller arguments would
/// need the reflection formula and never arise here.
fn ln_gamma(x: f64) -> f64 {
    debug_assert!(
        x >= LN_GAMMA_DOMAIN_MIN,
        "the log-gamma approximation is only valid for arguments of at least {LN_GAMMA_DOMAIN_MIN}"
    );
    let shifted = x - 1.0;
    let mut series = LANCZOS_BASE;
    let mut offset = shifted;
    for coefficient in LANCZOS_COEFFICIENTS {
        offset += 1.0;
        series += coefficient / offset;
    }
    let scale = shifted + LANCZOS_SHIFT;
    LN_SQRT_TAU + (shifted + 0.5) * scale.ln() - scale + series.ln()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "primitive outputs are compared against hand-computed exact values"
    )]

    use std::f64::consts;

    use super::*;
    use crate::test_util::{close, close_relative};
    use crate::{MIN_P_VALUE, two_sided_p_from_z};

    /// The two-sided p-value of a Cauchy-distributed statistic, which is the
    /// t distribution with one degree of freedom, in closed form.
    fn cauchy_two_sided_p(t: f64) -> f64 {
        1.0 - consts::FRAC_2_PI * t.abs().atan()
    }

    #[test]
    fn ln_gamma_matches_reference_values() {
        close_relative(ln_gamma(0.5), 5.723_649_429_247_004e-1, 1e-13);
        // Γ(1) = Γ(2) = 1, so the logarithm vanishes.
        close(ln_gamma(1.0), 0.0, 1e-14);
        close_relative(ln_gamma(1.5), -1.207_822_376_352_454_3e-1, 1e-13);
        close(ln_gamma(2.0), 0.0, 1e-14);
        close_relative(ln_gamma(2.5), 2.846_828_704_729_196e-1, 1e-13);
        close_relative(ln_gamma(5.0), 3.178_053_830_347_945, 1e-13);
        close_relative(ln_gamma(10.5), 1.394_062_521_940_376_3e1, 1e-13);
        close_relative(ln_gamma(100.0), 3.591_342_053_695_754e2, 1e-13);
        close_relative(ln_gamma(1000.0), 5.905_220_423_209_181e3, 1e-13);
        close_relative(ln_gamma(1e6), 1.281_550_456_914_761_1e7, 1e-13);
    }

    #[test]
    fn ln_gamma_reproduces_factorials() {
        // Γ(n + 1) = n!, an exact identity the approximation must respect.
        let mut factorial = 1.0_f64;
        for n in 1_u32..=12_u32 {
            factorial *= f64::from(n);
            close(ln_gamma(f64::from(n) + 1.0), factorial.ln(), 1e-12);
        }
    }

    #[test]
    fn regularized_incomplete_beta_matches_reference_values() {
        let cases = [
            (5.0, 0.5, 0.1, 2.570_589_699_229_372_3e-6),
            (5.0, 0.5, 0.9, 3.166_429_150_200_122e-1),
            (0.5, 0.5, 0.5, 5.000_000_000_000_001e-1),
            (2.0, 3.0, 0.4, 5.247_999_999_999_999e-1),
            (1.0, 1.0, 0.25, 0.25),
            (0.5, 5.0, 0.3, 9.347_377_538_310_915e-1),
        ];
        for (a, b, x, expected) in cases {
            close_relative(regularized_incomplete_beta(a, b, x), expected, 1e-12);
        }
    }

    #[test]
    fn regularized_incomplete_beta_keeps_its_accuracy_in_the_tail() {
        // Values this small are the reason the function is not computed as a
        // difference of two nearly equal numbers.
        let cases = [
            (5.0, 0.5, 1e-3, 2.461_963_550_285_712e-16),
            (20.0, 0.5, 1e-6, 1.253_707_473_199_495_8e-121),
            // A shape combination whose reflected route collapses into the
            // rounding error of one, so only the direct route can express it.
            (5.0, 20.0, 1e-3, 4.183_618_590_979_37e-11),
        ];
        for (a, b, x, expected) in cases {
            close_relative(regularized_incomplete_beta(a, b, x), expected, 1e-12);
        }
    }

    #[test]
    fn regularized_incomplete_beta_spans_the_unit_interval() {
        assert_eq!(regularized_incomplete_beta(5.0, 0.5, 0.0), 0.0);
        assert_eq!(regularized_incomplete_beta(5.0, 0.5, 1.0), 1.0);
        assert_eq!(regularized_incomplete_beta(0.5, 0.5, 0.0), 0.0);
        assert_eq!(regularized_incomplete_beta(0.5, 0.5, 1.0), 1.0);
    }

    #[test]
    fn regularized_incomplete_beta_is_symmetric_under_reflection() {
        // I_x(a, b) = 1 − I_(1−x)(b, a), an identity that holds on both sides of
        // the branch boundary and across it.
        for step in 1_i32..20_i32 {
            let x = f64::from(step) / 20.0;
            let direct = regularized_incomplete_beta(3.0, 7.0, x);
            let reflected = 1.0 - regularized_incomplete_beta(7.0, 3.0, 1.0 - x);
            close_relative(direct, reflected, 1e-12);
        }
    }

    #[test]
    fn student_t_two_sided_p_matches_published_critical_values() {
        // Critical values from published t tables, each the statistic at which
        // the two-sided p-value reaches the stated significance level.
        let cases = [
            (2.262, 9.0, 5.001_284_550_245_463e-2),
            (3.25, 9.0, 9.997_369_084_021_572e-3),
            (2.228, 10.0, 5.001_177_181_711_132e-2),
            (3.169, 10.0, 1.000_463_336_438_485_6e-2),
            (2.131, 15.0, 5.004_250_477_424_244e-2),
            (2.947, 15.0, 9.994_167_423_479_583e-3),
            (4.073, 15.0, 9.995_236_514_183_938e-4),
            (2.086, 20.0, 4.999_635_445_744_019e-2),
            (2.042, 30.0, 5.002_867_065_619_790_6e-2),
            (2.021, 40.0, 5.000_814_500_076_529e-2),
            (4.781, 40.0, 2.372_863_966_687_155e-5),
        ];
        for (t, degrees_of_freedom, expected) in cases {
            close_relative(
                student_t_two_sided_p(t, degrees_of_freedom),
                expected,
                1e-10,
            );
        }
    }

    #[test]
    fn student_t_two_sided_p_matches_reference_values_in_the_tail() {
        // Far past any tabulated critical value, where a false-discovery-rate
        // filter still needs the p-values to be ordered by strength.
        let cases = [
            (6.0, 10.0, 1.321_088_603_547_855_7e-4),
            (8.0, 12.0, 3.759_898_224_750_258e-6),
            (10.0, 10.0, 1.589_553_175_596_412_5e-6),
        ];
        for (t, degrees_of_freedom, expected) in cases {
            close_relative(
                student_t_two_sided_p(t, degrees_of_freedom),
                expected,
                1e-10,
            );
        }
    }

    #[test]
    fn student_t_two_sided_p_matches_reference_values_near_the_centre() {
        let cases = [
            (1.0, 10.0, 3.408_931_323_020_6e-1),
            (0.5, 25.0, 6.214_477_851_902_287e-1),
            // Many degrees of freedom put the statistic deep on the far side of
            // the mode, where only the reflected route stays converged.
            (0.5, 100.0, 6.181_735_658_308_866e-1),
        ];
        for (t, degrees_of_freedom, expected) in cases {
            close_relative(
                student_t_two_sided_p(t, degrees_of_freedom),
                expected,
                1e-10,
            );
        }
    }

    #[test]
    fn student_t_two_sided_p_accepts_fractional_degrees_of_freedom() {
        // Welch's t-test produces fractional degrees of freedom.
        close_relative(
            student_t_two_sided_p(2.0, 3.7),
            1.218_172_019_052_339_8e-1,
            1e-10,
        );
        close_relative(
            student_t_two_sided_p(3.0, 2.0),
            9.546_596_626_670_914e-2,
            1e-10,
        );
    }

    #[test]
    fn student_t_two_sided_p_matches_the_cauchy_closed_form() {
        // One degree of freedom is the Cauchy distribution, whose two-sided
        // p-value has an exact elementary form.
        for step in 1_i32..=50_i32 {
            let t = f64::from(step) / 10.0;
            close_relative(student_t_two_sided_p(t, 1.0), cauchy_two_sided_p(t), 1e-12);
        }
    }

    #[test]
    fn student_t_two_sided_p_approaches_the_normal_limit() {
        // The t distribution converges to the standard normal as its degrees of
        // freedom grow.
        close_relative(
            student_t_two_sided_p(1.96, 1e9),
            two_sided_p_from_z(1.96),
            1e-5,
        );
        close_relative(
            student_t_two_sided_p(1.96, 1e6),
            two_sided_p_from_z(1.96),
            1e-4,
        );
        close_relative(
            student_t_two_sided_p(3.0, 1e6),
            two_sided_p_from_z(3.0),
            1e-4,
        );
        // Ten degrees of freedom are nowhere near the limit, which is the whole
        // reason a small-sample test needs this distribution at all.
        assert!(student_t_two_sided_p(3.0, 10.0) > 4.0 * two_sided_p_from_z(3.0));
    }

    #[test]
    fn student_t_two_sided_p_is_certain_at_zero() {
        assert_eq!(student_t_two_sided_p(0.0, 1.0), NO_EVIDENCE);
        assert_eq!(student_t_two_sided_p(0.0, 10.0), NO_EVIDENCE);
        assert_eq!(student_t_two_sided_p(0.0, 1e6), NO_EVIDENCE);
    }

    #[test]
    fn student_t_two_sided_p_is_symmetric() {
        // The two-sided p-value depends only on the magnitude of the statistic.
        let magnitudes = [(2.228, 10.0), (8.0, 12.0)];
        for (t, degrees_of_freedom) in magnitudes {
            close_relative(
                student_t_two_sided_p(t, degrees_of_freedom),
                student_t_two_sided_p(-t, degrees_of_freedom),
                1e-12,
            );
        }
    }

    #[test]
    fn student_t_two_sided_p_falls_monotonically_with_magnitude() {
        let mut previous = NO_EVIDENCE;
        for step in 1_i32..=400_i32 {
            let t = f64::from(step) / 4.0;
            let p = student_t_two_sided_p(t, 12.0);
            assert!(p <= previous, "p({t}) = {p} rose above {previous}");
            // The reportable range is half-open: a p-value is a probability that
            // never reaches zero, however extreme the statistic gets.
            assert!(p > 0.0 && p <= NO_EVIDENCE, "p({t}) = {p} left (0, 1]");
            previous = p;
        }
        // A statistic of a hundred with twelve degrees of freedom is already
        // past the floor, despite the distribution's fat tails.
        assert_eq!(previous, MIN_P_VALUE);
    }

    #[test]
    fn student_t_two_sided_p_never_vanishes() {
        assert!(student_t_two_sided_p(1e6, 10.0) > 0.0);
        assert_eq!(student_t_two_sided_p(1e6, 10.0), MIN_P_VALUE);
    }

    #[test]
    fn student_t_two_sided_p_reports_no_evidence_for_a_degenerate_test() {
        assert_eq!(student_t_two_sided_p(2.0, 0.5), NO_EVIDENCE);
        assert_eq!(student_t_two_sided_p(2.0, 0.0), NO_EVIDENCE);
        assert_eq!(student_t_two_sided_p(2.0, -3.0), NO_EVIDENCE);
        assert_eq!(student_t_two_sided_p(2.0, f64::NAN), NO_EVIDENCE);
        assert_eq!(student_t_two_sided_p(2.0, f64::INFINITY), NO_EVIDENCE);
        // A statistic that is not finite means the test had no scatter to
        // measure against, which is a degeneracy rather than certainty.
        assert_eq!(student_t_two_sided_p(f64::NAN, 10.0), NO_EVIDENCE);
        assert_eq!(student_t_two_sided_p(f64::INFINITY, 10.0), NO_EVIDENCE);
        assert_eq!(student_t_two_sided_p(f64::NEG_INFINITY, 10.0), NO_EVIDENCE);
    }

    #[test]
    fn student_t_two_sided_p_admits_the_fewest_degrees_of_freedom() {
        // The boundary of the accepted range is inside it, not outside.
        assert!(student_t_two_sided_p(2.0, MIN_DEGREES_OF_FREEDOM) < NO_EVIDENCE);
    }
}
