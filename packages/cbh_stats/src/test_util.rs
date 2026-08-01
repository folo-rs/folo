//! Comparison helpers shared by the unit tests of this crate.

/// Asserts that two values differ by no more than `tolerance`.
#[track_caller]
pub(crate) fn close(actual: f64, expected: f64, tolerance: f64) {
    let difference = (actual - expected).abs();
    assert!(
        difference <= tolerance,
        "expected {expected}, got {actual} (difference {difference} exceeds {tolerance})"
    );
}

/// Asserts that two values differ by no more than `tolerance` relative to the
/// magnitude of the expected value.
///
/// This is the comparison to reach for when the expected value spans orders of
/// magnitude, where an absolute tolerance would either be unmeetable at the top
/// of the range or meaningless at the bottom.
///
/// # Panics
///
/// Panics if `expected` is zero, which has no magnitude for the tolerance to be
/// relative to. Use [`close`] for that case.
#[track_caller]
pub(crate) fn close_relative(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        expected != 0.0,
        "a relative comparison needs a non-zero expected value to measure against; use `close` for an absolute tolerance"
    );

    let error = ((actual - expected) / expected).abs();
    assert!(
        error <= tolerance,
        "expected {expected}, got {actual} (relative error {error} exceeds {tolerance})"
    );
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn close_relative_scales_its_tolerance_with_the_expected_magnitude() {
        // The same relative tolerance holds at both ends of the range, which is the
        // property this helper exists to provide.
        close_relative(1.000_000_1e-12, 1e-12, 1e-6);
        close_relative(1.000_000_1e12, 1e12, 1e-6);
    }

    #[test]
    #[should_panic]
    fn close_relative_refuses_a_zero_expected_value() {
        // Zero has no magnitude to be relative to: the division would yield NaN and
        // report even an exactly equal pair as differing. Refusing the comparison
        // sends the caller to `close` instead of failing for an unrelated reason.
        close_relative(0.0, 0.0, 1e-9);
    }
}
