//! A compact positional comparison, without a fabricated zero origin or numeric axis.

/// Draws the observed base range and current measurement on one shared linear span.
pub(crate) fn range_sketch(minimum: f64, maximum: f64, current: f64) -> String {
    // Fits beside short endpoint labels in a narrow GitHub comment without scrolling.
    const COLUMNS: usize = 19;
    const LAST: usize = COLUMNS - 1;
    let low = minimum.min(current);
    let high = maximum.max(current);
    // Normalize before subtracting so large finite values cannot overflow the span.
    let magnitude = low.abs().max(high.abs());
    let position = |value: f64| {
        if high <= low {
            return LAST.div_ceil(2);
        }
        let fraction = (value / magnitude - low / magnitude) / (high / magnitude - low / magnitude);
        column(fraction, LAST)
    };
    let start = position(minimum);
    let end = position(maximum);
    let point = position(current);
    let mut cells = ['-'; COLUMNS];
    for (index, cell) in cells.iter_mut().enumerate() {
        if index >= start && index <= end {
            *cell = if start == end {
                '|'
            } else if index == start {
                '['
            } else if index == end {
                ']'
            } else {
                '='
            };
        }
    }
    if let Some(cell) = cells.get_mut(point) {
        *cell = '*';
    }
    let sketch: String = cells.into_iter().collect();
    if current < minimum {
        format!("current {sketch} base")
    } else {
        format!("base {sketch} current")
    }
}

/// Rounds within the display grid; tiny ranges can honestly collapse to a single mark.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the normalized finite fraction is clamped to the display grid before conversion"
)]
fn column(fraction: f64, last: usize) -> usize {
    // The caller's fixed small grid is exactly representable.
    let width = f64::from(u32::try_from(last).expect("the sketch width fits in u32"));
    (fraction.clamp(0.0, 1.0) * width).round() as usize
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn range_and_measurement_keep_their_relative_positions_in_both_directions() {
        assert_eq!(
            range_sketch(100.0, 110.0, 120.0),
            "base [========]--------* current"
        );
        assert_eq!(
            range_sketch(100.0, 110.0, 90.0),
            "current *--------[========] base"
        );
        assert_eq!(
            range_sketch(100.0, 100.0, 120.0),
            "base |-----------------* current"
        );
        assert_eq!(
            range_sketch(100.0, 100.0, 90.0),
            "current *-----------------| base"
        );
    }

    #[test]
    fn scale_translation_and_extreme_values_do_not_create_a_zero_origin() {
        let ordinary = range_sketch(100.0, 110.0, 120.0);
        assert_eq!(range_sketch(1000.0, 1100.0, 1200.0), ordinary);
        assert_eq!(range_sketch(-20.0, -10.0, 0.0), ordinary);
        assert_eq!(range_sketch(1e300, 1.1e300, 1.2e300), ordinary);
        assert_eq!(range_sketch(1e-300, 1.1e-300, 1.2e-300), ordinary);
        assert_eq!(
            range_sketch(1.0, 2.0, f64::MAX),
            "base |-----------------* current"
        );
        assert_eq!(
            range_sketch(0.0, 0.0, 0.0),
            "base ---------*--------- current"
        );
        assert_eq!(range_sketch(-0.0, 0.0, 0.0), range_sketch(0.0, 0.0, 0.0));
        assert_eq!(column(0.25, 22), 6);
    }
}
