//! Selection-adjusted change-point significance.
//!
//! A change-point detector searches the whole history and reports the split
//! that looks strongest. The Mann-Whitney p-value at that chosen split is
//! therefore tainted by the search: even unchanged histories occasionally have
//! one unusually convincing split.
//!
//! [`selection_adjusted_change_point`] measures the whole procedure against
//! shuffled orderings of the series' actual values. Every shuffle retains the
//! same values and ties, runs the same Pettitt first-maximum split selection,
//! applies the same minimum-regime rule, and scores the accepted split with the
//! same exact-or-normal Mann-Whitney implementation. The resulting conditional
//! permutation p-value therefore includes both split selection and the series'
//! actual tie pattern.

mod permutation;

use std::num::NonZero;

use permutation::{SplitMix64, permutation_seed, shuffle};

use crate::{
    NO_EVIDENCE, exact_mw_feasible, exact_rank_sum_p_values, mann_whitney_tie_term,
    normal_mann_whitney_p, pettitt_rank_location, scaled_average_ranks,
};

/// Largest integer through which binary64 represents every integer exactly.
const MAX_EXACT_F64_INTEGER: u128 = 1 << f64::MANTISSA_DIGITS;

/// A located change point with its selected and selection-adjusted statistics.
///
/// This is the complete statistical handoff needed by the detector: the split
/// locates the regimes, `tainted_p` explains the adjustment in diagnostics,
/// `adjusted_p` enters the significance and family filters, and `superiority`
/// drives the practical regime-separation gate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionAdjustedChangePoint {
    /// Index where the after regime begins.
    pub index: usize,
    /// Mann-Whitney p-value at the split selected from the observed ordering.
    ///
    /// This is not an honest standalone p-value because the split was selected
    /// by searching the same data.
    pub tainted_p: f64,
    /// Conditional permutation p-value for the complete split-selection procedure.
    pub adjusted_p: f64,
    /// Probability that an after-regime value exceeds a before-regime value,
    /// counting ties as one half.
    pub superiority: f64,
}

/// Scores one rank ordering with the production change-point procedure.
///
/// Rank-multiset properties are cached once, including the tie correction and,
/// on first use, exact rank-sum tails for every feasible split size.
struct SplitScorer {
    min_regime: usize,
    total_scaled_rank_sum: usize,
    tie_term: f64,
    max_exact_size: usize,
    sorted_ranks: Vec<usize>,
    exact_p_by_size: Option<Vec<Vec<f64>>>,
}

/// The score and effect size at one ordering's selected reportable split.
#[derive(Clone, Copy, Debug)]
struct SelectedScore {
    index: usize,
    p: f64,
    superiority: f64,
}

impl SplitScorer {
    fn new(sorted_ranks: Vec<usize>, min_regime: usize) -> Self {
        let n = sorted_ranks.len();
        let max_split_size = n.checked_div(2).expect("the divisor is nonzero");
        let max_exact_size = (min_regime..=max_split_size)
            .take_while(|&size| exact_mw_feasible(size, n.saturating_sub(size)))
            .last()
            .unwrap_or(0);
        let total_scaled_rank_sum = sorted_ranks.iter().copied().sum();
        let tie_term = mann_whitney_tie_term(&sorted_ranks);
        Self {
            min_regime,
            total_scaled_rank_sum,
            tie_term,
            max_exact_size,
            sorted_ranks,
            exact_p_by_size: None,
        }
    }

    fn score(&mut self, ranks: &[usize]) -> Option<SelectedScore> {
        let n = ranks.len();
        let (index, scaled_rank_sum_left, _) = pettitt_rank_location(ranks)?;
        let right_len = n.checked_sub(index)?;
        let smaller_side = index.min(right_len);
        if smaller_side < self.min_regime {
            return None;
        }

        let rank_sum_left = count_to_f64(scaled_rank_sum_left) / 2.0;
        let left_len_f = count_to_f64(index);
        let right_len_f = count_to_f64(right_len);
        let u1 = rank_sum_left - left_len_f * (left_len_f + 1.0) / 2.0;
        let u2 = left_len_f * right_len_f - u1;
        let superiority = u2 / (left_len_f * right_len_f);

        let p = if smaller_side <= self.max_exact_size {
            let observed_sum = if index <= right_len {
                scaled_rank_sum_left
            } else {
                self.total_scaled_rank_sum
                    .saturating_sub(scaled_rank_sum_left)
            };
            let exact = self.exact_p_by_size.get_or_insert_with(|| {
                exact_rank_sum_p_values(&self.sorted_ranks, self.max_exact_size)
            });
            exact
                .get(smaller_side)
                .and_then(|row| row.get(observed_sum))
                .copied()
                .unwrap_or(NO_EVIDENCE)
        } else {
            normal_mann_whitney_p(index, right_len, rank_sum_left, self.tie_term)
        };

        Some(SelectedScore {
            index,
            p,
            superiority,
        })
    }
}

/// Locates and selection-adjusts a change point by conditional permutation.
///
/// `permutations` is the fixed Monte Carlo budget. It must leave room for the
/// plus-one correction in `usize`, and that corrected denominator must be no
/// greater than the largest integer represented exactly by `f64`.
/// `reject_at_or_above` is the largest adjusted p-value that could still pass
/// the caller's next gate. The function may stop early and return `1.0` once
/// the final fixed-budget p-value cannot fall below that boundary; this only
/// rejects work that cannot affect the verdict.
///
/// A shuffled ordering whose selected split violates `min_regime` contributes
/// the no-evidence score `1.0` and remains in the denominator. The reported
/// Monte Carlo value uses the standard plus-one correction
/// `(1 + extreme) / (1 + permutations)`, then is clamped to be no smaller than
/// the selected Mann-Whitney score.
///
/// Returns `None` when Pettitt cannot locate a split or the selected split does
/// not leave `min_regime` values on each side.
///
/// # Panics
///
/// Panics if `min_regime` is zero, `reject_at_or_above` is outside `(0, 1]`,
/// or the permutation budget cannot be incremented and represented exactly as
/// an `f64` denominator.
#[must_use]
pub fn selection_adjusted_change_point(
    values: &[f64],
    min_regime: usize,
    permutations: NonZero<usize>,
    reject_at_or_above: f64,
) -> Option<SelectionAdjustedChangePoint> {
    assert!(min_regime > 0, "a regime must contain at least one value");
    assert!(
        reject_at_or_above > 0.0 && reject_at_or_above <= 1.0,
        "the rejection boundary must be in (0, 1]"
    );
    assert!(
        permutations.get() < usize::MAX,
        "the permutation budget must leave room for the plus-one correction"
    );
    let total = permutations
        .get()
        .checked_add(1)
        .expect("the validated permutation budget leaves room for plus one");
    assert!(
        total as u128 <= MAX_EXACT_F64_INTEGER,
        "the plus-one permutation denominator must be exactly representable as f64"
    );

    let observed_ranks = scaled_average_ranks(values);
    let mut sorted_ranks = observed_ranks.clone();
    sorted_ranks.sort_unstable();
    let seed = permutation_seed(values, &sorted_ranks, min_regime);
    let mut scorer = SplitScorer::new(sorted_ranks.clone(), min_regime);
    let observed = scorer.score(&observed_ranks)?;

    // Selection adjustment must never make the selected split look more
    // significant. This also makes an observed score already outside the next
    // gate an immediate rejection without any shuffling.
    if observed.p >= reject_at_or_above {
        return Some(result(observed, NO_EVIDENCE));
    }

    let total_f = count_to_f64(total);
    let mut rng = SplitMix64::new(seed);
    let mut shuffled = sorted_ranks;
    let mut extreme = 0_usize;
    for _ in 0..permutations.get() {
        shuffle(&mut shuffled, &mut rng);
        let shuffled_p = scorer.score(&shuffled).map_or(NO_EVIDENCE, |score| score.p);
        if shuffled_p <= observed.p {
            extreme = extreme.saturating_add(1);
            let smallest_final_p = count_to_f64(extreme.saturating_add(1)) / total_f;
            if smallest_final_p >= reject_at_or_above {
                return Some(result(observed, NO_EVIDENCE));
            }
        }
    }

    let adjusted = count_to_f64(extreme.saturating_add(1)) / total_f;
    Some(result(observed, adjusted.max(observed.p).min(NO_EVIDENCE)))
}

fn result(score: SelectedScore, adjusted_p: f64) -> SelectionAdjustedChangePoint {
    SelectionAdjustedChangePoint {
        index: score.index,
        tainted_p: score.p,
        adjusted_p,
        superiority: score.superiority,
    }
}

/// Casts a bounded count to `f64`.
#[expect(
    clippy::cast_precision_loss,
    reason = "series lengths are bounded and permutation denominators are validated at or below 2^53"
)]
fn count_to_f64(count: usize) -> f64 {
    count as f64
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "the shared scorer must reproduce the production primitives bit-for-bit"
    )]
    #![allow(clippy::indexing_slicing, reason = "panic is acceptable in tests")]

    use super::*;
    use crate::{MannWhitneyU, pettitt};

    #[test]
    fn selected_score_matches_production_primitives_with_ties() {
        let exact = vec![1.0, 1.0, 2.0, 1.0, 5.0, 5.0, 5.0, 2.0, 5.0, 5.0];
        let normal = [vec![1.0; 50], vec![5.0; 50]].concat();
        for (values, min_regime) in [(exact, 2), (normal, 5)] {
            let ranks = scaled_average_ranks(&values);
            let mut sorted = ranks.clone();
            sorted.sort_unstable();
            let mut scorer = SplitScorer::new(sorted, min_regime);
            let selected = scorer.score(&ranks).expect("the split is reportable");
            let located = pettitt(&values).expect("Pettitt locates a split");
            assert_eq!(selected.index, located.index);

            let before = &values[..selected.index];
            let after = &values[selected.index..];
            let mann_whitney = MannWhitneyU::new(before, after).expect("both regimes are nonempty");
            assert_eq!(selected.p, mann_whitney.two_sided_p_value());
            assert_eq!(selected.superiority, mann_whitney.superiority());
        }
    }

    #[test]
    fn scorer_accepts_a_split_at_the_minimum_regime_boundary() {
        let values = [vec![1.0; 5], vec![2.0; 5]].concat();
        let ranks = scaled_average_ranks(&values);
        let mut sorted = ranks.clone();
        sorted.sort_unstable();
        let score = SplitScorer::new(sorted, 5)
            .score(&ranks)
            .expect("both regimes meet the minimum exactly");
        assert_eq!(score.index, 5);
    }

    #[test]
    fn calibration_is_reproducible() {
        let values = [10.0, 11.0, 10.0, 12.0, 11.0, 30.0, 31.0, 30.0, 32.0, 31.0];
        let budget = NonZero::new(2_000).expect("the test budget is nonzero");
        let first = selection_adjusted_change_point(&values, 5, budget, 0.025);
        let second = selection_adjusted_change_point(&values, 5, budget, 0.025);
        assert_eq!(first, second);
    }

    #[test]
    #[should_panic(expected = "must leave room for the plus-one correction")]
    fn maximum_usize_permutation_budget_is_rejected() {
        let budget = NonZero::new(usize::MAX).expect("usize::MAX is nonzero");
        let _result = selection_adjusted_change_point(&[1.0, 2.0], 1, budget, 0.5);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    #[should_panic(expected = "denominator must be exactly representable as f64")]
    fn inexact_permutation_denominator_is_rejected() {
        let budget = NonZero::new(
            usize::try_from(MAX_EXACT_F64_INTEGER)
                .expect("the binary64 exact-integer limit fits 64-bit usize"),
        )
        .expect("the binary64 exact-integer limit is nonzero");
        let _result = selection_adjusted_change_point(&[1.0, 2.0], 1, budget, 0.5);
    }

    #[test]
    fn clean_tied_step_survives_selection_adjustment() {
        let mut values = vec![10.0; 10];
        values.extend(std::iter::repeat_n(20.0, 10));
        let adjusted = selection_adjusted_change_point(
            &values,
            5,
            NonZero::new(2_000).expect("the test budget is nonzero"),
            0.025,
        )
        .expect("the clean middle split is reportable");
        assert_eq!(adjusted.index, 10);
        assert!(adjusted.adjusted_p < 0.025, "{adjusted:?}");
        assert!(adjusted.adjusted_p >= adjusted.tainted_p);
        assert_eq!(adjusted.superiority, 1.0);
    }

    #[test]
    fn reportable_tied_step_is_not_rejected_by_early_stopping() {
        // Of the C(12, 6) tied orderings, the two fully separated orders are at
        // least as extreme as this one. The adjusted chance level therefore stays
        // below the detector boundary while still encountering extreme shuffles.
        let values = [vec![10.0; 6], vec![20.0; 6]].concat();
        let adjusted = selection_adjusted_change_point(
            &values,
            5,
            NonZero::new(2_000).expect("the test budget is nonzero"),
            0.025,
        )
        .expect("the clean middle split is reportable");
        assert!(adjusted.adjusted_p < 0.025, "{adjusted:?}");
    }

    #[test]
    fn tied_null_distribution_is_conservative() {
        // Six repeated low values and six repeated high values have 924 distinct
        // temporal orderings. Enumerating all of them exercises the tie pattern
        // that a length-only, tie-free calibration cannot represent.
        let mut scores = Vec::new();
        let sorted_ranks = scaled_average_ranks(&[vec![1.0; 6], vec![2.0; 6]].concat());
        let low_rank = sorted_ranks
            .first()
            .copied()
            .expect("the ranks are nonempty");
        let high_rank = sorted_ranks
            .last()
            .copied()
            .expect("the ranks are nonempty");
        let mut scorer = SplitScorer::new(sorted_ranks, 5);
        let mut unreportable = 0_usize;
        for mask in 0_u16..(1_u16 << 12) {
            if mask.count_ones() != 6 {
                continue;
            }
            let ranks: Vec<usize> = (0..12)
                .map(|index| {
                    if mask & (1 << index) == 0 {
                        low_rank
                    } else {
                        high_rank
                    }
                })
                .collect();
            match scorer.score(&ranks) {
                Some(score) => scores.push(score.p),
                None => {
                    unreportable = unreportable.saturating_add(1);
                    scores.push(NO_EVIDENCE);
                }
            }
        }
        assert!(
            unreportable > 0,
            "the denominator must include rejected splits"
        );

        let total = count_to_f64(scores.len());
        let adjusted: Vec<f64> = scores
            .iter()
            .map(|&observed| {
                let at_least_as_extreme = scores.iter().filter(|&&score| score <= observed).count();
                (count_to_f64(at_least_as_extreme) / total).max(observed)
            })
            .collect();
        let mut levels = adjusted.clone();
        levels.sort_unstable_by(f64::total_cmp);
        levels.dedup_by(|left, right| left.total_cmp(right).is_eq());
        for level in levels {
            let reported = adjusted.iter().filter(|&&value| value <= level).count();
            assert!(
                count_to_f64(reported) <= level * total + f64::EPSILON,
                "P(adjusted <= {level}) exceeded {level} for a tied null"
            );
        }
    }

    #[test]
    fn unreportable_selected_split_has_no_adjusted_result() {
        let values = [1.0; 6];
        assert_eq!(
            selection_adjusted_change_point(
                &values,
                3,
                NonZero::new(100).expect("the test budget is nonzero"),
                0.025,
            ),
            None
        );
    }
}
