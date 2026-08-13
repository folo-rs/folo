//! Rendering a detector's verdict as the prose fragment a chapter embeds.
//!
//! The appendix states what the tool decided about each worked example, in numbers. Those
//! numbers come from here rather than from the author, so a change in detection behaviour
//! rewrites the sentence instead of leaving it quietly wrong.
//!
//! Both the reported and the quiet case carry a one-line reading of *why*, supplied by the
//! chapter. That is the one part a generator cannot derive: the figures show what happened,
//! and the surrounding prose is what makes it mean something.

use cbh_detect::{Direction, Finding, FindingMethod};

/// The fragment describing a finding the detector reported.
#[must_use]
pub fn reported(finding: &Finding, reading: &str) -> String {
    let direction = match finding.direction {
        Direction::Regression => "regression",
        Direction::Improvement => "improvement",
    };
    let method = match finding.method {
        FindingMethod::ChangePoint => "change point",
        FindingMethod::Drift => "drift",
    };

    // A change point names the commit the new level starts at, which is a claim about
    // where something happened. A drift names the newest commit in its window, which is
    // not — the movement is spread across the whole window. Rendering both as "attributed
    // to" would invite the reader to go looking at one commit's diff for a drift, which is
    // exactly the wrong thing to do with one.
    let attribution = match (finding.method, finding.commit.as_deref()) {
        (_, None) => "across the analyzed window".to_owned(),
        (FindingMethod::ChangePoint, Some(commit)) => format!("first seen at `{commit}`"),
        (FindingMethod::Drift, Some(commit)) => {
            format!("accumulated across the window ending at `{commit}`")
        }
    };

    format!(
        "> **Reported.** A {direction} of {:+.2}% via {method}, {attribution}, at {:.0}% \
         confidence.\n>\n\
         > The level moved from {:.2} to {:.2}.\n>\n\
         > {reading}.\n",
        finding.relative_delta * 100.0,
        finding.confidence * 100.0,
        finding.baseline,
        finding.latest,
    )
}

/// The fragment describing a series the detector left alone.
///
/// Takes the finding rather than a bare flag so a case that unexpectedly starts reporting
/// produces a fragment that says so, instead of silently continuing to claim silence.
#[must_use]
pub fn quiet(finding: Option<&Finding>, reading: &str) -> String {
    match finding {
        None => format!("> **Nothing reported.** {reading}.\n"),
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
    use super::*;
    use cbh_detect::examples;
    use cbh_detect::evaluate_with_log;
    use cbh_model::MetricKind;

    fn a_finding() -> Finding {
        let values = examples::clean_step();
        let series = examples::series("bench", &values, MetricKind::WallTime, 0);
        let context = examples::history_context(&series);
        evaluate_with_log(&series, &context).0.unwrap()
    }

    #[test]
    fn a_reported_fragment_carries_the_move_and_the_confidence() {
        let fragment = reported(&a_finding(), "because the level changed");

        assert!(fragment.contains("Reported."));
        assert!(fragment.contains("confidence"));
        assert!(fragment.contains("because the level changed."));
    }

    #[test]
    fn a_quiet_fragment_states_the_reason() {
        let fragment = quiet(None, "one point is not a level");

        assert!(fragment.contains("Nothing reported."));
        assert!(fragment.contains("one point is not a level."));
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

