//! Rendering a detector's verdict as the prose fragment a chapter embeds.
//!
//! The appendix states what the tool decided about each worked example, in numbers. Those
//! numbers come from here rather than from the author, so a change in detection behaviour
//! rewrites the sentence instead of leaving it quietly wrong.
//!
//! Both the reported and the quiet case carry a one-line reading of *why*, supplied by the
//! chapter. That is the one part a generator cannot derive: the figures show what happened,
//! and the surrounding prose is what makes it mean something.

use cbh_detect::{AnalysisMode, Direction, Finding, FindingMethod};
use cbh_render::format_value;

/// The fragment describing a finding the detector reported.
#[must_use]
pub fn reported(finding: &Finding, reading: &str, mode: AnalysisMode) -> String {
    match mode {
        AnalysisMode::History => history_reported(finding, reading),
        AnalysisMode::Branch => branch_reported(finding, reading),
    }
}

/// History mode: a change point or a drift over the analyzed window.
fn history_reported(finding: &Finding, reading: &str) -> String {
    let reading = capitalized(reading);
    let direction = direction_of(finding);
    let method = match finding.method {
        FindingMethod::ChangePoint => "change point",
        FindingMethod::Drift => "drift",
    };

    // A change point names the commit the new level starts at, which is a claim about
    // where something happened. A drift belongs to its whole window rather than one commit,
    // so it names the range it accumulated over. Rendering a drift as a single commit would
    // invite the reader to go looking at one commit's diff, which is exactly the wrong thing
    // to do with one.
    let attribution = match (finding.method, finding.commit.as_deref()) {
        (_, None) => "across the analyzed window".to_owned(),
        (FindingMethod::ChangePoint, Some(commit)) => format!("first seen at `{commit}`"),
        (FindingMethod::Drift, Some(commit)) => match finding.window_start_commit.as_deref() {
            Some(start) => format!("accumulated from `{start}` to `{commit}`"),
            None => format!("accumulated across the window ending at `{commit}`"),
        },
    };
    let baseline = format_value(finding.baseline);
    let latest = format_value(finding.latest);

    format!(
        "> **Reported.** A {direction} of {:+.2}% via {method}, {attribution}, at {:.0}% \
         confidence.\n>\n\
         > The level moved from {baseline} to {latest}.\n>\n\
         > {reading}.\n",
        finding.relative_delta * 100.0,
        finding.confidence * 100.0,
    )
}

/// Branch mode: the tip against the base prediction interval.
///
/// The detector still records a [`FindingMethod`] because both modes share one finding
/// type; the branch comparison is not a change point, so the fragment must not borrow
/// history-mode wording.
fn branch_reported(finding: &Finding, reading: &str) -> String {
    let reading = capitalized(reading);
    let direction = direction_of(finding);
    let baseline = format_value(finding.baseline);
    let latest = format_value(finding.latest);

    format!(
        "> **Reported.** A {direction} of {:+.2}% at the branch tip against the base \
         prediction interval, at {:.0}% confidence.\n>\n\
         > The tip is {latest} against a base level of {baseline}.\n>\n\
         > {reading}.\n",
        finding.relative_delta * 100.0,
        finding.confidence * 100.0,
    )
}

fn direction_of(finding: &Finding) -> &'static str {
    match finding.direction {
        Direction::Regression => "regression",
        Direction::Improvement => "improvement",
    }
}

/// Capitalizes the first character of a caption, so a lowercase source string reads as a
/// sentence in the rendered blockquote.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The fragment describing a series the detector left alone.
///
/// Takes the finding rather than a bare flag so a case that unexpectedly starts reporting
/// produces a fragment that says so, instead of silently continuing to claim silence.
#[must_use]
pub fn quiet(finding: Option<&Finding>, reading: &str) -> String {
    match finding {
        None => format!("> **Nothing reported.** {}.\n", capitalized(reading)),
        Some(found) => format!(
            "> **Reported**, unexpectedly: {:+.2}% at {:.0}% confidence. This example is \
             documented as staying quiet, so this fragment means the two have diverged.\n",
            found.relative_delta * 100.0,
            found.confidence * 100.0,
        ),
    }
}

#[cfg(test)]
mod tests {
    use cbh_detect::{evaluate_with_log, examples};
    use cbh_model::MetricKind;

    use super::*;

    fn a_finding() -> Finding {
        let values = examples::clean_step();
        let series = examples::series("bench", &values, MetricKind::WallTime, 0);
        let context = examples::history_context(&series);
        evaluate_with_log(&series, &context).0.unwrap()
    }

    fn a_branch_finding() -> Finding {
        let values = examples::clean_step();
        let series = examples::series("bench", &values, MetricKind::WallTime, 0);
        let merge_base = values
            .len()
            .checked_div(2)
            .and_then(|half| half.checked_sub(1))
            .expect("the example series holds more than one point");
        let context = examples::branch_context(&series, merge_base);
        evaluate_with_log(&series, &context).0.unwrap()
    }

    #[test]
    fn a_reported_fragment_carries_the_move_and_the_confidence() {
        let fragment = reported(
            &a_finding(),
            "because the level changed",
            AnalysisMode::History,
        );

        assert!(fragment.contains("Reported."));
        assert!(fragment.contains("confidence"));
        assert!(fragment.contains("Because the level changed."));
    }

    /// Branch mode compares the tip to the base interval. History-mode phrasing would
    /// describe a split that the branch detector never locates.
    #[test]
    fn a_branch_fragment_describes_a_tip_against_the_base_interval() {
        let fragment = reported(
            &a_branch_finding(),
            "the tip sits outside the predicted range",
            AnalysisMode::Branch,
        );

        assert!(fragment.contains("branch tip"));
        assert!(fragment.contains("base prediction interval"));
        assert!(!fragment.contains("via change point"));
        assert!(!fragment.contains("first seen"));
        assert!(!fragment.contains("the level moved"));
    }

    #[test]
    fn a_quiet_fragment_states_the_reason() {
        let fragment = quiet(None, "one point is not a level");

        assert!(fragment.contains("Nothing reported."));
        assert!(fragment.contains("One point is not a level."));
    }

    /// A quiet example that starts reporting must say so in the book rather than keep
    /// claiming silence — the failure would otherwise be invisible in the rendered page.
    #[test]
    fn a_quiet_fragment_that_finds_a_verdict_announces_the_divergence() {
        let fragment = quiet(Some(&a_finding()), "this should have been quiet");

        assert!(fragment.contains("unexpectedly"));
        assert!(fragment.contains("diverged"));
    }
}
