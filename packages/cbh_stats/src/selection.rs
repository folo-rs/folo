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
//! [`change_point_adjusted_p`] converts a tainted p into an honest, selection-adjusted p-value that
//! obeys the p-value contract `P(adjusted <= a) <= a` under the null. It does so by combining two
//! independently valid *upper bounds* on the true adjusted value and keeping the smaller — which,
//! since each bound is at least the truth, is itself at least the truth and so still honest:
//!
//! * A committed **calibration table**. Per series length it records the null distribution of the
//!   detector's whole procedure — locate the split, reject a too-short regime, then take the
//!   Mann–Whitney p-value. This is tight near the decision boundary, where borderline findings are
//!   won or lost. The table is generated and certified by the `cargo-bench-history-calibration`
//!   crate; see that crate and the book's "Selection adjustment" appendix for the method and the
//!   justification of every number.
//!
//! * A **union bound**, `searched_positions * tainted_p`. Reporting the most extreme of the
//!   positions the detector could have chosen can inflate the apparent significance by at most that
//!   factor, so multiplying it back out is a valid correction (Bonferroni over the reportable
//!   splits).
//!
//! # Why both
//!
//! The table is built by Monte Carlo, so it cannot resolve p-values below roughly its sampling
//! margin: every calibrated row bottoms out at the same shallow floor (~1e-3), however strong the
//! evidence. Feeding that floor into the downstream family correction would silently discard
//! obvious regressions in any batch of more than a couple of dozen series. The union bound has no
//! such floor — it scales straight down with `tainted_p` — so it carries the deep tail the table
//! cannot reach, while the table keeps the accuracy near the boundary the union bound is too loose
//! to provide. Ref: `cargo-bench-history`'s data-pipeline appendix, "Selection adjustment".
//!
//! # Scope
//!
//! Only the change-point detector's position search is corrected here. Branch-comparison mode runs
//! its own search and is knowingly left uncorrected for now (folo-rs/folo#485).

mod table;

pub use table::{ADJUSTED_LEVELS, CRITICAL_TAINTED_P, MAX_SERIES_LEN, MIN_SERIES_LEN, NUM_LEVELS};

/// The honest, selection-adjusted p-value for a change point located by searching every split.
///
/// `tainted_p` is the Mann–Whitney p-value the detector reports at the split it chose. `series_len`
/// selects the calibrated table row. `searched_positions` is the number of interior splits the
/// detector could have reported — those with both regimes at least the persistence floor — and
/// drives the union bound; it must be at least one.
///
/// The result is the smaller of the calibrated table lookup and the union bound
/// `searched_positions * tainted_p`, clamped into `[tainted_p, 1.0]`. Both are valid upper bounds
/// on the true adjusted value, so their minimum is too: it is never more significant than the input
/// and is a valid p-value under the null. See the module documentation for why both are needed.
///
/// # Panics
///
/// Panics when `series_len` is outside `MIN_SERIES_LEN..=MAX_SERIES_LEN`, or when
/// `searched_positions` is zero. The pipeline caps every analyzed series to that range and always
/// searches at least the middle split, so either is a caller bug rather than a runtime condition.
#[must_use]
pub fn change_point_adjusted_p(
    tainted_p: f64,
    series_len: usize,
    searched_positions: usize,
) -> f64 {
    assert!(
        (MIN_SERIES_LEN..=MAX_SERIES_LEN).contains(&series_len),
        "series length {series_len} is outside the calibrated range \
         {MIN_SERIES_LEN}..={MAX_SERIES_LEN}"
    );
    assert!(
        searched_positions >= 1,
        "the search covers at least the middle split"
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
    let table = row
        .iter()
        .position(|&critical| critical >= tainted_p)
        .and_then(|rung| ADJUSTED_LEVELS.get(rung).copied())
        .unwrap_or(1.0);
    // Union bound over the reportable splits (Bonferroni). Unlike the Monte Carlo table it has no
    // resolution floor, so it carries signals stronger than the table can resolve; near the
    // boundary it is loose and the table wins. Keeping the smaller of two valid bounds stays honest.
    let union = count_to_f64(searched_positions) * tainted_p;
    table.min(union).max(tainted_p).min(1.0)
}

/// Casts a split count to `f64`. Series lengths are far below 2^53, so the conversion is exact.
#[expect(
    clippy::cast_precision_loss,
    reason = "reportable-split counts are below the series-length cap and so far below 2^53"
)]
fn count_to_f64(count: usize) -> f64 {
    count as f64
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// The one contract every caller relies on: the adjusted value is a valid p-value that never
    /// makes a finding look *more* significant than its tainted input.
    #[test]
    fn adjusted_p_stays_between_tainted_and_one() {
        for series_len in [MIN_SERIES_LEN, 11, 50, 200, MAX_SERIES_LEN] {
            // One reportable split at the shortest history, growing with length; the exact count is
            // the detector's business, so exercise a spread here.
            for &searched in &[1_usize, 5, series_len] {
                for &tainted_p in &[0.0, 1e-6, 1e-3, 0.01, 0.03, 0.05, 0.2, 0.9, 1.0] {
                    let adjusted = change_point_adjusted_p(tainted_p, series_len, searched);
                    assert!(
                        adjusted >= tainted_p,
                        "n={series_len} k={searched}: adjusted {adjusted} below tainted {tainted_p}"
                    );
                    assert!(
                        adjusted <= 1.0,
                        "n={series_len} k={searched}: adjusted {adjusted} above one"
                    );
                }
            }
        }
    }

    /// A weaker tainted p-value can never earn a stronger adjusted one, so the correction preserves
    /// the ordering the ranking downstream depends on. The minimum of two non-decreasing bounds is
    /// itself non-decreasing.
    #[test]
    fn adjusted_p_is_non_decreasing_in_tainted_p() {
        let mut previous = f64::NEG_INFINITY;
        for step in 0..=1000 {
            let tainted_p = f64::from(step) / 1000.0;
            let adjusted = change_point_adjusted_p(tainted_p, 200, 191);
            assert!(
                adjusted >= previous,
                "adjusted {adjusted} at tainted {tainted_p} fell below its predecessor {previous}"
            );
            previous = adjusted;
        }
    }

    /// At the shortest history only one split (the exact middle) clears the regime floor, so there
    /// is no selection: both the table and a one-position union bound return the tainted p unchanged.
    #[test]
    fn shortest_history_needs_no_correction() {
        let tainted_p = 0.03;
        let adjusted = change_point_adjusted_p(tainted_p, MIN_SERIES_LEN, 1);
        assert!(
            (adjusted - tainted_p).abs() < 1e-12,
            "expected identity at n={MIN_SERIES_LEN}, got {adjusted}"
        );
    }

    /// A long history searches many splits, so a tainted p that looks significant is corrected well
    /// past the gate. Near the boundary the union bound is loose, so the table does the work.
    #[test]
    fn long_history_corrects_strongly() {
        let adjusted = change_point_adjusted_p(0.01, MAX_SERIES_LEN, MAX_SERIES_LEN - 9);
        assert!(
            adjusted > 0.05,
            "expected a strong correction at n={MAX_SERIES_LEN}, got {adjusted}"
        );
    }

    /// The reason the union bound exists: a signal far stronger than the table's Monte Carlo
    /// resolution must still yield a tiny adjusted value, not bottom out on the table's floor.
    #[test]
    fn deep_tail_beats_the_table_floor() {
        // A perfect separation floors the tainted p at the crate minimum; at n=100 the detector
        // searches 91 reportable splits.
        let tiny = 1e-15;
        let searched = 91;
        let adjusted = change_point_adjusted_p(tiny, 100, searched);
        // The table alone cannot go below ~1e-3 here; the union bound must pull it far below that.
        assert!(
            adjusted < 1e-10,
            "deep tail must beat the table floor, got {adjusted}"
        );
        // Specifically, the union bound wins: searched_positions * tainted_p.
        let union = count_to_f64(searched) * tiny;
        assert!(
            (adjusted - union).abs() <= union * 1e-9,
            "expected the union bound {union}, got {adjusted}"
        );
    }

    /// The complement: near the decision boundary the union bound is looser than the table, so the
    /// table's tighter correction is the one that survives.
    #[test]
    fn near_boundary_uses_the_table() {
        let tainted_p = 0.001;
        let searched = 91;
        let adjusted = change_point_adjusted_p(tainted_p, 100, searched);
        let union = count_to_f64(searched) * tainted_p;
        assert!(
            adjusted < union,
            "expected the table to beat the union bound {union}, got {adjusted}"
        );
        assert!(
            adjusted > tainted_p,
            "a searched history still earns some correction"
        );
    }

    #[test]
    #[should_panic(expected = "outside the calibrated range")]
    fn panics_below_the_range() {
        let adjusted = change_point_adjusted_p(0.01, MIN_SERIES_LEN.saturating_sub(1), 1);
        assert!(
            (0.0..=1.0).contains(&adjusted),
            "the call must panic before returning"
        );
    }

    #[test]
    #[should_panic(expected = "outside the calibrated range")]
    fn panics_above_the_range() {
        let adjusted = change_point_adjusted_p(0.01, MAX_SERIES_LEN.saturating_add(1), 1);
        assert!(
            (0.0..=1.0).contains(&adjusted),
            "the call must panic before returning"
        );
    }

    #[test]
    #[should_panic(expected = "at least the middle split")]
    fn panics_on_zero_searched_positions() {
        let adjusted = change_point_adjusted_p(0.01, 100, 0);
        assert!(
            (0.0..=1.0).contains(&adjusted),
            "the call must panic before returning"
        );
    }
}
