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

use super::AnalysisConfig;
use super::findings::relative_delta_of;
use super::series::BaseLevel;

/// `window` with its isolated measurement excursions removed, oldest first.
///
/// Borrows `window` unchanged in the ordinary case, which is every window that holds no
/// excursion at all.
pub(super) fn cleaned_window<'a>(
    window: &'a [BaseLevel],
    config: &AnalysisConfig,
) -> Cow<'a, [BaseLevel]> {
    let levels: Vec<f64> = window.iter().map(|level| level.value).collect();
    let discarded = isolated_excursions(&levels, config);
    if discarded.is_empty() {
        return Cow::Borrowed(window);
    }
    Cow::Owned(
        window
            .iter()
            .enumerate()
            .filter(|(index, _)| !discarded.contains(index))
            .map(|(_, level)| level.clone())
            .collect(),
    )
}

/// The indices of `levels` that are isolated measurement excursions, oldest first.
///
/// `levels` are the base window's per-commit levels, oldest first. A level qualifies
/// only when all three of these hold:
///
/// 1. **Its surroundings agree with each other.** The levels immediately before it and
///    the levels immediately after it must describe the same level, within
///    [`excursion_neighbour_agreement`](AnalysisConfig::excursion_neighbour_agreement).
///    This is what separates a bad measurement from a real step: when the code genuinely
///    changes, the levels after the change sit at the *new* level and disagree with the
///    ones before it, so a step is never mistaken for an excursion however large it is.
/// 2. **It stands far clear of them.** It must differ from its surroundings by at least
///    [`excursion_relative_magnitude`](AnalysisConfig::excursion_relative_magnitude),
///    which is set well above ordinary measurement wobble.
/// 3. **There are few of them.** If more than
///    [`excursion_max_removals`](AnalysisConfig::excursion_max_removals) levels qualify,
///    nothing is removed at all. A window full of candidates is not a clean window with
///    a bad reading in it; it is a benchmark that genuinely oscillates, and stripping one
///    of its two levels away would leave a spuriously tight window in which the
///    benchmark's own ordinary values read as large, certain regressions.
///
/// The window's first and last levels are never removed, since neither has surroundings
/// on both sides for test 1 to consult. That is deliberate rather than incidental at the
/// newest end: a level shift that landed on the base branch at its final commit is
/// indistinguishable, from inside the window, from a bad reading there, and the two call
/// for opposite treatment.
///
/// The tests apply to every metric kind. A count or an allocation figure reproduces
/// exactly across runs of unchanged code, so an isolated excursion in one is as much a
/// measurement artifact as it is in a timing series, and is as unrepresentative of the
/// level a pull request would merge into.
pub(super) fn isolated_excursions(levels: &[f64], config: &AnalysisConfig) -> Vec<usize> {
    let mut found = Vec::new();
    for index in 0..levels.len() {
        let Some(surroundings) = Surroundings::around(index, levels, config.excursion_neighbours)
        else {
            continue;
        };
        if !surroundings.agree(config.excursion_neighbour_agreement) {
            continue;
        }
        let Some(&level) = levels.get(index) else {
            continue;
        };
        if !surroundings.stands_clear(level, config.excursion_relative_magnitude) {
            continue;
        }
        found.push(index);
        if found.len() > config.excursion_max_removals {
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
    /// The surroundings of `levels[index]`, taking up to `reach` neighbours on each side.
    ///
    /// `None` when either side is empty, which is what excludes the window's endpoints,
    /// and `None` when the neighbourhood sits at zero. Both tests are proportional, and
    /// against a zero reference the magnitude test admits every nonzero level however
    /// small — so the rule would lose the very narrowness that makes it safe. A window
    /// resting at zero is left exactly as measured; the absolute floors already keep
    /// branch mode from reporting a move there.
    fn around(index: usize, levels: &[f64], reach: usize) -> Option<Self> {
        let before = levels.get(index.saturating_sub(reach)..index)?;
        let after_start = index.checked_add(1)?;
        let after_end = after_start.saturating_add(reach).min(levels.len());
        let after = levels.get(after_start..after_end)?;
        if before.is_empty() || after.is_empty() {
            return None;
        }
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
        relative_delta_of(self.after - self.before, self.level).abs() <= tolerance
    }

    /// Whether `level` differs from these surroundings by at least `magnitude`.
    fn stands_clear(&self, level: f64, magnitude: f64) -> bool {
        relative_delta_of(level - self.level, self.level).abs() >= magnitude
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::recorded::{
        CONTENDED_RUNNER_BASE, CONTENDED_RUNNER_EXCURSION, CONTENDED_RUNNER_LEVEL_START,
        STATIONARY_BIMODAL_BASE, STATIONARY_BIMODAL_NOISE,
    };

    /// A level far enough above its surroundings to qualify, used where a test needs an
    /// excursion and does not care about its exact size.
    const EXCURSION: f64 = 200.0;

    /// The level the synthetic windows sit at when nothing is happening.
    const LEVEL: f64 = 100.0;

    fn config() -> AnalysisConfig {
        AnalysisConfig::default()
    }

    fn flat(count: usize) -> Vec<f64> {
        vec![LEVEL; count]
    }

    #[test]
    fn a_flat_window_has_no_excursions() {
        assert!(isolated_excursions(&flat(16), &config()).is_empty());
    }

    #[test]
    fn a_lone_excursion_is_found() {
        let mut levels = flat(16);
        levels[8] = EXCURSION;
        assert_eq!(isolated_excursions(&levels, &config()), vec![8]);
    }

    #[test]
    fn a_lone_dip_is_found_as_readily_as_a_spike() {
        let mut levels = flat(16);
        levels[8] = LEVEL / 2.0;
        assert_eq!(isolated_excursions(&levels, &config()), vec![8]);
    }

    #[test]
    fn a_step_is_never_an_excursion_however_large() {
        // Every level past the step sits at the new level, so no level's surroundings
        // agree across it — which is the whole point: a real change must survive.
        let levels: Vec<f64> = flat(8).into_iter().chain(vec![EXCURSION; 8]).collect();
        assert!(isolated_excursions(&levels, &config()).is_empty());
    }

    #[test]
    fn a_move_below_the_magnitude_is_ordinary_scatter() {
        let config = config();
        let mut levels = flat(16);
        levels[8] = LEVEL * (1.0 + config.excursion_relative_magnitude / 2.0);
        assert!(isolated_excursions(&levels, &config).is_empty());
    }

    #[test]
    fn the_window_endpoints_are_never_removed() {
        let mut levels = flat(16);
        levels[0] = EXCURSION;
        let last = levels.len() - 1;
        levels[last] = EXCURSION;
        assert!(isolated_excursions(&levels, &config()).is_empty());
    }

    #[test]
    fn excursions_up_to_the_limit_are_all_found() {
        let config = config();
        let mut levels = flat(16);
        // Spaced so neither lands in the other's surroundings, which is the case the
        // limit is meant to admit: independent bad moments, not an oscillation.
        levels[4] = EXCURSION;
        levels[12] = EXCURSION;
        assert_eq!(config.excursion_max_removals, 2);
        assert_eq!(isolated_excursions(&levels, &config), vec![4, 12]);
    }

    #[test]
    fn a_window_past_the_limit_is_left_untouched() {
        let mut levels = flat(20);
        for index in [4, 10, 16] {
            levels[index] = EXCURSION;
        }
        assert!(isolated_excursions(&levels, &config()).is_empty());
    }

    #[test]
    fn a_bimodal_recording_is_left_untouched() {
        // The adversarial case. Half this recording's commits sit at each of two levels,
        // so many of them look isolated; removing them would leave a spuriously tight
        // window in which the series' own ordinary values read as certain regressions.
        let window = STATIONARY_BIMODAL_NOISE
            .get(..STATIONARY_BIMODAL_BASE)
            .expect("the base prefix is within the recording");
        assert!(isolated_excursions(window, &config()).is_empty());
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
        assert_eq!(isolated_excursions(window, &config()), vec![expected]);
    }

    #[test]
    fn a_window_resting_at_zero_is_left_untouched() {
        // Both tests are proportional and have nothing to say against a zero reference,
        // so such a window is left exactly as measured rather than cleaned by a rule
        // whose magnitude gate is inert there.
        let mut levels = vec![0.0; 16];
        levels[8] = 1.0;
        assert!(isolated_excursions(&levels, &config()).is_empty());
    }

    #[test]
    fn an_all_zero_window_has_no_excursions() {
        assert!(isolated_excursions(&vec![0.0; 16], &config()).is_empty());
    }

    #[test]
    fn a_window_too_short_to_have_surroundings_is_left_untouched() {
        for length in 0..=2 {
            let levels = vec![EXCURSION; length];
            assert!(
                isolated_excursions(&levels, &config()).is_empty(),
                "a window of {length} level(s) has no interior",
            );
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
        let cleaned = cleaned_window(&window, &config());
        assert!(matches!(cleaned, Cow::Borrowed(_)));
        assert_eq!(cleaned.as_ref(), window.as_slice());
    }

    #[test]
    fn cleaning_drops_the_excursion_and_keeps_every_other_level_intact() {
        let mut levels = flat(16);
        levels[8] = EXCURSION;
        let window = window_of(&levels);
        let cleaned = cleaned_window(&window, &config());
        let expected: Vec<BaseLevel> = window
            .iter()
            .filter(|level| level.topo_index != 8)
            .cloned()
            .collect();
        assert_eq!(cleaned.as_ref(), expected.as_slice());
    }
}
