//! Turning counts into the coordinates a figure is laid out in.
//!
//! Nearly every position in the catalogue is a count of something drawn: a commit
//! position along an axis, a row in a grid, a sample size a bar is scaled by. `plotters`
//! lays out in `f64`, so each of those has to be converted, and the argument for why the
//! conversion is safe is the same one every time. Stating it once here is what keeps it
//! from being restated — or quietly assumed — at every call site.

/// The coordinate a count of plotted elements sits at.
///
/// The count is of things a reader looks at in a single figure: commits along an axis,
/// cells in a grid, observations in a sample. A figure that a reader can take in bounds
/// that count far below the magnitude at which an `f64` stops representing consecutive
/// integers exactly, so the conversion is lossless for every count a figure can hold.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "the argument counts elements drawn in one figure, which is bounded by what \
              a reader can look at and therefore stays orders of magnitude below 2^53, \
              the point at which an f64 stops representing consecutive integers exactly"
)]
pub(crate) fn of(count: usize) -> f64 {
    count as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_converts_to_the_same_number() {
        assert!((of(0) - 0.0).abs() < f64::EPSILON);
        assert!((of(1) - 1.0).abs() < f64::EPSILON);
        assert!((of(4096) - 4096.0).abs() < f64::EPSILON);
    }
}
