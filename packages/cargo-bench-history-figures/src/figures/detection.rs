//! Figures and worked examples for the Detection chapter.
//!
//! Every figure here runs the **real detector** over the shared example series and draws
//! the answer it gave. Nothing restates the policy: the split a figure marks is the split
//! the code located, and the verdict beneath it is the verdict the code reached. A change
//! in detection behaviour therefore changes these assets, and the freshness check turns
//! that into a failing test rather than into prose that has quietly become wrong.

use cbh_detect::{AnalysisConfig, AnalysisMode, Finding, Series, evaluate_with_log, examples};
use cbh_model::MetricKind;
use cbh_stats::{mean, sample_std_dev};
use plotters::style::RGBColor;

use crate::assets::Asset;
use crate::styles::plot::{Mark, Observation, Plot};
use crate::{theme, verdict};

/// Every asset the Detection chapter embeds.
#[must_use]
pub fn assets() -> Vec<Asset> {
    let mut assets = Vec::new();
    assets.extend(clean_step());
    assets.extend(multi_step());
    assets.extend(slow_ramp());
    assets.extend(blip());
    assets.extend(returned_excursion());
    assets.extend(flat_noisy());
    assets.extend(branch());
    assets.extend(branch_base_moved());
    assets.extend(confidence_examples());
    assets.extend(minimums());
    assets
}

/// The branch-mode pair: a tip the base window does not explain, and one it does.
///
/// Drawn as two figures over the *same* base window so the only thing that differs between
/// them is the tip. That is the comparison the chapter is making, and separate base windows
/// would leave a reader unable to tell which difference produced the different verdict.
fn branch() -> Vec<Asset> {
    let mut assets = Vec::new();
    for (name, tip, reading) in BRANCH_CASES {
        let (values, finding) = branch_finding(name, tip);

        let tip_index = values.len().saturating_sub(1);
        let prediction = branch_prediction_band(
            values
                .get(..tip_index)
                .expect("the branch figure always has base-side values before its tip"),
        );
        let plot = Plot::new("a branch tip against its base window", values.len())
            .value_label("ns")
            .scattered()
            .observations(values.iter().enumerate().map(|(index, &value)| {
                let observation = Observation::new(index, value);
                if index == tip_index {
                    observation.marked(if finding.is_some() {
                        Mark::Regression
                    } else {
                        Mark::Focus
                    })
                } else {
                    observation
                }
            }))
            .band(
                0,
                tip_index.saturating_sub(1),
                "base window",
                theme::HIGHLIGHT,
            )
            .value_band(
                0,
                tip_index,
                prediction,
                "predicted range",
                theme::ALTERNATE,
            )
            .split(tip_index, "the branch tip");

        assets.push(Asset::new(format!("{name}.svg"), plot.render()));
        assets.push(Asset::new(
            format!("{name}.md"),
            finding.as_ref().map_or_else(
                || verdict::quiet(None, reading),
                |found| verdict::reported(found, reading, AnalysisMode::Branch),
            ),
        ));
    }
    assets
}

/// The level the branch figures' base window sits at.
const BRANCH_BASE_LEVEL: f64 = 100.0;

/// The two branch figures: a tip the base window does not explain, and one it does. The
/// tip levels are what make one report and the other stay quiet, which the accompanying
/// test pins against the real detector so a policy change cannot silently reverse a
/// lesson while keeping the asset names.
const BRANCH_CASES: [(&str, f64, &str); 2] = [
    (
        "detection-branch-reported",
        BRANCH_BASE_LEVEL * 1.30,
        "the tip sits outside the range a further measurement was expected in",
    ),
    (
        "detection-branch-quiet",
        BRANCH_BASE_LEVEL,
        "the tip is inside the range the base window predicts, so there is nothing to report",
    ),
];

/// How wide the branch figures draw their illustrative prediction range.
///
/// The detector records the p-value, not a plotted cutoff. This multiple makes the range
/// visibly cover ordinary base scatter while keeping the reporting tip outside it.
const BRANCH_PREDICTION_SIGMAS: f64 = 3.0;

/// The value interval the branch figures shade as the base window's prediction.
fn branch_prediction_band(values: &[f64]) -> (f64, f64) {
    let centre = mean(values).expect("the branch figure's base window is not empty");
    let scatter = sample_std_dev(values)
        .expect("the branch figure's base window has enough points to estimate scatter");
    let sample_count = crate::coord::of(values.len());
    let half_width = scatter * (1.0 + 1.0 / sample_count).sqrt() * BRANCH_PREDICTION_SIGMAS;
    (centre - half_width, centre + half_width)
}

/// Runs the real branch detector over the shared base window with `tip` appended, and
/// returns the laid-out values and the verdict it reached.
fn branch_finding(name: &str, tip: f64) -> (Vec<f64>, Option<Finding>) {
    // One more base commit than the comparison needs, so the window is genuinely a window
    // rather than the whole series, and the merge base sits where a real branch would fork.
    let mut values: Vec<f64> = examples::scattered(
        &[BRANCH_BASE_LEVEL; BASE_COMMITS],
        examples::TIMING_NOISE_CV,
        examples::seed_of("branch-base"),
    );
    values.push(tip);
    let series = examples::series(name, &values, MetricKind::WallTime, 0);
    let context = examples::branch_context(&series, BASE_COMMITS.saturating_sub(1));
    let (finding, _) = evaluate_with_log(&series, &context);
    (values, finding)
}

/// The older base level in the branch-base-moved figure.
///
/// Lower than the current base by enough to qualify as a trusted base-side boundary.
const MOVED_BASE_OLD_LEVEL: f64 = 100.0;

/// The current base level in the branch-base-moved figure.
const MOVED_BASE_CURRENT_LEVEL: f64 = 130.0;

/// The branch tip in the branch-base-moved figure.
///
/// Chosen outside the current regime's prediction band so the generated verdict proves
/// that the comparison was formed from the newer base level.
const MOVED_BASE_TIP_LEVEL: f64 = 145.0;

/// How many commits each base regime occupies in the branch-base-moved figure.
///
/// Equal halves fill the default comparison window while satisfying the stricter evidence
/// floor for accepting a base-side regime boundary.
const MOVED_BASE_REGIME: usize = 8;

/// Builds the branch-base-moved example and returns its values and detector verdict.
fn branch_base_moved_finding() -> (Vec<f64>, Option<Finding>) {
    let base_levels: Vec<f64> = std::iter::repeat_n(MOVED_BASE_OLD_LEVEL, MOVED_BASE_REGIME)
        .chain(std::iter::repeat_n(
            MOVED_BASE_CURRENT_LEVEL,
            MOVED_BASE_REGIME,
        ))
        .collect();
    let mut values = examples::scattered(
        &base_levels,
        examples::TIMING_NOISE_CV,
        examples::seed_of("branch_base_moved"),
    );
    values.push(MOVED_BASE_TIP_LEVEL);
    let series = examples::series("branch_base_moved", &values, MetricKind::WallTime, 0);
    let context = examples::branch_context(&series, base_levels.len().saturating_sub(1));
    let (finding, _) = evaluate_with_log(&series, &context);
    (values, finding)
}

/// A branch base that changed recently, so the older base level is discarded.
fn branch_base_moved() -> Vec<Asset> {
    let (values, finding) = branch_base_moved_finding();
    let finding = finding.expect(
        "the moved-base example places the tip outside the current regime's prediction \
         band, so it must report if the detector uses the newer regime",
    );
    let tip_index = values.len().saturating_sub(1);
    let current_start = MOVED_BASE_REGIME;
    let current_values = values
        .get(current_start..tip_index)
        .expect("the moved-base example has a current base regime before the tip");
    let prediction = branch_prediction_band(current_values);

    let plot = Plot::new("a branch tip after the base level moved", values.len())
        .value_label("ns")
        .scattered()
        .observations(values.iter().enumerate().map(|(index, &value)| {
            let observation = Observation::new(index, value);
            if index < current_start {
                observation.marked(Mark::Removed)
            } else if index == tip_index {
                observation.marked(Mark::Regression)
            } else {
                observation
            }
        }))
        .band(
            0,
            current_start.saturating_sub(1),
            "discarded older base level",
            theme::MUTED,
        )
        .value_band(
            current_start,
            tip_index,
            prediction,
            "predicted range from current base",
            theme::ALTERNATE,
        )
        .split(current_start, "current-base boundary");

    vec![
        Asset::new("detection-branch-base-moved.svg", plot.render()),
        Asset::new(
            "detection-branch-base-moved.md",
            verdict::reported(
                &finding,
                "the prediction is centered on the newer base regime, not on the whole \
                 mixed window",
                AnalysisMode::Branch,
            ),
        ),
    ]
}

/// The baseline for the lower-confidence step.
///
/// High enough that the absolute floor is irrelevant, leaving the example about the
/// amount of evidence for an accepted relative move rather than about metric units.
const LOWER_CONFIDENCE_BASELINE: f64 = 1_000.0;

/// Relative size of the minimum-length confidence example's step.
///
/// Chosen large enough that practical floors and residual scatter are not what limits
/// confidence; only the number of rank comparisons is.
const LOWER_CONFIDENCE_STEP_RELATIVE: f64 = 0.30;

/// Within-regime spacing for the minimum-length confidence example.
///
/// The regimes stay fully separated, but individual values do not tie, so the rank
/// test demonstrates the confidence cap from having only the minimum number of points.
const LOWER_CONFIDENCE_INTRA_REGIME_SPACING: f64 = 1.0;

/// A fully separated step with the minimum allowed regime length on each side.
fn lower_confidence_step_values() -> Vec<f64> {
    let config = AnalysisConfig::default();
    let elevated = LOWER_CONFIDENCE_BASELINE * (1.0 + LOWER_CONFIDENCE_STEP_RELATIVE);
    let midpoint = (crate::coord::of(config.min_regime) - 1.0) / 2.0;
    (0..config.min_regime)
        .map(|index| {
            LOWER_CONFIDENCE_BASELINE
                + (crate::coord::of(index) - midpoint) * LOWER_CONFIDENCE_INTRA_REGIME_SPACING
        })
        .chain((0..config.min_regime).map(|index| {
            elevated + (crate::coord::of(index) - midpoint) * LOWER_CONFIDENCE_INTRA_REGIME_SPACING
        }))
        .collect()
}

/// History-mode examples whose accepted findings carry different confidence values.
fn confidence_examples() -> Vec<Asset> {
    let high_values = examples::clean_step();
    let (_, high) = judge_history("confidence_high", &high_values, MetricKind::WallTime);
    let high = high.expect("the clean-step confidence example must report");

    let lower_values = lower_confidence_step_values();
    let (_, lower) = judge_history("confidence_lower", &lower_values, MetricKind::WallTime);
    let lower = lower.expect("the lower-confidence example must still report");

    vec![
        Asset::new(
            "detection-confidence-high.svg",
            confidence_plot(
                "a cleanly separated step",
                &high_values,
                &high,
                theme::REGRESSION,
            )
            .render(),
        ),
        Asset::new(
            "detection-confidence-high.md",
            verdict::reported(
                &high,
                "a clean split with extra evidence rounds to a very high confidence",
                AnalysisMode::History,
            ),
        ),
        Asset::new(
            "detection-confidence-lower.svg",
            confidence_plot(
                "a minimum-length clean step",
                &lower_values,
                &lower,
                theme::ALTERNATE,
            )
            .render(),
        ),
        Asset::new(
            "detection-confidence-lower.md",
            verdict::reported(
                &lower,
                "the split is clean, but the minimum-length regimes cap the confidence",
                AnalysisMode::History,
            ),
        ),
    ]
}

/// A confidence example with the detector's chosen split and fitted levels.
fn confidence_plot(caption: &str, values: &[f64], finding: &Finding, color: RGBColor) -> Plot {
    let split = attributed_index(finding);
    let mut plot = Plot::new(caption, values.len())
        .value_label("ns")
        .base_color(color)
        .values(values)
        .rule(finding.baseline, "baseline", theme::HIGHLIGHT)
        .rule(finding.latest, "latest", theme::REGRESSION);
    if let Some(split) = split.filter(|index| *index > 0 && *index < values.len()) {
        plot = plot.split(split, "change point");
    }
    plot
}

/// How many base-side commits the branch figures lay out.
///
/// Chosen above the comparison window's own minimum so the figures show a window being
/// selected from a longer history rather than one that happens to be the whole series.
const BASE_COMMITS: usize = 18;

/// Runs the real history-mode detector over `values` and returns the series, the verdict,
/// and the plotted observations.
fn judge_history(name: &str, values: &[f64], kind: MetricKind) -> (Series, Option<Finding>) {
    let series = examples::series(name, values, kind, 0);
    let context = examples::history_context(&series);
    let (finding, _) = evaluate_with_log(&series, &context);
    (series, finding)
}

/// The index of the commit a finding is attributed to, where the example's commit naming
/// makes it recoverable.
///
/// The shared builder names commits `commit<topological index>`, so the attributed commit
/// carries its own position. Reading it back is what lets a figure mark the split the
/// detector actually chose rather than the split the author expected.
fn attributed_index(finding: &Finding) -> Option<usize> {
    finding
        .commit
        .as_deref()?
        .strip_prefix("commit")?
        .parse()
        .ok()
}

/// A step, with the split the detector located and each regime's own level.
fn clean_step() -> Vec<Asset> {
    let values = examples::clean_step();
    let (_, finding) = judge_history("tokenize", &values, MetricKind::WallTime);
    let finding = finding.expect(
        "the clean_step example is asserted to report in cbh_detect's own tests, so a \
         missing verdict here means the two are reading different data",
    );
    // A finding the detector reported always names its commit, and the shared builder names
    // commits by topological index, so the split is recoverable. Falling back to no split at
    // all is the honest response to an unrecoverable one: better an unmarked figure than one
    // marking a boundary the detector did not choose.
    let split = attributed_index(&finding);

    let mut plot = Plot::new("a level that stepped", values.len())
        .value_label("ns")
        .values(&values)
        .rule(finding.baseline, "baseline", theme::HIGHLIGHT)
        .rule(finding.latest, "new level", theme::REGRESSION);
    if let Some(split) = split.filter(|index| *index > 0 && *index < values.len()) {
        // The shading alone distinguishes the two regimes; the split marker and the two
        // level rules already say what they are, so labelling the bands as well would
        // crowd the top of the plot with three overlapping captions.
        plot = plot
            .split(split, "change point")
            .band(0, split.saturating_sub(1), "", theme::HIGHLIGHT)
            .band(split, values.len().saturating_sub(1), "", theme::REGRESSION);
    }

    vec![
        Asset::new("detection-clean-step.svg", plot.render()),
        Asset::new(
            "detection-clean-step.md",
            verdict::reported(
                &finding,
                "the split is where the level changed, not where the \
                                          largest single jump happened",
                AnalysisMode::History,
            ),
        ),
    ]
}

/// How many persistence floors each regime in the multiple-step example occupies.
///
/// The point of the figure is split selection rather than minimum evidence, so every
/// visible level gets comfortable support on both sides of its boundary.
const MULTI_STEP_REGIME_MULTIPLE: usize = 2;

/// Levels used by the multiple-step figure.
///
/// The first jump is deliberately larger than the later one, so the single boundary the
/// detector reports is stable and visually explainable without marking the later step.
const MULTI_STEP_LEVELS: [f64; 3] = [100.0, 140.0, 160.0];

/// A series with several steps, where the detector still reports one boundary.
fn multi_step() -> Vec<Asset> {
    let regime = AnalysisConfig::default()
        .min_regime
        .saturating_mul(MULTI_STEP_REGIME_MULTIPLE);
    let levels: Vec<f64> = MULTI_STEP_LEVELS
        .into_iter()
        .flat_map(|level| std::iter::repeat_n(level, regime))
        .collect();
    let values = examples::scattered(
        &levels,
        examples::TIMING_NOISE_CV,
        examples::seed_of("multi_step"),
    );
    let (_, finding) = judge_history("json_decode", &values, MetricKind::WallTime);
    let finding = finding.expect(
        "the multiple-step example has sustained, separated regimes, so a missing verdict \
         means the figure no longer demonstrates split selection",
    );
    let split = attributed_index(&finding);

    let mut plot = Plot::new("several steps, one reported boundary", values.len())
        .value_label("ns")
        .values(&values)
        .rule(finding.baseline, "fitted before", theme::HIGHLIGHT)
        .rule(finding.latest, "fitted after", theme::REGRESSION);
    if let Some(split) = split.filter(|index| *index > 0 && *index < values.len()) {
        plot = plot.split(split, "change point").band(
            split,
            values.len().saturating_sub(1),
            "single after-side model",
            theme::REGRESSION,
        );
    }

    vec![
        Asset::new("detection-multi-step.svg", plot.render()),
        Asset::new(
            "detection-multi-step.md",
            verdict::reported(
                &finding,
                "the split search chooses one boundary even though another step remains \
                 visible",
                AnalysisMode::History,
            ),
        ),
    ]
}

/// A drift, with the fitted movement the trend detector reports.
fn slow_ramp() -> Vec<Asset> {
    let values = examples::slow_ramp();
    let (_, finding) = judge_history("index_build", &values, MetricKind::WallTime);
    let finding = finding.expect(
        "the slow_ramp example is asserted to report a drift in cbh_detect's own tests, so \
         a missing verdict here means the two are reading different data",
    );

    let plot = Plot::new("a level that drifted", values.len())
        .value_label("ns")
        .values(&values)
        .rule(finding.baseline, "fitted start", theme::MUTED)
        .rule(finding.latest, "fitted end", theme::REGRESSION);

    vec![
        Asset::new("detection-slow-ramp.svg", plot.render()),
        Asset::new(
            "detection-slow-ramp.md",
            verdict::reported(
                &finding,
                "no single commit is responsible, so the finding names the window",
                AnalysisMode::History,
            ),
        ),
    ]
}

/// A lone elevated point, which is not a level.
fn blip() -> Vec<Asset> {
    let values = examples::blip();
    let (_, finding) = judge_history("search_exact", &values, MetricKind::WallTime);

    let peak = values
        .iter()
        .enumerate()
        .max_by(|left, right| left.1.total_cmp(right.1))
        .map(|(index, _)| index)
        .unwrap_or_default();

    let plot = Plot::new("one elevated measurement", values.len())
        .value_label("ns")
        .observations(values.iter().enumerate().map(|(index, &value)| {
            let observation = Observation::new(index, value);
            if index == peak {
                observation.marked(Mark::Focus)
            } else {
                observation
            }
        }));

    vec![
        Asset::new("detection-blip.svg", plot.render()),
        Asset::new(
            "detection-blip.md",
            verdict::quiet(
                finding.as_ref(),
                "a level has to persist; one point is an event, not a level",
            ),
        ),
    ]
}

/// How many persistence floors each stretch of the returned-excursion example occupies.
///
/// The elevated part is long enough to read as a real level while it lasted, so the figure
/// is distinct from the single-point blip above it.
const RETURNED_EXCURSION_REGIME_MULTIPLE: usize = 2;

/// The level the returned-excursion example visits before returning to baseline.
///
/// Far enough above the baseline that the excursion is visually unmistakable without
/// needing a large plotted sample.
const RETURNED_EXCURSION_LEVEL: f64 = 140.0;

/// The settled level before and after the returned excursion.
///
/// Kept at the same round baseline as the other detection examples so percentages and
/// chart positions read consistently across the chapter.
const RETURNED_EXCURSION_BASELINE: f64 = 100.0;

/// The returned-excursion values, shared by the figure and its lockstep test.
fn returned_excursion_values() -> Vec<f64> {
    let regime = AnalysisConfig::default()
        .min_regime
        .saturating_mul(RETURNED_EXCURSION_REGIME_MULTIPLE);
    let levels: Vec<f64> = [
        RETURNED_EXCURSION_BASELINE,
        RETURNED_EXCURSION_LEVEL,
        RETURNED_EXCURSION_BASELINE,
    ]
    .into_iter()
    .flat_map(|level| std::iter::repeat_n(level, regime))
    .collect();
    let values = examples::scattered(
        &levels,
        examples::TIMING_NOISE_CV,
        examples::seed_of("returned_excursion"),
    );
    values
}

/// A sustained excursion that has already returned to its original level.
fn returned_excursion() -> Vec<Asset> {
    let values = returned_excursion_values();
    let (_, finding) = judge_history("regex_cache", &values, MetricKind::WallTime);
    let regime = AnalysisConfig::default()
        .min_regime
        .saturating_mul(RETURNED_EXCURSION_REGIME_MULTIPLE);
    let elevated_start = regime;
    let elevated_end = regime.saturating_mul(2).saturating_sub(1);

    let plot = Plot::new("a sustained excursion that returned", values.len())
        .value_label("ns")
        .observations(values.iter().enumerate().map(|(index, &value)| {
            let observation = Observation::new(index, value);
            if (elevated_start..=elevated_end).contains(&index) {
                observation.marked(Mark::Focus)
            } else {
                observation
            }
        }))
        .band(
            elevated_start,
            elevated_end,
            "temporary level",
            theme::HIGHLIGHT,
        )
        .rule(
            RETURNED_EXCURSION_BASELINE,
            "baseline and current level",
            theme::MUTED,
        );

    vec![
        Asset::new("detection-blip-returned.svg", plot.render()),
        Asset::new(
            "detection-blip-returned.md",
            verdict::quiet(
                finding.as_ref(),
                "the current level has already returned to the baseline",
            ),
        ),
    ]
}

/// A series that is simply noisy, and stays quiet.
fn flat_noisy() -> Vec<Asset> {
    let values = examples::flat_noisy();
    let (_, finding) = judge_history("tokenize_ascii", &values, MetricKind::WallTime);

    let plot = Plot::new("scatter around one unchanged level", values.len())
        .value_label("ns")
        .values(&values);

    vec![
        Asset::new("detection-flat-noisy.svg", plot.render()),
        Asset::new(
            "detection-flat-noisy.md",
            verdict::quiet(
                finding.as_ref(),
                "the split search still nominates its best candidate here; everything \
                 downstream exists to reject it",
            ),
        ),
    ]
}

/// The minimum evidence each detector demands, read from the shipped defaults.
fn minimums() -> Vec<Asset> {
    let config = AnalysisConfig::default();
    let markdown = format!(
        "| Detector | Needs |\n|---|---|\n\
         | History change point | {} in the analyzed window, and {} on each side of the split |\n\
         | History drift | {} in the analyzed window |\n\
         | Branch comparison | {} on the base side, collapsed to one level each |\n",
        points(config.min_series_points),
        points(config.min_regime),
        points(config.drift_min_points),
        commits(config.min_series_points),
    );

    vec![Asset::new("detection-minimums.md", markdown)]
}

/// `count` rendered as a plural-correct number of points.
fn points(count: usize) -> String {
    if count == 1 {
        "1 point".to_owned()
    } else {
        format!("{count} points")
    }
}

/// `count` rendered as a plural-correct number of commits.
fn commits(count: usize) -> String {
    if count == 1 {
        "1 commit".to_owned()
    } else {
        format!("{count} commits")
    }
}

#[cfg(test)]
mod tests {
    use cbh_detect::{Direction, FindingMethod};

    use super::*;

    #[test]
    fn the_branch_examples_report_the_outcomes_the_chapter_teaches() {
        // The chapter presents `detection-branch-reported` as a tip that reports and
        // `detection-branch-quiet` as a tip the base window explains. Pin both against the
        // real detector so a policy change fails this test rather than silently reversing a
        // lesson while keeping the asset names. Runs the detector, not the renderer, so it
        // needs no SVG budget.
        let (_, reported) = branch_finding("detection-branch-reported", BRANCH_BASE_LEVEL * 1.30);
        let (_, quiet) = branch_finding("detection-branch-quiet", BRANCH_BASE_LEVEL);

        let reported = reported.expect("the reported example must yield a branch finding");
        assert_eq!(reported.direction, Direction::Regression);
        assert!(
            quiet.is_none(),
            "the quiet example must yield no finding, got {quiet:?}"
        );
    }

    #[test]
    fn the_moved_base_example_uses_the_current_base_regime() {
        let (_, finding) = branch_base_moved_finding();
        let finding = finding.expect("the moved-base example must yield a branch finding");

        assert!(
            finding.baseline > MOVED_BASE_OLD_LEVEL,
            "the older base regime must not anchor the comparison: {finding:?}"
        );
        assert!(
            (finding.baseline - MOVED_BASE_CURRENT_LEVEL).abs()
                < (finding.baseline - MOVED_BASE_OLD_LEVEL).abs(),
            "the comparison must be closer to the current regime than the discarded one"
        );
    }

    #[test]
    fn accepted_confidence_examples_are_not_all_the_same_value() {
        let (_, high) = judge_history(
            "confidence_high",
            &examples::clean_step(),
            MetricKind::WallTime,
        );
        let (_, lower) = judge_history(
            "confidence_lower",
            &lower_confidence_step_values(),
            MetricKind::WallTime,
        );
        let high = high.expect("the clean-step confidence example must report");
        let lower = lower.expect("the lower-confidence example must report");
        let displayed_high = format!("{:.0}", high.confidence * 100.0);
        let displayed_lower = format!("{:.0}", lower.confidence * 100.0);

        assert!(high.confidence > lower.confidence);
        assert!(
            lower.confidence < 1.0,
            "the lower-confidence example must not be exact internal certainty"
        );
        assert_eq!(
            displayed_high, "100",
            "the high-confidence example must display as rounded certainty"
        );
        assert_ne!(
            displayed_lower, "100",
            "the lower-confidence example must display below rounded certainty"
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn every_documented_example_produces_its_asset() {
        let paths: Vec<String> = assets().into_iter().map(|asset| asset.path).collect();

        for expected in [
            "detection-clean-step.svg",
            "detection-clean-step.md",
            "detection-multi-step.svg",
            "detection-multi-step.md",
            "detection-slow-ramp.svg",
            "detection-slow-ramp.md",
            "detection-blip.svg",
            "detection-blip.md",
            "detection-blip-returned.svg",
            "detection-blip-returned.md",
            "detection-flat-noisy.svg",
            "detection-flat-noisy.md",
            "detection-branch-reported.svg",
            "detection-branch-reported.md",
            "detection-branch-quiet.svg",
            "detection-branch-quiet.md",
            "detection-branch-base-moved.svg",
            "detection-branch-base-moved.md",
            "detection-confidence-high.svg",
            "detection-confidence-high.md",
            "detection-confidence-lower.svg",
            "detection-confidence-lower.md",
            "detection-minimums.md",
        ] {
            assert!(
                paths.iter().any(|path| path == expected),
                "{expected} missing"
            );
        }
    }

    /// The chapter presents these two as the worked examples of a step and a drift. If the
    /// detector ever arbitrates them the other way, the prose and the figures would
    /// contradict each other, so the claim is pinned here as well as in `cbh_detect`.
    #[test]
    fn the_worked_examples_report_the_methods_the_chapter_describes() {
        let (_, step) = judge_history("tokenize", &examples::clean_step(), MetricKind::WallTime);
        let (_, drift) = judge_history("index_build", &examples::slow_ramp(), MetricKind::WallTime);

        assert_eq!(
            step.map(|finding| finding.method),
            Some(FindingMethod::ChangePoint)
        );
        assert_eq!(
            drift.map(|finding| finding.method),
            Some(FindingMethod::Drift)
        );
    }

    /// The chapter's two negative examples carry more weight than its positive ones: they
    /// are the evidence that the tool does not cry wolf.
    #[test]
    fn the_quiet_examples_stay_quiet() {
        let (_, blip) = judge_history("search_exact", &examples::blip(), MetricKind::WallTime);
        let returned_values = returned_excursion_values();
        let (_, returned) = judge_history("regex_cache", &returned_values, MetricKind::WallTime);
        let (_, noisy) = judge_history(
            "tokenize_ascii",
            &examples::flat_noisy(),
            MetricKind::WallTime,
        );

        assert!(blip.is_none(), "a lone elevated point must not report");
        assert!(
            returned.is_none(),
            "a sustained excursion that returned to baseline must not report"
        );
        assert!(noisy.is_none(), "scatter around one level must not report");
    }

    #[test]
    fn a_reported_step_is_attributed_to_a_commit_inside_the_series() {
        let (_, finding) = judge_history("tokenize", &examples::clean_step(), MetricKind::WallTime);
        let finding = finding.unwrap();

        let index = attributed_index(&finding).unwrap();

        assert!(index > 0 && index < examples::clean_step().len());
    }

    #[test]
    fn a_reported_step_moves_in_the_direction_the_values_do() {
        let (_, finding) = judge_history("tokenize", &examples::clean_step(), MetricKind::WallTime);
        let finding = finding.unwrap();

        assert_eq!(finding.direction, Direction::Regression);
        assert!(finding.latest > finding.baseline);
    }
}
