//! A full-line, zero-anchored comparison of the base range and current observation.

/// Draws the observed base range and current measurement on one shared linear span.
pub(crate) fn range_sketch(minimum: f64, maximum: f64, current: f64) -> String {
    // Fill the desktop book's code area after its padding, including both delimiters and
    // a coincident zero/current mark. Narrow readers scroll instead of losing resolution.
    const COLUMNS: usize = 83;
    const LAST: usize = COLUMNS - 1;
    let low = minimum.min(current).min(0.0);
    let high = maximum.max(current).max(0.0);
    if high <= low {
        // Every value is zero: a long axis would invent a nonzero span.
        return "0[*]".to_owned();
    }
    // Normalize before subtracting so large finite values cannot overflow the span.
    let magnitude = low.abs().max(high.abs());
    let position = |value: f64| {
        let fraction = (value / magnitude - low / magnitude) / (high / magnitude - low / magnitude);
        column(fraction, LAST)
    };
    let start = position(minimum);
    let end = position(maximum);
    let point = position(current);
    let zero = position(0.0);
    let mut sketch = String::new();
    for index in 0..COLUMNS {
        if index == zero {
            sketch.push('0');
        }
        if index == start {
            sketch.push('[');
        }
        // Delimiters enclose scaled cells, rather than replacing an endpoint. Keep both
        // zero and the observation visible when they coincide at this display resolution.
        if index == point {
            sketch.push('*');
        } else if index != zero {
            sketch.push(if index >= start && index <= end {
                '='
            } else {
                '-'
            });
        }
        if index == end {
            sketch.push(']');
        }
    }
    sketch
}

/// Rounds within the display grid; tiny ranges can honestly collapse to a single cell.
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
    fn nonnegative_comparisons_start_at_zero_and_use_the_full_line() {
        let regression = range_sketch(100.0, 110.0, 120.0);
        assert_eq!(
            regression,
            format!("0{}[{}]{}*", "-".repeat(67), "=".repeat(8), "-".repeat(6))
        );
        let improvement = range_sketch(100.0, 120.0, 90.0);
        assert_eq!(
            improvement,
            format!("0{}*{}[{}]", "-".repeat(61), "-".repeat(5), "=".repeat(15))
        );
        assert_eq!(regression.len(), 85);
        assert_eq!(improvement.len(), 85);
        assert_eq!(column(0.25, 82), 21);
    }

    #[test]
    fn scaling_preserves_positions_but_translation_keeps_the_real_zero_anchor() {
        let ordinary = range_sketch(100.0, 110.0, 120.0);
        assert_eq!(range_sketch(1000.0, 1100.0, 1200.0), ordinary);
        assert_eq!(range_sketch(1e300, 1.1e300, 1.2e300), ordinary);
        assert_eq!(range_sketch(1e-300, 1.1e-300, 1.2e-300), ordinary);
        let translated = range_sketch(0.0, 10.0, 20.0);
        assert_ne!(translated, ordinary);
        assert!(translated.starts_with("0["));
        assert!(translated.ends_with('*'));
    }

    #[test]
    fn zero_flat_and_coincident_values_keep_every_mark_without_inventing_a_span() {
        assert_eq!(range_sketch(0.0, 0.0, 0.0), "0[*]");
        assert_eq!(range_sketch(-0.0, 0.0, -0.0), "0[*]");
        assert_eq!(
            range_sketch(0.0, 0.0, 120.0),
            format!("0[]{}*", "-".repeat(81))
        );
        assert_eq!(
            range_sketch(120.0, 120.0, 0.0),
            format!("0*{}[=]", "-".repeat(81))
        );
        assert_eq!(
            range_sketch(60.0, 60.0, 120.0),
            format!("0{}[=]{}*", "-".repeat(40), "-".repeat(40))
        );
        assert_eq!(
            range_sketch(120.0, 120.0, 120.0),
            format!("0{}[*]", "-".repeat(81))
        );
    }

    #[test]
    fn negative_values_remain_left_of_zero_on_the_same_increasing_scale() {
        // Stored and fitted measurements admit finite negatives, so do not clip their range.
        assert_eq!(
            range_sketch(-120.0, -100.0, -90.0),
            format!("[{}]{}*{}0", "=".repeat(15), "-".repeat(6), "-".repeat(60))
        );
        assert_eq!(
            range_sketch(-110.0, -100.0, -120.0),
            format!("*{}[{}]{}0", "-".repeat(6), "=".repeat(8), "-".repeat(67))
        );
        assert_eq!(
            range_sketch(-60.0, 60.0, 120.0),
            format!("[{}0{}]{}*", "=".repeat(27), "=".repeat(28), "-".repeat(26))
        );
    }

    #[test]
    fn extreme_finite_values_and_rounding_collisions_never_overflow_or_hide_zero() {
        let extreme = range_sketch(-f64::MAX, f64::MAX, 0.0);
        assert_eq!(extreme, format!("[{}0*{}]", "=".repeat(41), "=".repeat(41)));
        assert_eq!(extreme.len(), 86);
        assert_eq!(
            range_sketch(1.0, 2.0, f64::MAX),
            format!("0[]{}*", "-".repeat(81))
        );
        let tiny = f64::from_bits(1);
        assert_eq!(
            range_sketch(tiny, tiny * 2.0, tiny * 4.0),
            format!("0{}[{}]{}*", "-".repeat(20), "=".repeat(21), "-".repeat(40))
        );
        let rounded = range_sketch(100.0, 120.0, 120.01);
        assert!(rounded.starts_with('0'));
        assert!(rounded.ends_with("*]"));
        assert_eq!(rounded.matches('*').count(), 1);
    }
}
