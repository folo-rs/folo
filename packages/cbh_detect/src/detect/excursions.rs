//! Isolated measurement excursions in a branch-mode comparison window.
//!
//! A shared CI runner occasionally has a bad moment — another tenant takes the cores —
//! and one commit's measurement lands far above the level its neighbours agree on. The
//! reading describes the runner, not the code, and it does not belong in the evidence
//! branch mode judges a pull request against.
//!
//! Leaving it there is not merely untidy. Branch mode measures how far a context run
//! sits from the base level in units of the window's own scatter (see
//! [`prediction_interval_p`](super::findings)), so one wild reading both drags the
//! window's centre toward it and inflates its scatter several-fold. The window then
//! finds nothing surprising, and a genuine regression passes unreported — the failure is
//! silent, which is what makes it worth removing rather than tolerating.
//!
//! History mode does not use this. Its gates take their level and their scatter from
//! medians, which absorb a single wild reading on their own, so cleaning would change
//! the evidence without improving the verdict.
//!
//! What counts as such a reading is deliberately narrow, because discarding a level
//! *tightens* the window and so makes branch mode readier to report: a rule that removes
//! too much manufactures the false positives the analysis was narrowed to eliminate.
//! Three tests must all hold, and [`isolated_excursions`] documents what each defends
//! against.

use std::borrow::Cow;

use cbh_stats as stats;

use super::findings::relative_delta_of;
use super::noise_gates;
use super::series::BaseLevel;

/// One base-window reading branch mode left out of a comparison.
///
/// Discarding a reading changes a verdict without declining anything, so no gate records
/// it and it would otherwise be invisible. This carries what a reader needs to reconstruct
/// the decision — which commit, what it measured, and the level its neighbours agreed on
/// that it stood clear of — so `--verbose` can explain a comparison that silently narrowed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiscardedReading {
    /// First-parent topological index of the base-ref commit the reading came from.
    pub topo_index: usize,
    /// The value that was left out.
    pub value: f64,
    /// The level its neighbours on both sides agreed on, which it stood clear of.
    pub neighbour_level: f64,
}

/// `window` with its isolated measurement excursion removed, oldest first, and what was
/// removed.
///
/// `context_level` is the level the context run sits at, which is evidence about the
/// window: a candidate the context run agrees with is a level this series reaches rather
/// than a moment its runner lost.
///
/// Borrows `window` unchanged in the ordinary case, which is every window that holds no
/// excursion at all.
pub(super) fn cleaned_window(
    window: &[BaseLevel],
    context_level: Option<f64>,
) -> (Cow<'_, [BaseLevel]>, Vec<DiscardedReading>) {
    let levels: Vec<f64> = window.iter().map(|level| level.value).collect();
    let discarded = isolated_excursions(&levels, context_level);
    if discarded.is_empty() {
        return (Cow::Borrowed(window), Vec::new());
    }
    // Reported against the same neighbourhood the judgment consulted, so the explanation
    // and the decision cannot describe different surroundings.
    let reported = discarded
        .iter()
        .filter_map(|&index| {
            let level = window.get(index)?;
            let surroundings =
                Surroundings::around(index, &levels, noise_gates::EXCURSION_NEIGHBOURS)?;
            Some(DiscardedReading {
                topo_index: level.topo_index,
                value: level.value,
                neighbour_level: surroundings.level,
            })
        })
        .collect();
    let kept = window
        .iter()
        .enumerate()
        .filter(|(index, _)| !discarded.contains(index))
        .map(|(_, level)| level.clone())
        .collect();
    (Cow::Owned(kept), reported)
}

/// The indices of `levels` that are isolated measurement excursions, oldest first.
///
/// `levels` are the base window's per-commit levels, oldest first. A level qualifies
/// only when all three of these hold:
///
/// 1. **Its surroundings agree with each other.** The levels immediately before it and
///    the levels immediately after it must describe the same level, within
///    [`EXCURSION_NEIGHBOUR_AGREEMENT`](noise_gates::EXCURSION_NEIGHBOUR_AGREEMENT).
///    This is what separates a bad measurement from a real step: when the code genuinely
///    changes, the levels after the change sit at the *new* level and disagree with the
///    ones before it, so a step is never mistaken for an excursion however large it is.
/// 2. **It stands far clear of them.** It must differ from its surroundings by at least
///    [`EXCURSION_RELATIVE_MAGNITUDE`](noise_gates::EXCURSION_RELATIVE_MAGNITUDE),
///    which is set well above ordinary measurement wobble.
/// 3. **It is the only one, and the context run does not agree with it.** If more than
///    [`EXCURSION_MAX_REMOVALS`](noise_gates::EXCURSION_MAX_REMOVALS) levels qualify,
///    nothing is removed at all, and a level the context run itself sits at is never
///    removed. Either way the window has shown the level twice, and twice is a level the
///    series reaches rather than a moment its runner lost. Stripping a recurring level
///    away would leave a window describing a level the benchmark does not reliably hold,
///    against which its own ordinary values read as large, certain regressions.
///
/// A level without a full set of neighbours on *both* sides is never removed, since test 1
/// has nothing complete to consult. That is deliberate rather than incidental at the
/// newest end: a level shift that landed on the base branch near its final commit is
/// indistinguishable, from inside the window, from a bad reading there, and the two call
/// for opposite treatment.
///
/// The tests apply to every metric kind. A count or an allocation figure reproduces
/// exactly across runs of unchanged code, so an isolated excursion in one is as much a
/// measurement artifact as it is in a timing series, and is as unrepresentative of the
/// level a pull request would merge into.
pub(super) fn isolated_excursions(levels: &[f64], context_level: Option<f64>) -> Vec<usize> {
    let mut found = Vec::new();
    for index in 0..levels.len() {
        let Some(surroundings) =
            Surroundings::around(index, levels, noise_gates::EXCURSION_NEIGHBOURS)
        else {
            continue;
        };
        if !surroundings.agree(noise_gates::EXCURSION_NEIGHBOUR_AGREEMENT) {
            continue;
        }
        let Some(&level) = levels.get(index) else {
            continue;
        };
        if !surroundings.stands_clear(level, noise_gates::EXCURSION_RELATIVE_MAGNITUDE) {
            continue;
        }
        if context_level.is_some_and(|context| {
            surroundings.same_level(level, context, noise_gates::EXCURSION_NEIGHBOUR_AGREEMENT)
        }) {
            continue;
        }
        found.push(index);
        if found.len() > noise_gates::EXCURSION_MAX_REMOVALS {
            return Vec::new();
        }
    }
    found
}

/// The levels neighbouring one candidate, and what they say about its surroundings.
///
/// The candidate is judged against its own neighbourhood rather than against the whole
/// window because the window may legitimately hold a level shift: measured against a
/// window spanning two regimes, ordinary levels of either regime stand clear of the
/// middle and would be mistaken for excursions.
#[derive(Debug)]
struct Surroundings {
    /// The level the neighbours before the candidate sit at.
    before: f64,
    /// The level the neighbours after the candidate sit at.
    after: f64,
    /// The level of the neighbourhood as a whole, and the reference both tests measure
    /// against so that the two are expressed on one scale.
    level: f64,
}

impl Surroundings {
    /// The surroundings of `levels[index]`, taking `reach` neighbours on each side.
    ///
    /// `None` unless both sides are complete. A shorter side would let one adjacent level
    /// speak for that side, which is what `reach` exists to outvote, so the window's outer
    /// `reach` levels at each end are never candidates. It is also `None` when the
    /// neighbourhood sits at zero: both tests are proportional, and against a zero
    /// reference the magnitude test admits every nonzero level however small — the rule
    /// would lose the very narrowness that makes it safe. A window resting at zero is left
    /// exactly as measured; the absolute floors already keep branch mode from reporting a
    /// move there.
    fn around(index: usize, levels: &[f64], reach: usize) -> Option<Self> {
        let before = levels.get(index.checked_sub(reach)?..index)?;
        let after_start = index.checked_add(1)?;
        let after = levels.get(after_start..after_start.checked_add(reach)?)?;
        let neighbours: Vec<f64> = before.iter().chain(after).copied().collect();
        let level = stats::median(&neighbours)?;
        if level.abs() <= f64::EPSILON {
            return None;
        }
        Some(Self {
            before: stats::median(before)?,
            after: stats::median(after)?,
            level,
        })
    }

    /// Whether the two sides describe the same level, to within `tolerance`.
    fn agree(&self, tolerance: f64) -> bool {
        self.same_level(self.before, self.after, tolerance)
    }

    /// Whether `left` and `right` describe the same level, measured on this
    /// neighbourhood's scale so every judgment about the candidate shares one reference.
    fn same_level(&self, left: f64, right: f64, tolerance: f64) -> bool {
        relative_delta_of(right - left, self.level).abs() <= tolerance
    }

    /// Whether `level` differs from these surroundings by at least `magnitude`.
    fn stands_clear(&self, level: f64, magnitude: f64) -> bool {
        relative_delta_of(level - self.level, self.level).abs() >= magnitude
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use super::*;
    use crate::detect::noise_gates::{EXCURSION_NEIGHBOURS, EXCURSION_RELATIVE_MAGNITUDE};
    use crate::detect::recorded::{
        CONTENDED_RUNNER_BASE, CONTENDED_RUNNER_EXCURSION, CONTENDED_RUNNER_LEVEL_START,
        STATIONARY_BIMODAL_BASE, STATIONARY_BIMODAL_NOISE,
    };
    /// A level far enough above its surroundings to qualify, used where a test needs an
    /// excursion and does not care about its exact size.
    const EXCURSION: f64 = 200.0;

    /// The level the synthetic windows sit at when nothing is happening.
    const LEVEL: f64 = 100.0;

    fn flat(count: usize) -> Vec<f64> {
        vec![LEVEL; count]
    }

    #[test]
    fn a_flat_window_has_no_excursions() {
        assert!(isolated_excursions(&flat(16), None).is_empty());
    }

    #[test]
    fn a_lone_excursion_is_found() {
        let mut levels = flat(16);
        levels[8] = EXCURSION;
        assert_eq!(isolated_excursions(&levels, None), vec![8]);
    }

    #[test]
    fn a_lone_dip_is_found_as_readily_as_a_spike() {
        let mut levels = flat(16);
        levels[8] = LEVEL / 2.0;
        assert_eq!(isolated_excursions(&levels, None), vec![8]);
    }

    #[test]
    fn a_step_is_never_an_excursion_however_large() {
        // Every level past the step sits at the new level, so no level's surroundings
        // agree across it — which is the whole point: a real change must survive.
        let levels: Vec<f64> = flat(8).into_iter().chain(vec![EXCURSION; 8]).collect();
        assert!(isolated_excursions(&levels, None).is_empty());
    }

    #[test]
    fn a_step_reaching_the_window_end_is_never_an_excursion() {
        // The new level fills only the window's trailing edge, which is never a candidate,
        // so the last commit at the old level is the *only* position with a full set of
        // neighbours — and those neighbours disagree, its own side at the old level and the
        // far side at the new. The surroundings-agree test is therefore the sole thing
        // between this step and being read as a lone excursion: unlike the centred step
        // above, there is no second qualifying candidate to trip the removal limit and mask
        // a bypassed agreement check. Were that check dropped, this commit would be
        // discarded as a bad reading, tightening the window against the very move that just
        // landed.
        let levels: Vec<f64> = flat(11).into_iter().chain(vec![EXCURSION; 3]).collect();
        assert!(isolated_excursions(&levels, None).is_empty());
    }

    #[test]
    fn a_move_below_the_magnitude_is_ordinary_scatter() {
        let mut levels = flat(16);
        levels[8] = LEVEL * (1.0 + EXCURSION_RELATIVE_MAGNITUDE / 2.0);
        assert!(isolated_excursions(&levels, None).is_empty());
    }

    #[test]
    fn levels_without_full_surroundings_are_never_removed() {
        // The outer `EXCURSION_NEIGHBOURS` levels at each end have no complete side to be
        // judged against, so an excursion there is kept however far it stands out. Each
        // position is tried in an otherwise clean window, so the test fails if a
        // truncated neighbourhood is ever consulted instead of the position being skipped.
        let length = 16;
        let last = length - 1;
        let protected =
            (0..EXCURSION_NEIGHBOURS).chain((last - EXCURSION_NEIGHBOURS + 1)..=last);

        for index in protected {
            let mut levels = flat(length);
            levels[index] = EXCURSION;
            assert!(
                isolated_excursions(&levels, None).is_empty(),
                "index {index} has no complete surroundings and must be kept",
            );
        }
    }

    #[test]
    fn a_level_with_complete_surroundings_just_inside_the_edge_is_removable() {
        // The counterpart of the test above: protection stops exactly where complete
        // surroundings begin, so the guard cannot quietly widen into the window.
        let index = EXCURSION_NEIGHBOURS;
        let mut levels = flat(16);
        levels[index] = EXCURSION;
        assert_eq!(isolated_excursions(&levels, None), vec![index]);
    }

    #[test]
    fn a_second_candidate_stops_the_window_being_cleaned() {
        // Two separated levels agreeing on a value their surroundings do not is a
        // benchmark that visits more than one level, which the comparison must account
        // for rather than edit away. This is the sparse counterpart of the bimodal
        // recording below, where the second level is too rare to look like a mode.
        let mut levels = flat(16);
        levels[5] = EXCURSION;
        levels[11] = EXCURSION;
        assert!(isolated_excursions(&levels, None).is_empty());
    }

    #[test]
    fn a_level_the_context_run_also_sits_at_is_kept() {
        // The context run is the window's second sighting of that level, and two
        // sightings are a level the series reaches rather than a moment its runner lost.
        // Discarding it would leave the window describing a level the series does not
        // reliably hold, and the context run would read as a large, certain regression
        // against what remained.
        let mut levels = flat(16);
        levels[8] = EXCURSION;
        assert!(isolated_excursions(&levels, Some(EXCURSION)).is_empty());
    }

    #[test]
    fn a_context_run_elsewhere_does_not_protect_the_excursion() {
        // Only a context run at the candidate's own level is evidence of recurrence. One
        // that moved somewhere else says nothing about it, and the window is still
        // cleaned — which is the ordinary case the rule exists to serve.
        let mut levels = flat(16);
        levels[8] = EXCURSION;
        let elsewhere = LEVEL * (1.0 + EXCURSION_RELATIVE_MAGNITUDE / 2.0);
        assert_eq!(isolated_excursions(&levels, Some(elsewhere)), vec![8]);
    }

    #[test]
    fn a_window_past_the_limit_is_left_untouched() {
        let mut levels = flat(20);
        for index in [5, 11, 16] {
            levels[index] = EXCURSION;
        }
        assert!(isolated_excursions(&levels, None).is_empty());
    }

    #[test]
    fn a_bimodal_recording_is_left_untouched() {
        // The adversarial case. Half this recording's commits sit at each of two levels,
        // so many of them look isolated; removing them would leave a spuriously tight
        // window in which the series' own ordinary values read as certain regressions.
        let window = STATIONARY_BIMODAL_NOISE
            .get(..STATIONARY_BIMODAL_BASE)
            .expect("the base prefix is within the recording");
        assert!(isolated_excursions(window, None).is_empty());
    }

    #[test]
    fn a_recorded_contended_commit_is_found() {
        let window = CONTENDED_RUNNER_EXCURSION
            .get(CONTENDED_RUNNER_LEVEL_START..CONTENDED_RUNNER_BASE)
            .expect("the base slice is within the recording");
        let expected = window
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| left.total_cmp(right))
            .map(|(index, _)| index)
            .expect("the window is not empty");
        assert_eq!(isolated_excursions(window, None), vec![expected]);
    }

    #[test]
    fn a_window_resting_at_zero_is_left_untouched() {
        // Both tests are proportional and have nothing to say against a zero reference,
        // so such a window is left exactly as measured rather than cleaned by a rule
        // whose magnitude gate is inert there.
        let mut levels = vec![0.0; 16];
        levels[8] = 1.0;
        assert!(isolated_excursions(&levels, None).is_empty());
    }

    #[test]
    fn an_all_zero_window_has_no_excursions() {
        assert!(isolated_excursions(&[0.0; 16], None).is_empty());
    }

    #[test]
    fn a_window_too_short_to_have_surroundings_is_left_untouched() {
        // A candidate needs a full set of neighbours on each side, so a window shorter
        // than that plus the candidate itself has nowhere a level could be judged. Each
        // length is tried with a genuine excursion at every position, so the test would
        // fail if a short window fell back to judging against what neighbours it has.
        let shortest_with_interior = EXCURSION_NEIGHBOURS.saturating_mul(2).saturating_add(1);
        for length in 0..shortest_with_interior {
            for index in 0..length {
                let mut levels = flat(length);
                levels[index] = EXCURSION;
                assert!(
                    isolated_excursions(&levels, None).is_empty(),
                    "a window of {length} level(s) has no judgeable interior at {index}",
                );
            }
        }
    }

    fn window_of(levels: &[f64]) -> Vec<BaseLevel> {
        levels
            .iter()
            .enumerate()
            .map(|(index, &value)| BaseLevel {
                topo_index: index,
                value,
                interval: None,
            })
            .collect()
    }

    #[test]
    fn a_clean_window_is_passed_through_without_copying() {
        let window = window_of(&flat(16));
        let (cleaned, discarded) = cleaned_window(&window, None);
        assert!(matches!(cleaned, Cow::Borrowed(_)));
        assert_eq!(cleaned.as_ref(), window.as_slice());
        assert!(discarded.is_empty());
    }

    #[test]
    fn cleaning_drops_the_excursion_and_keeps_every_other_level_intact() {
        let mut levels = flat(16);
        levels[8] = EXCURSION;
        let window = window_of(&levels);
        let (cleaned, discarded) = cleaned_window(&window, None);
        let expected: Vec<BaseLevel> = window
            .iter()
            .filter(|level| level.topo_index != 8)
            .cloned()
            .collect();
        assert_eq!(cleaned.as_ref(), expected.as_slice());
        assert_eq!(
            discarded,
            vec![DiscardedReading {
                topo_index: 8,
                value: EXCURSION,
                neighbour_level: LEVEL,
            }],
            "what was reported must be exactly what was removed, described against the \
             neighbourhood the judgment consulted"
        );
    }
}
