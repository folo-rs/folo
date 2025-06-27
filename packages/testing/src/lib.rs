//! Private helpers for testing and examples in Folo packages.

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::*;

/// Calculates the difference between two f64 values and considers
/// them equal if the difference is not more than `close_enough`.
///
/// This is a "correctly performed" floating point equality comparison.
#[must_use]
pub fn f64_diff_abs(a: f64, b: f64, close_enough: f64) -> f64 {
    let diff = (a - b).abs();

    if diff <= close_enough { 0.0 } else { diff }
}
