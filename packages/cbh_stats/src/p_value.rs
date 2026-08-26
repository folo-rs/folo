//! The reportable range every test in this crate clamps its p-value into.

/// The p-value of a test that found no evidence against the null hypothesis.
pub(crate) const NO_EVIDENCE: f64 = 1.0;

/// The smallest p-value any test in this crate reports.
///
/// Every test floors its p-value here instead of letting an extreme statistic
/// underflow to exactly zero. A zero p-value clears every Benjamini–Hochberg
/// threshold unconditionally — `0 <= (k/m)·q` holds for any false-discovery
/// rate, however strict — so a single catastrophically extreme statistic would
/// be reported without the multiple-comparison correction having any say. The
/// floor keeps that comparison meaningful while still conveying overwhelming
/// significance.
///
/// It also marks the point past which a p-value stops being a quantity worth
/// reasoning about: at `1e-15` the distributional assumptions behind these
/// tests dominate the arithmetic by many orders of magnitude, so a smaller
/// number would carry no additional meaning.
pub(crate) const MIN_P_VALUE: f64 = 1e-15;

/// Clamps a computed tail probability into the reportable p-value range.
///
/// A tail probability that is not finite cannot support any conclusion, so it
/// maps to [`NO_EVIDENCE`]. That covers `NaN`, which would otherwise propagate
/// and silently compare false against every threshold, and it covers an
/// infinite input, which no arithmetic on probabilities can produce and so only
/// arises when a computation has broken down. Reporting the broken case as
/// certainty would invent a finding out of a defect, and this crate's callers
/// act on small p-values.
///
/// A *finite* negative input is treated differently, and deliberately: it is
/// how a vanishing probability arrives once rounding carries it past zero, so
/// it floors to [`MIN_P_VALUE`] alongside an exact zero rather than being
/// discarded as evidence.
pub(crate) fn clamp_p_value(p: f64) -> f64 {
    if !p.is_finite() {
        return NO_EVIDENCE;
    }
    p.clamp(MIN_P_VALUE, NO_EVIDENCE)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "primitive outputs are compared against hand-computed exact values"
    )]

    use super::*;

    #[test]
    fn clamp_p_value_passes_ordinary_probabilities_through() {
        assert_eq!(clamp_p_value(0.5), 0.5);
        assert_eq!(clamp_p_value(0.05), 0.05);
        assert_eq!(clamp_p_value(NO_EVIDENCE), NO_EVIDENCE);
        assert_eq!(clamp_p_value(MIN_P_VALUE), MIN_P_VALUE);
    }

    #[test]
    fn clamp_p_value_floors_vanishing_probabilities() {
        // The whole point of the floor: a probability that underflowed to zero
        // must not come back out as zero.
        assert_eq!(clamp_p_value(0.0), MIN_P_VALUE);
        assert_eq!(clamp_p_value(1e-300), MIN_P_VALUE);
        assert!(clamp_p_value(0.0) > 0.0);
    }

    #[test]
    fn clamp_p_value_caps_probabilities_above_one() {
        // Continuity corrections can push an approximation a hair above one.
        assert_eq!(clamp_p_value(1.000_000_1), NO_EVIDENCE);
        assert_eq!(clamp_p_value(f64::INFINITY), NO_EVIDENCE);
    }

    #[test]
    fn clamp_p_value_reports_no_evidence_for_a_degenerate_statistic() {
        assert_eq!(clamp_p_value(f64::NAN), NO_EVIDENCE);
    }

    #[test]
    fn clamp_p_value_reports_no_evidence_for_a_negatively_infinite_statistic() {
        // No arithmetic on probabilities yields negative infinity, so it means the
        // computation behind it broke down. Flooring it would read as the most
        // significant result this crate can express, manufacturing a finding out of
        // a defect; the answer is that there is nothing to conclude.
        assert_eq!(clamp_p_value(f64::NEG_INFINITY), NO_EVIDENCE);
    }

    #[test]
    fn clamp_p_value_floors_a_finite_probability_rounded_past_zero() {
        // Unlike an infinity, a small negative is how a vanishing probability
        // arrives when rounding carries it just past zero, so it stays evidence.
        assert_eq!(clamp_p_value(-1e-300), MIN_P_VALUE);
        assert_eq!(clamp_p_value(-0.0), MIN_P_VALUE);
    }
}
