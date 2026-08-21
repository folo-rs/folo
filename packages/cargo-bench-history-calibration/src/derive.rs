//! The adjusted-level grid, the conservativeness margin, and the row-by-row ladder.
//!
//! This turns the null distribution from [`crate::permutation`] into the committed table's
//! numbers. Every series length gets a *ladder*: on a shared grid of adjusted levels, the largest
//! tainted p-value that still deserves that level once the detector's every-position search is
//! accounted for. Monte Carlo rows carry a Dvoretzky–Kiefer–Wolfowitz margin so the committed
//! numbers err, family-wide and to a stated certainty, toward reporting fewer findings rather than
//! more (the book's "Selection adjustment" appendix).

use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::permutation::{FindingDist, collect_exact, collect_monte_carlo, row_seed};
use crate::{
    ANCHOR_LEVEL, DKW_ALPHA, EXACT_MAX_N, LADDER_FLOOR, LADDER_RATIO, MAX_SERIES_LEN,
    MIN_SERIES_LEN, count_f64,
};

/// The committed calibration table: the adjusted-level grid shared by every row, and one ladder
/// of critical tainted-p values per series length.
///
/// It is the in-memory form of `cbh_stats/src/selection/table.rs`; `crate::render` serializes it
/// and the run-time primitive `cbh_stats::selection::change_point_adjusted_p` reads the serialized
/// arrays back. Rows run from [`MIN_SERIES_LEN`] to [`MAX_SERIES_LEN`] with no gaps, so a lookup is
/// an exact row index with no interpolation.
#[derive(Debug)]
pub struct Table {
    /// The adjusted levels each ladder rung sits at, ascending, ending at exactly `1.0`.
    levels: Vec<f64>,
    /// One row per length in `MIN_SERIES_LEN..=MAX_SERIES_LEN`; each holds a critical tainted-p per
    /// level, in the same order as [`Self::levels`].
    rows: Vec<Vec<f64>>,
}

impl Table {
    /// The shared grid of adjusted levels.
    pub(crate) fn levels(&self) -> &[f64] {
        &self.levels
    }

    /// The per-length ladders, row `i` describing length `MIN_SERIES_LEN + i`.
    pub(crate) fn rows(&self) -> &[Vec<f64>] {
        &self.rows
    }
}

/// Builds the whole committed table, drawing `samples` orderings for each Monte Carlo row.
///
/// Rows up to [`EXACT_MAX_N`] are counted exactly and carry no margin; longer rows are sampled and
/// share one Dvoretzky–Kiefer–Wolfowitz margin. Rows are independent, so they are derived in
/// parallel across the available CPUs.
#[must_use]
pub fn derive_table(samples: u64) -> Table {
    let levels = adjusted_levels();
    let lengths: Vec<usize> = (MIN_SERIES_LEN..=MAX_SERIES_LEN).collect();
    let monte_carlo_rows = lengths.iter().filter(|&&n| n > EXACT_MAX_N).count();
    let dkw = dkw_margin(samples, monte_carlo_rows);
    let levels_ref = &levels;

    // A row's cost grows with `n` (each of the `samples` orderings does O(n) work), so a static
    // split would leave the long-series rows running as a single-threaded tail. Each row is an
    // independent task, so Rayon's work-stealing keeps every core busy despite that skew. A row's
    // output is a pure function of `n` (its `row_seed(n)` stream), so the table is identical
    // regardless of the order the scheduler happens to run the rows in.
    let rows: Vec<Vec<f64>> = lengths
        .par_iter()
        .map(|&n| {
            let (distribution, margin) = if n <= EXACT_MAX_N {
                // Exhaustive counting is exact, so an exact row needs no margin.
                (collect_exact(n), 0.0)
            } else {
                (collect_monte_carlo(n, samples, row_seed(n)), dkw)
            };
            ladder(&distribution, margin, levels_ref)
        })
        .collect();

    Table { levels, rows }
}

/// The ascending grid of adjusted levels every row's ladder is sampled on.
///
/// Geometric with ratio [`LADDER_RATIO`], anchored exactly on [`ANCHOR_LEVEL`] so the significance
/// gate compares against the very bits it uses rather than a rounded neighbor (§6.1). It spans
/// down to [`LADDER_FLOOR`] and ends with an explicit `1.0` rung that accepts everything. Each rung
/// is computed independently as `anchor · ratio^k`, the power spelled out by [`int_pow`] so the
/// grid reproduces bit-for-bit on every platform, so no rounding accumulates across the grid.
pub(crate) fn adjusted_levels() -> Vec<f64> {
    let down_steps = grid_steps(ANCHOR_LEVEL / LADDER_FLOOR);
    let up_steps = grid_steps(1.0 / ANCHOR_LEVEL);

    let mut levels = Vec::with_capacity(down_steps.saturating_add(up_steps).saturating_add(2));
    for k in (1..=down_steps).rev() {
        levels.push(ANCHOR_LEVEL / int_pow(LADDER_RATIO, k));
    }
    levels.push(ANCHOR_LEVEL);
    for k in 1..=up_steps {
        levels.push(ANCHOR_LEVEL * int_pow(LADDER_RATIO, k));
    }
    levels.push(1.0);
    levels
}

/// How many geometric steps of [`LADDER_RATIO`] fit strictly inside `span` (a ratio `>= 1`).
///
/// `span = ratio^steps`, so `steps = ln(span)/ln(ratio)`; flooring counts only the rungs that stay
/// on the intended side of the endpoint. For the committed constants this is at most a few dozen,
/// far inside `usize` and exactly representable once floored.
fn grid_steps(span: f64) -> usize {
    let steps = span.ln() / LADDER_RATIO.ln();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "steps is a small non-negative count (a few dozen), floored, far inside usize"
    )]
    let count = steps.floor() as usize;
    count
}

/// `base` raised to a small non-negative integer power, by explicit binary exponentiation.
///
/// [`f64::powi`] lowers to a platform intrinsic whose multiplication schedule is not fixed across
/// targets, so its result can differ in the last bit from one operating system to the next. The
/// committed level grid is pinned to the last bit and re-derived by a freshness test that runs on
/// every platform (§6.1), so the grid must reproduce byte-for-byte everywhere. Spelling the
/// exponentiation out as a fixed sequence of IEEE-754 multiplications — each individually portable
/// — makes the derived bits independent of the host. Exponents here are a few dozen at most, well
/// inside the loop's reach.
fn int_pow(mut base: f64, mut exp: usize) -> f64 {
    let mut acc = 1.0;
    while exp > 0 {
        if exp & 1 == 1 {
            acc *= base;
        }
        base *= base;
        exp >>= 1;
    }
    acc
}

/// The one-sided Dvoretzky–Kiefer–Wolfowitz margin that makes a Monte Carlo row conservative
/// family-wide.
///
/// Massart's inequality (tight constant one) bounds `P(sup(G − G_hat) > eps) ≤ exp(−2·N·eps²)`
/// for one row's empirical CDF `G_hat` of `N = samples` draws. Splitting the family budget
/// [`DKW_ALPHA`] evenly across the Monte Carlo rows (Bonferroni) and inverting gives this margin,
/// which is added to every sampled CDF before the ladder reads it (§6.2).
pub(crate) fn dkw_margin(samples: u64, monte_carlo_rows: usize) -> f64 {
    let alpha_row = DKW_ALPHA / count_f64(monte_carlo_rows as u64);
    (alpha_row.recip().ln() / (2.0 * count_f64(samples))).sqrt()
}

/// The critical tainted-p for every grid level in one row.
///
/// For adjusted level `a`, the honest bar is: at most a fraction `a` of *all* orderings may be a
/// finding whose tainted p-value is at or below the returned critical value. Reading the
/// conservative CDF `G_hat + margin` against `a` allows `floor((a − margin)·total)` findings, so the
/// critical value is the largest finding p-value whose *entire* equal-valued mass fits within that
/// count — `0.0` when even the smallest finding value spills past it (an unreachable rung), `1.0`
/// when every finding fits (a rung that accepts any input). Requiring the full mass to fit, rather
/// than crediting a value the moment its first ordering does, is what keeps the row conservative on
/// the Mann–Whitney statistic's heavily tied p-values.
fn ladder(distribution: &FindingDist, margin: f64, levels: &[f64]) -> Vec<f64> {
    let total = count_f64(distribution.total());
    let findings = distribution.findings();
    let findings_f = count_f64(findings);
    levels
        .iter()
        .map(|&level| {
            if level >= 1.0 {
                // The top rung is the identity: any tainted p-value is at most one.
                return 1.0;
            }
            let allowance = level - margin;
            if allowance <= 0.0 {
                return 0.0;
            }
            let target = (allowance * total).floor();
            if target < 1.0 {
                return 0.0;
            }
            if target >= findings_f {
                return 1.0;
            }
            distribution.largest_within(floored_count(target))
        })
        .collect()
}

/// A non-negative, already-floored count as `u64`.
///
/// Callers pass `x.floor()` of a value known to be below `total` (at most `n!`, far inside
/// `f64`'s exact-integer range), so the conversion neither truncates a fraction nor overflows.
fn floored_count(value: f64) -> u64 {
    debug_assert!(
        value >= 0.0 && value.is_finite(),
        "count must be a finite non-negative"
    );
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "value is a non-negative floored count below n! < 2^53, exact in u64"
    )]
    let count = value as u64;
    count
}

#[cfg(test)]
mod tests {
    use cbh_stats::selection;

    use super::*;
    use crate::permutation::collect_exact;

    /// Compares two `f64` slices for exact bit equality, so the freshness assertions do not trip
    /// the float-comparison lint and pin the committed bytes to the last digit.
    fn same_bits(left: &[f64], right: &[f64]) -> bool {
        left.iter()
            .map(|value| value.to_bits())
            .eq(right.iter().map(|value| value.to_bits()))
    }

    /// The committed table, read through the production primitive, must never report a change point
    /// as *more* significant than the exact null distribution allows. For every reachable exact row
    /// this enumerates the true null and checks `change_point_adjusted_p(v, n, sp) >= F(v)` at every
    /// distinct finding p-value, where `F` is the exact finding CDF. A failure here means the
    /// calibration ladder credited a discrete p-value before its full mass fit — the
    /// anti-conservative defect the `largest_within` inverse exists to prevent.
    #[test]
    #[cfg_attr(miri, ignore = "exhaustive enumeration is far too slow under Miri")]
    fn committed_exact_rows_are_conservative() {
        for series_len in MIN_SERIES_LEN..=EXACT_MAX_N {
            let FindingDist::Counts {
                cumulative, total, ..
            } = collect_exact(series_len)
            else {
                panic!("rows up to EXACT_MAX_N enumerate exactly");
            };
            let total_f = count_f64(total);
            let searched = series_len
                .saturating_sub(crate::MIN_REGIME.saturating_mul(2))
                .saturating_add(1);
            for (value, cumulative_mass) in &cumulative {
                let honest = count_f64(*cumulative_mass) / total_f;
                let adjusted = selection::change_point_adjusted_p(*value, series_len, searched);
                assert!(
                    adjusted >= honest,
                    "n={series_len}: adjusted p {adjusted} < honest CDF {honest} at tainted p {value}"
                );
            }
        }
    }

    #[test]
    fn grid_matches_the_committed_levels() {
        assert!(
            same_bits(&adjusted_levels(), &selection::ADJUSTED_LEVELS),
            "the derived adjusted-level grid differs from the committed ADJUSTED_LEVELS"
        );
    }

    /// Re-derives the first row from scratch — exact enumeration, no Monte Carlo, no margin — and
    /// pins it to the committed table. Any change to the derivation or to the shipped data fails
    /// `just test` without waiting for the full-table `check` recipe (design.md §6.4).
    #[test]
    #[cfg_attr(
        miri,
        ignore = "the 3.6M-ordering enumeration exceeds the Miri time budget"
    )]
    fn first_row_reproduces_the_committed_table() {
        let row = ladder(&collect_exact(MIN_SERIES_LEN), 0.0, &adjusted_levels());
        let committed = selection::CRITICAL_TAINTED_P
            .first()
            .expect("the committed table has at least one row");
        assert!(
            same_bits(&row, committed),
            "the re-derived first row differs from the committed table"
        );
    }

    /// Shape invariants over the shipped table. Monotonicity in `n` is deliberately not asserted:
    /// it is false at the bottom of the ladder, where a short history cannot reach a small
    /// p-value, so a naive check would fail on correct data (design.md §6.4).
    #[test]
    fn committed_table_has_valid_shape() {
        for (row_index, row) in selection::CRITICAL_TAINTED_P.iter().enumerate() {
            let mut previous = f64::NEG_INFINITY;
            for &critical in row {
                assert!(
                    (0.0..=1.0).contains(&critical),
                    "row {row_index}: critical value {critical} is outside [0, 1]"
                );
                assert!(
                    critical >= previous,
                    "row {row_index}: critical value {critical} falls below its predecessor \
                     {previous}"
                );
                previous = critical;
            }
            assert_eq!(
                row.last().map(|value| value.to_bits()),
                Some(1.0_f64.to_bits()),
                "row {row_index}: the top rung must accept every tainted p-value"
            );
        }
    }
}
