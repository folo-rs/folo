//! The standard-normal p-value, reproduced bit-for-bit from `cbh_stats`.
//!
//! The calibration table records, per rank ordering, the Mann–Whitney p-value the production
//! detector would compute. That p-value ends in [`two_sided_p_from_z`], so the table can only be
//! faithful if this routine returns exactly what `cbh_stats::normal` returns. The algorithm and
//! its constants are therefore copied verbatim from `packages/cbh_stats/src/normal.rs` (Abramowitz
//! & Stegun 7.1.6 below the crossover, 7.1.14 above it) rather than re-derived, and the
//! `matches_production` test pins that the two agree.
//!
//! Duplicating it here — instead of taking a build-time dependency on `cbh_stats` — is what lets
//! the `write` binary regenerate the committed table when `cbh_stats` itself does not yet compile
//! (its `selection/table.rs` is the file this generator emits).

use std::f64::consts;

/// The smallest p-value the production tests report; mirrors `cbh_stats::p_value::MIN_P_VALUE`.
///
/// Value chosen to match the production floor so the table's p-values clamp identically.
pub(crate) const MIN_P_VALUE: f64 = 1e-15;

/// See `cbh_stats::normal::SERIES_LIMIT`. Value copied to match production.
const SERIES_LIMIT: f64 = 1.5;

/// See `cbh_stats::normal::SERIES_TERMS`. Value copied to match production.
const SERIES_TERMS: u32 = 28;

/// See `cbh_stats::normal::FRACTION_DEPTH`. Value copied to match production.
const FRACTION_DEPTH: u32 = 120;

/// See `cbh_stats::normal::NUMERATOR_STEP`. Value copied to match production.
const NUMERATOR_STEP: f64 = 0.5;

/// `1/√π`, the scale factor of the `erfc` continued fraction.
const FRAC_1_SQRT_PI: f64 = consts::FRAC_2_SQRT_PI * 0.5;

/// The two-sided p-value for a standard-normal statistic `z`, floored at [`MIN_P_VALUE`].
pub(crate) fn two_sided_p_from_z(z: f64) -> f64 {
    clamp_p_value(2.0 * normal_cdf(-z.abs()))
}

/// The standard normal cumulative distribution function, `Φ(z)`.
fn normal_cdf(z: f64) -> f64 {
    0.5 * erfc(-z / consts::SQRT_2)
}

/// Clamps a computed tail probability into the reportable range; mirrors
/// `cbh_stats::p_value::clamp_p_value`.
pub(crate) fn clamp_p_value(p: f64) -> f64 {
    if !p.is_finite() {
        return 1.0;
    }
    p.clamp(MIN_P_VALUE, 1.0)
}

/// The complementary error function, `erfc(x) = 1 − erf(x)`.
fn erfc(x: f64) -> f64 {
    if x.abs() < SERIES_LIMIT {
        return 1.0 - erf_series(x, SERIES_TERMS);
    }
    if x.is_sign_positive() {
        tail_erfc(x, FRACTION_DEPTH)
    } else {
        2.0 - tail_erfc(-x, FRACTION_DEPTH)
    }
}

/// The error function `erf(x)` for arguments below [`SERIES_LIMIT`].
fn erf_series(x: f64, terms: u32) -> f64 {
    let x_squared = x * x;
    let mut term = x;
    let mut total = x;
    let mut denominator = 1.0_f64;
    for _ in 0..terms {
        denominator += 2.0;
        term *= 2.0 * x_squared / denominator;
        total += term;
    }
    consts::FRAC_2_SQRT_PI * (-x_squared).exp() * total
}

/// The complementary error function for arguments at or above [`SERIES_LIMIT`].
fn tail_erfc(x: f64, depth: u32) -> f64 {
    let levels_below_top = depth.saturating_sub(1);
    let mut numerator = f64::from(levels_below_top) * NUMERATOR_STEP;
    let mut fraction = 0.0_f64;
    for _ in 0..levels_below_top {
        fraction = numerator / (x + fraction);
        numerator -= NUMERATOR_STEP;
    }
    FRAC_1_SQRT_PI * (-x * x).exp() / (x + fraction)
}
