//! The Student-t distribution and the p-values derived from it.
//!
//! # Formulations
//!
//! The two-sided tail is the regularized incomplete beta function evaluated at
//! `I_(ν/(ν+t²))(ν/2, 1/2)`, the closed form of the doubled t tail, which keeps
//! its relative accuracy however far out the statistic lies.
//!
//! `I_x(a, b)` itself is the continued fraction of Abramowitz & Stegun,
//! *Handbook of Mathematical Functions* (1964), equations 26.5.8 and 26.5.9:
//!
//! ```text
//! I_x(a, b) = x^a·(1−x)^b / (a·B(a, b)) · 1/(1 + d₁/(1 + d₂/(1 + …)))
//! d_(2m)   =  m·(b − m)·x / ((a + 2m − 1)·(a + 2m))
//! d_(2m+1) = −(a + m)·(a + b + m)·x / ((a + 2m)·(a + 2m + 1))
//! ```
//!
//! The levels therefore come in pairs sharing an index `m`, which is how
//! [`FRACTION_PAIRS`] counts them, and `d₁` is the one level whose index is
//! zero. The fraction is evaluated from its deepest level upwards with a zero
//! tail. It converges only for arguments below `(a + 1)/(a + b + 2)`, so the
//! rest of the interval is reached through the reflection
//! `I_x(a, b) = 1 − I_(1−x)(b, a)`; that switch is the one Numerical Recipes in
//! C, 2nd ed., §6.4 (`betai`) makes, and it is *not* the distribution's mode.
//!
//! The leading factor is formed in logarithms, which needs `ln Γ`. That is the
//! Lanczos approximation (C. Lanczos, *A precision approximation of the gamma
//! function*, SIAM J. Numer. Anal. B 1, 1964) in its `g = 7`, nine-coefficient
//! parameterization:
//!
//! ```text
//! Γ(z + 1) = √(2π)·(z + g + 1/2)^(z + 1/2)·e^(−(z + g + 1/2))·A(z)
//! A(z) = c₀ + Σ_(k=1..8) c_k/(z + k)
//! ```
//!
//! This form holds for `z + 1` at or above [`LN_GAMMA_DOMAIN_MIN`]; smaller
//! arguments would need the reflection formula and never arise, because the
//! shapes evaluated here are `1/2` and `ν/2` at `ν ≥ MIN_DEGREES_OF_FREEDOM`.
//!
//! # Validation
//!
//! Every quantity fixed here — the coefficients, the fraction's depth and the
//! accuracy the entry point claims — is pinned by this module's tests. Their
//! reference values are quoted to the full precision of `f64` from an
//! independent double-precision implementation (`scipy.special` 1.18), so a
//! transposed coefficient or a misaligned offset shows up as a failing test
//! rather than as a plausible wrong answer. Two identities that no reference
//! table can drift out of sync with are swept as well: `I_x(a, b)` rises
//! monotonically across the route switch, and it complements its own reflection
//! to one.
//!
//! Measured against those references:
//!
//! * `ln Γ` is accurate to an absolute `1e-13` on `[1/2, 30]` — an absolute
//!   bound, because the function has zeros at one and two where no relative one
//!   is meaningful — and to a relative `1e-14` from there to `1e9`.
//! * `I_x(a, b)` is accurate to a relative `1e-13` for shapes in `[1/2, 20]`
//!   over the whole unit interval, and for the `(ν/2, 1/2)` shapes of the t
//!   distribution as far as the accuracy stated on
//!   [`student_t_two_sided_p`] holds.
//!
//! The fraction's depth is converged rather than merely plausible: evaluating
//! at twice the depth reproduces the same bits everywhere the fraction is used,
//! and that doubling is the procedure to repeat whenever the depth is
//! questioned. The depth is a parameter of the fraction for exactly that
//! reason.

use crate::{NO_EVIDENCE, clamp_p_value};

/// Fewest degrees of freedom for which a t statistic carries information.
const MIN_DEGREES_OF_FREEDOM: f64 = 1.0;

/// Pairs of levels in the incomplete beta continued fraction.
///
/// Twenty-five pairs already reach the last bit anywhere the fraction is
/// evaluated, for every shape combination this crate puts to it, so this count
/// carries margin while keeping the sweep's cost independent of its arguments.
const FRACTION_PAIRS: u32 = 60;

/// Smallest argument the log-gamma approximation below is valid for.
///
/// See the module's formulation notes for why nothing here reaches it.
const LN_GAMMA_DOMAIN_MIN: f64 = 0.5;

/// `ln(√(2π))`, the constant term of the log-gamma approximation.
const LN_SQRT_TAU: f64 = 0.918_938_533_204_672_7;

/// The shift applied to the argument of the Lanczos approximation.
///
/// Fixed at `g + 1/2` by the parameterization of [`LANCZOS_COEFFICIENTS`], so
/// it cannot be varied independently of them.
const LANCZOS_SHIFT: f64 = 7.5;

/// The leading coefficient `c₀` of the Lanczos series.
///
/// The one term of `A(z)` (see the module's formulation notes) that has no
/// offset to divide by.
const LANCZOS_BASE: f64 = 0.999_999_999_999_809_9;

/// The Lanczos series coefficients `c₁` onwards, in offset order.
///
/// Together with [`LANCZOS_BASE`] these are the published coefficient set of
/// the parameterization described in the module's formulation notes. Their
/// order is part of the formula, not a convention: coefficient `c_k` pairs with
/// the offset `z + k`, and any other pairing approximates a different function.
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
    // The continued fraction converges only for arguments below this limit, so
    // the rest of the interval is evaluated through the reflection
    // `I_x(a, b) = 1 − I_(1−x)(b, a)`, which puts the argument back inside it.
    // This is purely a convergence switch — the switch of Numerical Recipes in
    // C, 2nd ed., §6.4 (`betai`) — and not the distribution's mode, which is
    // `(a − 1)/(a + b − 2)` and lies elsewhere: routing by the mode would send
    // part of the domain down the route that does not converge.
    let direct_route_limit = (a + 1.0) / (a + b + 2.0);
    if converges_directly(x, direct_route_limit) {
        scale * beta_continued_fraction(a, b, x, FRACTION_PAIRS) / a
    } else {
        1.0 - scale * beta_continued_fraction(b, a, 1.0 - x, FRACTION_PAIRS) / b
    }
}

/// Whether the fraction converges for `x` directly, rather than reflected.
//
// Mutation-skipped: the two routes are the same function evaluated two ways and
// agree to the last bits at the boundary — `regularized_incomplete_beta_
// complements_its_own_reflection` pins that — so moving the boundary by one
// value cannot change any result.
#[cfg_attr(test, mutants::skip)]
fn converges_directly(x: f64, direct_route_limit: f64) -> bool {
    x < direct_route_limit
}

/// The continued fraction of the incomplete beta function.
///
/// Evaluates `pairs` pairs of levels, plus the unpaired topmost one, of the
/// fraction described in the module's formulation notes. [`FRACTION_PAIRS`] is
/// the count the crate is validated at; the count is a parameter so that the
/// validation can be reproduced at another one.
fn beta_continued_fraction(a: f64, b: f64, x: f64, pairs: u32) -> f64 {
    // Evaluated from the deepest level upwards, each level shrinking towards
    // zero, so the innermost levels contribute nothing but cost.
    let mut fraction = 0.0_f64;
    let mut index = f64::from(pairs);
    for _ in 0..pairs {
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
/// Uses the Lanczos approximation described in the module's formulation notes,
/// which is accurate to roughly fifteen digits for arguments of at least
/// [`LN_GAMMA_DOMAIN_MIN`].
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

    /// Shape parameters the identity and convergence sweeps range over.
    ///
    /// Spans the lopsided halves the t distribution generates, the balanced
    /// combinations a table of reference values alone would not exercise, and
    /// both sides of the shapes at which the route switch crosses the middle of
    /// the unit interval.
    const SHAPES: [f64; 6] = [0.5, 1.0, 2.0, 3.0, 5.0, 20.0];

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
    fn regularized_incomplete_beta_matches_reference_values_across_the_route_switch() {
        // Shapes of three and seven put the route switch at 1/3 and the
        // distribution's mode at 1/4, so the arguments between them are
        // evaluated by the direct route although they lie past the mode. Every
        // value here is a reference value, not a self-consistency check, so
        // routing them the other way would be visible.
        let switch = (3.0 + 1.0) / (3.0 + 7.0 + 2.0);
        let cases = [
            (0.2, 2.618_024_960_000_001_6e-1),
            (0.25, 3.993_225_097_656_25e-1),
            (0.3, 5.371_688_339_999_998e-1),
            (switch, 6.228_217_243_306_404e-1),
            (0.35, 6.627_267_211_249_999e-1),
            (0.5, 9.101_562_5e-1),
            (0.9, 9.999_970_02e-1),
        ];
        for (x, expected) in cases {
            close_relative(regularized_incomplete_beta(3.0, 7.0, x), expected, 1e-12);
        }
    }

    #[test]
    fn regularized_incomplete_beta_matches_reference_values_for_lopsided_shapes() {
        // The shapes the t distribution generates are as lopsided as they come:
        // one of the two is always a half.
        let cases = [
            (0.5, 0.5, 0.1, 2.048_327_646_991_334_5e-1),
            (0.5, 0.5, 0.9, 7.951_672_353_008_665e-1),
            (20.0, 0.5, 0.5, 1.653_098_093_639_878_8e-7),
            (0.5, 20.0, 0.5, 9.999_998_346_901_908e-1),
            (7.0, 3.0, 0.75, 6.006_774_902_343_75e-1),
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
    #[cfg_attr(miri, ignore)] // Sweeps thousands of points, which Miri is too slow to evaluate.
    fn regularized_incomplete_beta_complements_its_own_reflection() {
        // I_x(a, b) + I_(1−x)(b, a) = 1, an identity that holds on both sides of
        // the route switch and across it, and that no reference table can drift
        // out of sync with. Stated as a sum rather than as a difference, the
        // identity stays informative even where one of the two terms is far
        // smaller than the rounding error of the other.
        for a in SHAPES {
            for b in SHAPES {
                for step in 1_i32..40_i32 {
                    let x = f64::from(step) / 40.0;
                    let direct = regularized_incomplete_beta(a, b, x);
                    let reflected = regularized_incomplete_beta(b, a, 1.0 - x);
                    close(direct + reflected, 1.0, 1e-13);
                }
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Sweeps thousands of points, which Miri is too slow to evaluate.
    fn regularized_incomplete_beta_rises_monotonically() {
        // A distribution function cannot fall, least of all where the two
        // evaluation routes meet.
        for a in SHAPES {
            for b in SHAPES {
                let mut previous = 0.0_f64;
                for step in 0_i32..=200_i32 {
                    let x = f64::from(step) / 200.0;
                    let value = regularized_incomplete_beta(a, b, x);
                    assert!(
                        value >= previous,
                        "I_{x}({a}, {b}) = {value} fell below {previous}"
                    );
                    previous = value;
                }
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Sweeps thousands of points, which Miri is too slow to evaluate.
    fn beta_continued_fraction_is_converged_at_its_depth() {
        // The procedure that establishes the depth: doubling it must not move a
        // single bit anywhere the fraction is evaluated, which is only ever
        // below the route switch.
        for a in SHAPES {
            for b in SHAPES {
                let limit = (a + 1.0) / (a + b + 2.0);
                for step in 1_i32..20_i32 {
                    let x = limit * f64::from(step) / 20.0;
                    assert_eq!(
                        beta_continued_fraction(a, b, x, FRACTION_PAIRS),
                        beta_continued_fraction(a, b, x, FRACTION_PAIRS * 2),
                        "the fraction moved when its depth doubled at I_{x}({a}, {b})"
                    );
                }
            }
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
        // The t distribution converges to the standard normal at a rate of one over its
        // degrees of freedom, so every tenfold increase must close the gap by about a
        // decade. Asserting the rate rather than one bound separates real convergence
        // from a formula that merely happens to land nearby.
        let normal = two_sided_p_from_z(1.96);
        let gap = |degrees_of_freedom: f64| {
            ((student_t_two_sided_p(1.96, degrees_of_freedom) - normal) / normal).abs()
        };

        let mut coarser = gap(1e2);
        for exponent in 3_i32..=6_i32 {
            let finer = gap(10.0_f64.powi(exponent));
            let shrinkage = coarser / finer;
            assert!(
                (9.0..11.0).contains(&shrinkage),
                "a decade of degrees of freedom closed the gap {shrinkage}-fold, not tenfold"
            );
            coarser = finer;
        }

        close_relative(
            student_t_two_sided_p(3.0, 1e6),
            two_sided_p_from_z(3.0),
            1e-4,
        );

        // At the extreme below, the two log-gamma terms whose difference the density
        // needs both run to some ten billion, so subtracting them spends most of the
        // mantissa and the agreement stops improving — it worsens. The detector compares
        // a tip against a base window of at most a few dozen points, so no such degrees
        // of freedom arise in use; this bound records where the arithmetic gives out,
        // and is deliberately loose enough to hold under a less exact library routine.
        close_relative(
            student_t_two_sided_p(1.96, 1e9),
            two_sided_p_from_z(1.96),
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
