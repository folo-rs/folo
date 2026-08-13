//! Figures and worked examples for the Detection chapter.
//!
//! Every figure here runs the **real detector** over the shared example series and draws
//! the answer it gave. Nothing restates the policy: the split a figure marks is the split
//! the code located, and the verdict beneath it is the verdict the code reached. A change
//! in detection behaviour therefore changes these assets, and the freshness check turns
//! that into a failing test rather than into prose that has quietly become wrong.

use cbh_detect::examples;
use cbh_detect::{AnalysisConfig, Finding, Series, evaluate_with_log};
use cbh_model::MetricKind;

use crate::assets::Asset;
use crate::styles::plot::{Mark, Observation, Plot};
use crate::theme;
use crate::verdict;

/// Every asset the Detection chapter embeds.
#[must_use]
pub fn assets() -> Vec<Asset> {
    let mut assets = Vec::new();
    assets.extend(clean_step());
    assets.extend(slow_ramp());
    assets.extend(blip());
    assets.extend(flat_noisy());
    assets.extend(minimums());
    assets
}

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
            verdict::reported(&finding, "the split is where the level changed, not where the \
                                          largest single jump happened"),
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
    use super::*;
    use cbh_detect::{Direction, FindingMethod};

    #[test]
    fn every_documented_example_produces_its_asset() {
        let paths: Vec<String> = assets().into_iter().map(|asset| asset.path).collect();

        for expected in [
            "detection-clean-step.svg",
            "detection-clean-step.md",
            "detection-slow-ramp.svg",
            "detection-slow-ramp.md",
            "detection-blip.svg",
            "detection-blip.md",
            "detection-flat-noisy.svg",
            "detection-flat-noisy.md",
            "detection-minimums.md",
        ] {
            assert!(paths.iter().any(|path| path == expected), "{expected} missing");
        }
    }

    /// The chapter presents these two as the worked examples of a step and a drift. If the
    /// detector ever arbitrates them the other way, the prose and the figures would
    /// contradict each other, so the claim is pinned here as well as in `cbh_detect`.
    #[test]
    fn the_worked_examples_report_the_methods_the_chapter_describes() {
        let (_, step) = judge_history("tokenize", &examples::clean_step(), MetricKind::WallTime);
        let (_, drift) =
            judge_history("index_build", &examples::slow_ramp(), MetricKind::WallTime);

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
        let (_, noisy) =
            judge_history("tokenize_ascii", &examples::flat_noisy(), MetricKind::WallTime);

        assert!(blip.is_none(), "a lone elevated point must not report");
        assert!(noisy.is_none(), "scatter around one level must not report");
    }

    #[test]
    fn a_reported_step_is_attributed_to_a_commit_inside_the_series() {
        let (_, finding) =
            judge_history("tokenize", &examples::clean_step(), MetricKind::WallTime);
        let finding = finding.unwrap();

        let index = attributed_index(&finding).unwrap();

        assert!(index > 0 && index < examples::clean_step().len());
    }

    #[test]
    fn a_reported_step_moves_in_the_direction_the_values_do() {
        let (_, finding) =
            judge_history("tokenize", &examples::clean_step(), MetricKind::WallTime);
        let finding = finding.unwrap();

        assert_eq!(finding.direction, Direction::Regression);
        assert!(finding.latest > finding.baseline);
    }
}
