//! The null-distribution procedure, evaluated on rank orderings.
//!
//! Under the null hypothesis (no real level shift) a series' values matter only through their
//! rank ordering, and every ordering of `n` distinct values is equally likely. So the
//! selection-adjusted p-value is a property of `n` alone: the distribution, over all `n!`
//! orderings, of the p-value the production change-point procedure reports.
//!
//! That procedure is: locate the split with **Pettitt** (over every interior position), reject it
//! unless the shorter side has at least [`MIN_REGIME`] points, and otherwise score it with the
//! two-sided **Mann–Whitney** p-value at that split. On a permutation of `1..=n` the ranks *are*
//! the values, so both steps close to O(n) arithmetic with no ties — which is what makes tabulating
//! every `n` up to a thousand affordable. The closed form here is proven equal to the production
//! `cbh_stats` primitives by the crate's `matches_production` test, so the table cannot drift from
//! the code it describes.

use std::collections::HashMap;

use crate::normal::two_sided_p_from_z;
use crate::{
    FNV_OFFSET_BASIS, FNV_PRIME, MIN_REGIME, SPLITMIX_GAMMA, SPLITMIX_MIX_A, SPLITMIX_MIX_B,
    count_f64,
};

/// The p-value the production change-point procedure reports for one rank ordering, or `None`
/// when the located split is rejected for having too short a regime.
///
/// `ranks[i]` is the rank of the `i`-th value; on a permutation of `1..=n` that is the value
/// itself. Reproduces `cbh_stats::pettitt` (location, stats.rs:193-231) followed by
/// `cbh_stats::MannWhitneyU` (significance, stats.rs:307-320) with the tie term identically zero.
pub(crate) fn located_split_p(ranks: &[f64]) -> Option<f64> {
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

    // Mann–Whitney at the located split. The joint ranking of the two sides is the whole
    // permutation, so `rank_sum_left` is already the left rank sum and every tie group is a
    // singleton (`tie_term = 0`).
    let n1 = count_f64(best_t as u64);
    let n2 = count_f64(n.saturating_sub(best_t) as u64);
    let u1 = rank_sum_left - n1 * (n1 + 1.0) / 2.0;
    let u2 = n1 * n2 - u1;
    let u = u1.min(u2);
    let mean_u = n1 * n2 / 2.0;
    let variance = (n1 * n2 / 12.0) * (n_f + 1.0);
    let z = ((mean_u - u) - 0.5).max(0.0) / variance.sqrt();
    Some(two_sided_p_from_z(z))
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

    /// The `k`-th smallest finding p-value, `1 <= k <= findings()`.
    #[expect(
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        reason = "k is in 1..=findings() (asserted); `k - 1` and the partition index are valid \
                  positions into a slice of that length"
    )]
    pub(crate) fn nth_smallest(&self, k: u64) -> f64 {
        debug_assert!(k >= 1 && k <= self.findings(), "quantile rank out of range");
        match self {
            Self::Samples { sorted, .. } => {
                let index = usize::try_from(k - 1).expect("finding rank fits usize");
                sorted[index]
            }
            Self::Counts { cumulative, .. } => {
                // The smallest p whose running count reaches `k`.
                let index = cumulative.partition_point(|&(_, running)| running < k);
                cumulative[index].0
            }
        }
    }
}

/// The exact finding distribution for length `n`, by enumerating all `n!` orderings.
///
/// Feasible only for small `n` (the caller restricts it to `n <= EXACT_MAX_N`). Counts by p-value
/// bits, so its memory is the number of *distinct* p-values — a few hundred — not `n!`.
pub(crate) fn collect_exact(n: usize) -> FindingDist {
    let mut counts: HashMap<u64, u64> = HashMap::new();
    let mut total = 0_u64;
    let mut ranks: Vec<f64> = (1..=n).map(|value| count_f64(value as u64)).collect();
    for_each_permutation(&mut ranks, |permutation| {
        total = total.saturating_add(1);
        if let Some(p) = located_split_p(permutation) {
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
    let mut rng = SplitMix64::new(seed);
    let mut ranks: Vec<f64> = (1..=n).map(|value| count_f64(value as u64)).collect();
    let mut sorted: Vec<f64> = Vec::new();
    for _ in 0..samples {
        shuffle(&mut ranks, &mut rng);
        if let Some(p) = located_split_p(&ranks) {
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
            let swap_with = if index.is_multiple_of(2) { 0 } else { counters[index] };
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
        let shorter = tau.min(n.checked_sub(tau).expect("Pettitt locates a split within the series"));
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
        for n in [MIN_SERIES_LEN, 11, 17, 32, 64, 200] {
            let mut ranks: Vec<f64> = (1..=n).map(|value| count_f64(value as u64)).collect();
            let mut rng = SplitMix64::new(row_seed(n));
            for _ in 0..2_000 {
                shuffle(&mut ranks, &mut rng);
                match (located_split_p(&ranks), production_located_split_p(&ranks)) {
                    (None, None) => {}
                    (Some(ours), Some(theirs)) => assert!(
                        (ours - theirs).abs() <= 1e-12,
                        "n={n}: kernel p {ours} disagrees with production p {theirs}"
                    ),
                    (ours, theirs) => {
                        panic!("n={n}: eligibility disagreed: kernel {ours:?} vs production {theirs:?}")
                    }
                }
            }
        }
    }
}
