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
#[track_caller]
pub(crate) fn close_relative(actual: f64, expected: f64, tolerance: f64) {
    let error = ((actual - expected) / expected).abs();
    assert!(
        error <= tolerance,
        "expected {expected}, got {actual} (relative error {error} exceeds {tolerance})"
    );
}
