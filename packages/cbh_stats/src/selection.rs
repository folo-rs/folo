//! Selection-adjusted change-point p-values.
//!
//! # Why this exists
//!
//! The change-point detector reports the Mann–Whitney p-value at the split it *chose* by searching
//! every interior position for the most convincing one. That p-value — this crate calls it the
//! *tainted p* — is not an honest p-value: the search means even a series with no real level shift
//! tends to throw up one striking-looking split, so comparing the tainted p against a significance
//! threshold overstates how surprising the finding is.
//!
//! [`change_point_adjusted_p`] converts a tainted p into an honest, selection-adjusted p-value by
//! reading a committed calibration table. The table records, per series length, the null
//! distribution of the detector's whole procedure — locate the split, reject a too-short regime,
//! then take the Mann–Whitney p-value — so the adjusted value obeys the p-value contract
//! `P(adjusted <= a) <= a` under the null. The table is generated and certified by the
//! `cargo-bench-history-calibration` crate; see that crate and `cargo-bench-history`'s design
//! appendix for the method and the justification of every number.
//!
//! # Scope
//!
//! Only the change-point detector's position search is corrected here. Branch-comparison mode runs
//! its own search and is knowingly left uncorrected for now (folo-rs/folo#485).

mod table;

pub use table::{ADJUSTED_LEVELS, CRITICAL_TAINTED_P, MAX_SERIES_LEN, MIN_SERIES_LEN, NUM_LEVELS};

/// The honest, selection-adjusted p-value for a change point located by searching every split.
///
/// `tainted_p` is the Mann–Whitney p-value the detector reports at the split it chose, and
/// `series_len` is the number of points it searched over. The result is the smallest calibrated
/// adjusted level whose critical value still covers `tainted_p`, clamped into `[tainted_p, 1.0]`,
/// so it is never more significant than the input and is a valid p-value under the null.
///
/// # Panics
///
/// Panics when `series_len` is outside `MIN_SERIES_LEN..=MAX_SERIES_LEN`. The pipeline caps every
/// analyzed series to that range, so a length outside it is a caller bug rather than a runtime
/// condition.
#[must_use]
pub fn change_point_adjusted_p(tainted_p: f64, series_len: usize) -> f64 {
    assert!(
        (MIN_SERIES_LEN..=MAX_SERIES_LEN).contains(&series_len),
        "series length {series_len} is outside the calibrated range \
         {MIN_SERIES_LEN}..={MAX_SERIES_LEN}"
    );
    let row_index = series_len
        .checked_sub(MIN_SERIES_LEN)
        .expect("series_len >= MIN_SERIES_LEN by the assert above");
    let row = CRITICAL_TAINTED_P
        .get(row_index)
        .expect("row index is in range by the assert above");
    // The rungs are ascending in critical value, so the first rung whose critical value reaches the
    // observed tainted p is the smallest adjusted level that honestly covers it. The top rung's
    // critical value is 1.0, so a tainted p in `[0, 1]` always lands on a rung.
    row.iter()
        .position(|&critical| critical >= tainted_p)
        .and_then(|rung| ADJUSTED_LEVELS.get(rung).copied())
        .unwrap_or(1.0)
        .max(tainted_p)
        .min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one contract every caller relies on: the adjusted value is a valid p-value that never
    /// makes a finding look *more* significant than its tainted input.
    #[test]
    fn adjusted_p_stays_between_tainted_and_one() {
        for series_len in [MIN_SERIES_LEN, 11, 50, 200, MAX_SERIES_LEN] {
            for &tainted_p in &[0.0, 1e-6, 1e-3, 0.01, 0.03, 0.05, 0.2, 0.9, 1.0] {
                let adjusted = change_point_adjusted_p(tainted_p, series_len);
                assert!(
                    adjusted >= tainted_p,
                    "n={series_len}: adjusted {adjusted} below tainted {tainted_p}"
                );
                assert!(
                    adjusted <= 1.0,
                    "n={series_len}: adjusted {adjusted} above one"
                );
            }
        }
    }

    /// A weaker tainted p-value can never earn a stronger adjusted one, so the correction preserves
    /// the ordering the ranking downstream depends on.
    #[test]
    fn adjusted_p_is_non_decreasing_in_tainted_p() {
        let mut previous = f64::NEG_INFINITY;
        for step in 0..=1000 {
            let tainted_p = f64::from(step) / 1000.0;
            let adjusted = change_point_adjusted_p(tainted_p, 200);
            assert!(
                adjusted >= previous,
                "adjusted {adjusted} at tainted {tainted_p} fell below its predecessor {previous}"
            );
            previous = adjusted;
        }
    }

    /// At the shortest history only one split (the exact middle) clears the regime floor, so the
    /// position search is trivial and the honest answer is the tainted p unchanged.
    #[test]
    fn shortest_history_needs_no_correction() {
        let tainted_p = 0.03;
        let adjusted = change_point_adjusted_p(tainted_p, MIN_SERIES_LEN);
        assert!(
            (adjusted - tainted_p).abs() < 1e-12,
            "expected identity at n={MIN_SERIES_LEN}, got {adjusted}"
        );
    }

    /// A long history searches many splits, so a tainted p that looks significant is corrected well
    /// past the gate — the whole point of the table.
    #[test]
    fn long_history_corrects_strongly() {
        let adjusted = change_point_adjusted_p(0.01, MAX_SERIES_LEN);
        assert!(adjusted > 0.05, "expected a strong correction at n={MAX_SERIES_LEN}, got {adjusted}");
    }

    #[test]
    #[should_panic(expected = "outside the calibrated range")]
    fn panics_below_the_range() {
        let adjusted = change_point_adjusted_p(0.01, MIN_SERIES_LEN.saturating_sub(1));
        assert!((0.0..=1.0).contains(&adjusted), "the call must panic before returning");
    }

    #[test]
    #[should_panic(expected = "outside the calibrated range")]
    fn panics_above_the_range() {
        let adjusted = change_point_adjusted_p(0.01, MAX_SERIES_LEN.saturating_add(1));
        assert!((0.0..=1.0).contains(&adjusted), "the call must panic before returning");
    }
}
