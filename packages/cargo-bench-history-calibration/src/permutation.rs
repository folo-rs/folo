//! The null-distribution procedure, evaluated on rank orderings.
//!
//! Under the null hypothesis (no real level shift) a series' values matter only through their
//! rank ordering, and every ordering of `n` distinct values is equally likely. So the
//! selection-adjusted p-value is a property of `n` alone: the distribution, over all `n!`
//! orderings, of the p-value the production change-point procedure reports.
//!
//! That procedure is: locate the split with **Pettitt** (over every interior position), reject it
//! unless the shorter side has at least [`MIN_REGIME`] points, and otherwise score it with the
//! two-sided **Mann–Whitney** p-value at that split — exact at every split whose smaller side keeps
//! the subset count inside f64's exact-integer range, the normal approximation only at the
//! near-balanced wide splits that overflow it, mirroring `cbh_stats::MannWhitneyU`. On a permutation
//! of `1..=n` the ranks *are* the values and the joint ranking at any split is the whole set, so
//! the exact tail depends only on the split size and the left rank sum; a [`SplitScorer`]
//! precomputes it per split size once, keeping an exact row as cheap to tabulate as an approximate
//! one. The kernel is proven equal to the production `cbh_stats` primitives by the crate's
//! `matches_production` test, so the table cannot drift from the code it describes.

use std::collections::HashMap;

use crate::normal::{clamp_p_value, two_sided_p_from_z};
use crate::{
    FNV_OFFSET_BASIS, FNV_PRIME, MIN_REGIME, SPLITMIX_GAMMA, SPLITMIX_MIX_A, SPLITMIX_MIX_B,
    count_f64,
};

/// Locates the split a rank ordering reports, returning `(size, left rank sum)` of the shorter-side
/// regime, or `None` when the located split is rejected for having too short a regime.
///
/// `ranks[i]` is the rank of the `i`-th value; on a permutation of `1..=n` that is the value
/// itself. Reproduces `cbh_stats::pettitt` (location, stats.rs:193-231) and its `MIN_REGIME`
/// rejection (findings.rs:1013-1020); the scorer then turns the located split into a p-value.
fn locate(ranks: &[f64]) -> Option<(usize, f64)> {
    let n = ranks.len();
    let last = n.checked_sub(1)?;
    if last == 0 {
        return None;
    }
    let n_f = count_f64(n as u64);

    // Pettitt: `U_t = 2·R_t − t·(n+1)`, `R_t` the prefix rank sum; the first `argmax|U_t|` wins,
    // exactly as `cbh_stats::pettitt` breaks ties (stats.rs:216, strict `>`). `t_f` tracks the
    // split position as an `f64` incrementally so the hot loop needs no per-iteration cast.
    let mut prefix = 0.0_f64;
    let mut t_f = 0.0_f64;
    let mut best_abs = -1.0_f64;
    let mut best_t = 1_usize;
    let mut rank_sum_left = 0.0_f64;
    for (position, &rank) in ranks.iter().enumerate() {
        prefix += rank;
        let t = position.saturating_add(1);
        if t > last {
            break;
        }
        t_f += 1.0;
        let u = 2.0 * prefix - t_f * (n_f + 1.0);
        let abs = u.abs();
        if abs > best_abs {
            best_abs = abs;
            best_t = t;
            rank_sum_left = prefix;
        }
    }

    // MinRegime: production locates over every position, then rejects on the shorter side
    // (findings.rs:1013-1020), so the filter is applied to the global argmax, not to eligible
    // positions only.
    let shorter = best_t.min(n.saturating_sub(best_t));
    if shorter < MIN_REGIME {
        return None;
    }
    Some((best_t, rank_sum_left))
}

/// The exact subset-count ceiling: above `2^53` an `f64` no longer holds consecutive integers, so
/// the knapsack's counts would round. Mirrors `cbh_stats::stats::EXACT_COUNT_LIMIT`.
const EXACT_COUNT_LIMIT: u128 = 1 << 53;

/// Whether the exact two-sided Mann–Whitney tail is representable in f64 for a split of these two
/// side sizes, i.e. `C(n1+n2, min(n1, n2)) < 2^53`.
///
/// Mirrors `cbh_stats::stats::exact_mw_feasible`, so the table scores each split exactly on
/// precisely the splits the detector does.
#[expect(
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    reason = "n >= k, so `n - k` cannot underflow; the running value is the integer binomial \
              C(n-k+i, i), so dividing by i is exact; and count stays below 2^53 until the early \
              return, keeping the product far inside u128"
)]
fn exact_mw_feasible(n1: usize, n2: usize) -> bool {
    let n = n1.saturating_add(n2);
    let k = n1.min(n2);
    let mut count: u128 = 1;
    for i in 1..=k {
        count = count * (n - k + i) as u128 / i as u128;
        if count >= EXACT_COUNT_LIMIT {
            return false;
        }
    }
    true
}

/// The largest smaller-side size whose subset count `C(n, k)` still fits f64's exact-integer range.
///
/// `C(n, k)` rises with `k` up to the balanced split, so the exact-feasible smaller sides are the
/// prefix `0..=k_lo`; this returns that `k_lo`. Splits with a smaller side above it are the
/// near-balanced ones the normal approximation must cover.
fn max_exact_side(n: usize) -> usize {
    let mut k_lo: usize = 0;
    let mut k: usize = 1;
    while k.saturating_mul(2) <= n && exact_mw_feasible(k, n.saturating_sub(k)) {
        k_lo = k;
        k = k.saturating_add(1);
    }
    k_lo
}

/// Scores the located split of a rank ordering with the p-value the production detector reports,
/// for one series length.
///
/// For a tie-free ordering of `1..=n` the joint ranking at any split is the whole set `1..=n`, so
/// the exact two-sided Mann–Whitney p-value depends only on the split size and its rank sum. The
/// scorer precomputes it per split size, turning each ordering's score into a table lookup — what
/// keeps an exact Monte Carlo row as cheap as the approximate one it replaces.
///
/// The exact tail counts the size-`k` subsets of `1..=n`, of which there are `C(n, k)`; f64 counts
/// them exactly only below `2^53`. `C(n, k)` peaks at the balanced split, so a split is exact
/// precisely when its *smaller* side `min(k, n − k)` is at most `k_lo`, the largest side whose
/// count still fits. The scorer tabulates the smaller-side subset sums up to `k_lo` and scores
/// every split from them — a smaller-side split from its own rank sum, a wide split through its
/// complementary smaller side — leaving only the near-balanced splits above `k_lo` to the normal
/// approximation. The choice is per split, not per series: a long series still earns the exact tail
/// at a lopsided split, exactly mirroring `cbh_stats::MannWhitneyU`.
pub(crate) struct SplitScorer {
    /// The series length.
    n: usize,
    /// The series length as `f64`, for the normal approximation's mean and variance.
    n_f: f64,
    /// The total rank sum `n·(n+1)/2`, mapping a wide split's left rank sum to its complementary
    /// smaller side's rank sum.
    total_rank_sum: f64,
    /// The largest smaller-side size still counted exactly in f64; splits whose smaller side is at
    /// most this are scored exactly, wider ones by the normal approximation.
    k_lo: usize,
    /// Per smaller-side size `k` in `MIN_REGIME..=k_lo`, the exact two-sided p indexed by that
    /// side's rank sum; other indices are empty.
    by_size: Vec<Vec<f64>>,
}

impl SplitScorer {
    /// Builds the scorer for length `n`.
    ///
    /// The exact table counts, for every smaller-side size `k` up to `k_lo` and rank sum `s`, the
    /// size-`k` subsets of `1..=n` summing to `s` (one shared cardinality knapsack over the items
    /// `1..=n`), then turns each column into the doubled minority tail — the same statistic
    /// `cbh_stats::MannWhitneyU` computes, reached by the same arithmetic so the two agree to the
    /// last bit. Enumerating only up to `k_lo` keeps every subset count at most `C(n, k_lo) < 2^53`,
    /// so the counts never leave f64's exact-integer range even for a long series.
    #[expect(
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::integer_division,
        reason = "the knapsack indices stay within the allocated `k_lo × max_sum` grid by \
                  construction: `size` runs `0..=k_lo`, `sum` runs `0..=max_sum`, and each access \
                  subtracts a value in `1..=n` from a sum at least that large, so no index or \
                  subtraction escapes; a size-`k_lo` subset sums to at most `k_lo·n`, and \
                  `k_lo·(k_lo−1)` is a product of consecutive integers, so halving it is exact"
    )]
    pub(crate) fn for_length(n: usize) -> Self {
        let n_f = count_f64(n as u64);
        let total_rank_sum = n_f * (n_f + 1.0) / 2.0;
        let k_lo = max_exact_side(n);

        // `counts[size][sum]` counts the size-`size` subsets of the values seen so far that sum to
        // `sum`. Folding the values `1..=n` in one at a time while walking `size` and `sum` downward
        // spends each value at most once per subset (the 0/1-knapsack order). Only the smaller-side
        // sizes `0..=k_lo` are built; a size-`k_lo` subset reaches at most the sum of the `k_lo`
        // largest values, which bounds the sum axis.
        let max_sum = k_lo * n - k_lo * (k_lo - 1) / 2;
        let mut counts: Vec<Vec<f64>> = vec![vec![0.0_f64; max_sum + 1]; k_lo + 1];
        counts[0][0] = 1.0;
        for value in 1..=n {
            for size in (1..=k_lo).rev() {
                for sum in (value..=max_sum).rev() {
                    counts[size][sum] += counts[size - 1][sum - value];
                }
            }
        }

        // A smaller-side split leaves `n − k ≥ n − k_lo ≥ MIN_REGIME` on the other side, so every
        // size `MIN_REGIME..=k_lo` is reportable; wider splits are scored through their complement.
        let mut by_size: Vec<Vec<f64>> = vec![Vec::new(); k_lo + 1];
        for size in MIN_REGIME..=k_lo {
            by_size[size] = tail_p_by_sum(&counts[size]);
        }
        Self {
            n,
            n_f,
            total_rank_sum,
            k_lo,
            by_size,
        }
    }

    /// The p-value the production procedure reports for one rank ordering, or `None` when the
    /// located split is rejected for having too short a regime.
    ///
    /// A split whose smaller side is at most `k_lo` is scored exactly — a smaller-side split from
    /// its own rank sum, a wide split from its complementary smaller side's rank sum (the total
    /// less the left sum). The near-balanced splits above `k_lo` fall to the normal approximation,
    /// the same per-split choice `cbh_stats::MannWhitneyU` makes.
    #[expect(
        clippy::indexing_slicing,
        reason = "an exact branch reads `by_size[k]` only for a smaller-side size `k` in \
                  `MIN_REGIME..=k_lo`, a populated row, at `[sum_index]` of a real subset's rank \
                  sum — both in range by construction"
    )]
    pub(crate) fn located_split_p(&self, ranks: &[f64]) -> Option<f64> {
        let (best_t, rank_sum_left) = locate(ranks)?;
        let p = if best_t <= self.k_lo {
            self.by_size[best_t][sum_index(rank_sum_left)]
        } else if best_t >= self.n.saturating_sub(self.k_lo) {
            let small = self.n.saturating_sub(best_t);
            self.by_size[small][sum_index(self.total_rank_sum - rank_sum_left)]
        } else {
            normal_split_p(self.n_f, best_t, rank_sum_left)
        };
        Some(p)
    }
}

/// The doubled minority tail for every left rank sum of one split size.
///
/// `counts[s]` is the number of subsets whose rank sum is `s`; the two-sided p at an observed `s`
/// is `2 · min(P(sum ≤ s), P(sum ≥ s))`, capped at one and floored like the production clamp. The
/// running counts are exact integers below `2^53`, so the reals here equal
/// `cbh_stats::stats::exact_two_sided_p`'s to the last bit.
#[expect(
    clippy::indexing_slicing,
    reason = "`p` is allocated to `counts.len()` and `sum` is its own enumeration index, so the \
              write is in range by construction"
)]
fn tail_p_by_sum(counts: &[f64]) -> Vec<f64> {
    let total: f64 = counts.iter().sum();
    let mut cumulative = 0.0_f64;
    let mut p = vec![0.0_f64; counts.len()];
    for (sum, &count) in counts.iter().enumerate() {
        cumulative += count;
        let lower = cumulative;
        let upper = total - cumulative + count;
        p[sum] = if total > 0.0 {
            clamp_p_value((2.0 * lower.min(upper) / total).min(1.0))
        } else {
            1.0
        };
    }
    p
}

/// The array index for a left rank sum, which is an exact non-negative integer.
fn sum_index(rank_sum_left: f64) -> usize {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a left rank sum of integer ranks is a non-negative integer far below usize::MAX, \
                  so the rounded cast is exact"
    )]
    let index = rank_sum_left.round() as usize;
    index
}

/// The tie-free normal-approximation two-sided p at a located split.
///
/// Reproduces `cbh_stats::MannWhitneyU::two_sided_p_value` for a permutation of `1..=n`, where
/// every tie group is a singleton so the tie term is identically zero.
fn normal_split_p(n_f: f64, best_t: usize, rank_sum_left: f64) -> f64 {
    let n1 = count_f64(best_t as u64);
    let n2 = n_f - n1;
    let u1 = rank_sum_left - n1 * (n1 + 1.0) / 2.0;
    let u2 = n1 * n2 - u1;
    let u = u1.min(u2);
    let mean_u = n1 * n2 / 2.0;
    let variance = (n1 * n2 / 12.0) * (n_f + 1.0);
    let z = ((mean_u - u) - 0.5).max(0.0) / variance.sqrt();
    two_sided_p_from_z(z)
}

/// The empirical distribution of finding p-values for one series length.
///
/// It answers the two questions the ladder builder (`derive.rs`) asks: how many of the `total`
/// orderings produced a finding, and what the `k`-th smallest finding p-value is. Exact rows carry
/// a compressed `(p, cumulative-count)` table because a short history has only a few hundred
/// distinct p-values however many orderings realize them; Monte Carlo rows carry the sorted sample
/// because at large `n` the distinct values are too many to compress usefully.
pub(crate) enum FindingDist {
    /// A Monte Carlo row: every finding p-value, sorted ascending. `total` is the sample count.
    Samples {
        /// The finding p-values, ascending.
        sorted: Vec<f64>,
        /// The number of orderings sampled.
        total: u64,
    },
    /// An exact row: distinct finding p-values ascending, each paired with the running count of
    /// orderings whose p-value is at most it.
    Counts {
        /// Distinct finding p-values ascending, each with the count of orderings at most it.
        cumulative: Vec<(f64, u64)>,
        /// The number of orderings that produced a finding.
        findings: u64,
        /// The number of orderings evaluated, `n!`.
        total: u64,
    },
}

impl FindingDist {
    /// The number of orderings evaluated (the ladder's denominator).
    pub(crate) fn total(&self) -> u64 {
        match self {
            Self::Samples { total, .. } | Self::Counts { total, .. } => *total,
        }
    }

    /// How many of those orderings produced a finding.
    pub(crate) fn findings(&self) -> u64 {
        match self {
            Self::Samples { sorted, .. } => sorted.len() as u64,
            Self::Counts { findings, .. } => *findings,
        }
    }

    /// The largest finding p-value whose entire equal-valued mass fits within the first `limit`
    /// findings, or `0.0` when even the smallest finding value already spills past it.
    ///
    /// This is the honest conservative inverse of the finding CDF. Mann–Whitney p-values are
    /// heavily discrete, so many orderings share one value; a rung may claim that value only once
    /// *all* of its orderings fit under the rung's allowance. Crediting it the moment its first
    /// ordering fit would let the rung report more significance than the value truly commands,
    /// breaking `P(adjusted <= a) <= a` (the anti-conservative failure the ladder must avoid).
    #[expect(
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        reason = "every index below is guarded: `partition_point` returns a count in `0..=len`, and \
                  each subtraction is applied only after a `> 0` / `< len` check, so it stays in range"
    )]
    pub(crate) fn largest_within(&self, limit: u64) -> f64 {
        match self {
            Self::Samples { sorted, .. } => {
                let limit = usize::try_from(limit).expect("a finding count fits usize");
                if limit == 0 {
                    return 0.0;
                }
                if limit >= sorted.len() {
                    return sorted.last().copied().unwrap_or(0.0);
                }
                let candidate = sorted[limit - 1];
                // `candidate`'s tied block may run past `limit`; if it does, the value does not fit
                // and the answer is the largest value strictly below it, whose block ends earlier.
                let at_or_below = sorted.partition_point(|&value| value <= candidate);
                if at_or_below <= limit {
                    candidate
                } else {
                    let below = sorted.partition_point(|&value| value < candidate);
                    if below == 0 { 0.0 } else { sorted[below - 1] }
                }
            }
            Self::Counts { cumulative, .. } => {
                // Each entry's running count already includes its full mass, so the last entry
                // whose running count is within `limit` is exactly the largest value that fits.
                let count = cumulative.partition_point(|&(_, running)| running <= limit);
                if count == 0 {
                    0.0
                } else {
                    cumulative[count - 1].0
                }
            }
        }
    }
}

/// The exact finding distribution for length `n`, by enumerating all `n!` orderings.
///
/// Feasible only for small `n` (the caller restricts it to `n <= EXACT_MAX_N`). Counts by p-value
/// bits, so its memory is the number of *distinct* p-values — a few hundred — not `n!`.
pub(crate) fn collect_exact(n: usize) -> FindingDist {
    let scorer = SplitScorer::for_length(n);
    let mut counts: HashMap<u64, u64> = HashMap::new();
    let mut total = 0_u64;
    let mut ranks: Vec<f64> = (1..=n).map(|value| count_f64(value as u64)).collect();
    for_each_permutation(&mut ranks, |permutation| {
        total = total.saturating_add(1);
        if let Some(p) = scorer.located_split_p(permutation) {
            counts
                .entry(p.to_bits())
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
        }
    });

    let mut cumulative: Vec<(f64, u64)> = counts
        .into_iter()
        .map(|(bits, count)| (f64::from_bits(bits), count))
        .collect();
    cumulative.sort_by(|left, right| left.0.total_cmp(&right.0));
    let mut running = 0_u64;
    for entry in &mut cumulative {
        running = running.saturating_add(entry.1);
        entry.1 = running;
    }
    FindingDist::Counts {
        cumulative,
        findings: running,
        total,
    }
}

/// The Monte Carlo finding distribution for length `n` over `samples` random orderings.
///
/// The stream is a pure function of `seed`, so the row reproduces bit-for-bit on every platform.
pub(crate) fn collect_monte_carlo(n: usize, samples: u64, seed: u64) -> FindingDist {
    let scorer = SplitScorer::for_length(n);
    let mut rng = SplitMix64::new(seed);
    let mut ranks: Vec<f64> = (1..=n).map(|value| count_f64(value as u64)).collect();
    let mut sorted: Vec<f64> = Vec::new();
    for _ in 0..samples {
        shuffle(&mut ranks, &mut rng);
        if let Some(p) = scorer.located_split_p(&ranks) {
            sorted.push(p);
        }
    }
    sorted.sort_by(f64::total_cmp);
    FindingDist::Samples {
        sorted,
        total: samples,
    }
}

/// The seed for a row, a deterministic FNV-1a hash of a per-row label.
///
/// Distinct rows draw unrelated `splitmix64` streams, so the rows are independent rather than
/// copies of one sequence.
pub(crate) fn row_seed(n: usize) -> u64 {
    let label = format!("cbh-calibration-row-{n}");
    label.bytes().fold(FNV_OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
    })
}

/// A `splitmix64` generator, the workspace's committed-constant RNG (`scatter.rs:33-64`).
///
/// A fixed algorithm with committed constants reproduces forever; a random crate's stream is not
/// guaranteed stable across versions, which a checked-in table cannot tolerate.
#[derive(Debug)]
struct SplitMix64 {
    /// The counter the finalizer is applied to; the seed is its starting value.
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(SPLITMIX_GAMMA);
        let mut mixed = self.state;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(SPLITMIX_MIX_A);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(SPLITMIX_MIX_B);
        mixed ^ (mixed >> 31)
    }
}

/// Fisher–Yates shuffle of `values` in place.
///
/// The `% bound` index has a bias of order `bound / 2^64`, which at `bound <= 1000` is below
/// `1e-16` and cannot move a tabulated value; using it keeps the stream a pure function of the
/// seed on every platform.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "`index + 1` is at most values.len() <= MAX_SERIES_LEN and `% bound` has bound >= 2 \
              since index >= 1, so neither can overflow or divide by zero"
)]
fn shuffle(values: &mut [f64], rng: &mut SplitMix64) {
    for index in (1..values.len()).rev() {
        let bound = index as u64 + 1;
        let pick = usize::try_from(rng.next_u64() % bound).expect("a value below bound fits usize");
        values.swap(index, pick);
    }
}

/// Calls `visit` once for every permutation of `values`, via Heap's algorithm.
#[expect(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "`counters` has length `n` and `index` stays in `0..n` by the algorithm's invariant, \
              so every index is valid and `counters[index] += 1` and `index += 1` cannot overflow"
)]
fn for_each_permutation(values: &mut [f64], mut visit: impl FnMut(&[f64])) {
    let n = values.len();
    visit(values);
    let mut counters = vec![0_usize; n];
    let mut index = 0_usize;
    while index < n {
        if counters[index] < index {
            let swap_with = if index.is_multiple_of(2) {
                0
            } else {
                counters[index]
            };
            values.swap(swap_with, index);
            visit(values);
            counters[index] += 1;
            index = 0;
        } else {
            counters[index] = 0;
            index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MIN_SERIES_LEN;

    /// The production change-point procedure the kernel reproduces: locate with Pettitt, reject a
    /// too-short regime, then the two-sided Mann–Whitney p-value at the located split
    /// (`cbh_detect` findings.rs:1004-1033).
    fn production_located_split_p(values: &[f64]) -> Option<f64> {
        let n = values.len();
        let change = cbh_stats::pettitt(values)?;
        let tau = change.index;
        let shorter = tau.min(
            n.checked_sub(tau)
                .expect("Pettitt locates a split within the series"),
        );
        if shorter < MIN_REGIME {
            return None;
        }
        let before = values.get(..tau)?;
        let after = values.get(tau..)?;
        cbh_stats::MannWhitneyU::new(before, after).map(|ranked| ranked.two_sided_p_value())
    }

    /// The self-contained kernel must agree with the production primitives on every ordering, or
    /// the committed table would describe a procedure the detector does not run. A permutation of
    /// `1..=n` is tie-free, so its average ranks are the values themselves and the two paths are
    /// expected to agree to the last bit; a tiny tolerance absorbs only reassociation, not a real
    /// divergence.
    #[test]
    fn matches_production_over_random_permutations() {
        // The lengths straddle both regimes the kernel switches on: `EXACT_MAX_N` (exact
        // enumeration vs Monte Carlo) and the per-split exactness cutoff (the exact tail at a
        // feasible split, the normal approximation at a near-balanced wide one), so a drift in
        // either between the kernel and production would surface here.
        for n in [MIN_SERIES_LEN, 11, 17, 32, 55, 56, 57, 58, 64, 200] {
            let scorer = SplitScorer::for_length(n);
            let mut ranks: Vec<f64> = (1..=n).map(|value| count_f64(value as u64)).collect();
            let mut rng = SplitMix64::new(row_seed(n));
            for _ in 0..2_000 {
                shuffle(&mut ranks, &mut rng);
                match (
                    scorer.located_split_p(&ranks),
                    production_located_split_p(&ranks),
                ) {
                    (None, None) => {}
                    (Some(ours), Some(theirs)) => assert!(
                        (ours - theirs).abs() <= 1e-12,
                        "n={n}: kernel p {ours} disagrees with production p {theirs}"
                    ),
                    (ours, theirs) => {
                        panic!(
                            "n={n}: eligibility disagreed: kernel {ours:?} vs production {theirs:?}"
                        )
                    }
                }
            }
        }
    }
}
