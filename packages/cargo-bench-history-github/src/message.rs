use crate::cli::{Conclusion, PendingArgs};
use crate::marker;
use crate::model::{Instance, IssueKind};
use crate::result::{Coverage, Evidence, Outcome, PublicationState};

const REGRESSION_HEADING: &str = "# Benchmark history";
const PR_HEADING: &str = "## Benchmark history";
const WARNING_HEADING: &str = "> [!WARNING]";
// Reporting uses one message catalogue; callers supply evidence, not presentation policy.
const DOCUMENTATION_URL: &str = "https://folo-rs.github.io/folo/cargo-bench-history/";
const ADVISORY: &str = "Benchmark results are advisory and do not block merging.";

/// Wraps validated history evidence and the caller-paired summary in the rolling issue body.
///
/// The lifecycle supplies its checked state; composition does not classify the evidence again.
pub(crate) fn regression_issue(
    instance: &Instance,
    owner: &PendingArgs,
    evidence: &Evidence,
    state: PublicationState,
    summary: &str,
    artifact_url: Option<&str>,
) -> String {
    let mut sections = vec![
        marker::issue(instance, IssueKind::Regression),
        marker::analyzed_sha(instance, &evidence.report.commit),
        marker::run_owner(instance, owner),
        marker::state(instance, state.marker_value()),
        REGRESSION_HEADING.to_owned(),
        ADVISORY.to_owned(),
    ];
    push_result_status(&mut sections, evidence);
    sections.push(format!(
        "Analyzed commit: {}",
        evidence.report.commit.as_str()
    ));
    sections.push(summary.to_owned());
    push_links(&mut sections, artifact_url);
    join_sections(sections)
}

/// Builds the independent run-failure alert, without claiming an analysis verdict or recovery.
pub(crate) fn failure_issue(instance: &Instance, run_id: u64, run_url: &str) -> String {
    let mut sections = vec![
        marker::issue(instance, IssueKind::FailureAlert),
        marker::alert_run(instance, run_id),
        "# Benchmark-history automation failed".to_owned(),
        "Benchmark history may be incomplete. Inspect the failed run to determine whether repair is needed."
            .to_owned(),
        format!("Failed run: {run_url}"),
    ];
    push_links(&mut sections, None);
    join_sections(sections)
}

/// Composes a PR result with ownership, measured commit and disclosed collection scope.
///
/// The summary remains tool-owned prose; finish-side freshness is applied by the lifecycle.
/// The checked state is supplied by that lifecycle rather than reselected while rendering.
pub(crate) fn pr_result(
    instance: &Instance,
    owner: &PendingArgs,
    evidence: &Evidence,
    state: PublicationState,
    packages: &str,
    summary: &str,
    artifact_url: Option<&str>,
) -> String {
    let mut sections = vec![
        marker::pr_comment(instance),
        marker::analyzed_sha(instance, &evidence.report.commit),
        marker::run_owner(instance, owner),
        marker::state(instance, state.marker_value()),
        PR_HEADING.to_owned(),
        ADVISORY.to_owned(),
    ];
    push_result_status(&mut sections, evidence);
    sections.push(format_scope(packages));
    sections.push(format!(
        "Analyzed commit: {}",
        evidence.report.commit.as_str()
    ));
    sections.push(summary.to_owned());
    push_links(&mut sections, artifact_url);
    join_sections(sections)
}

/// Seeds the owned placeholder for a nonempty package scope while collection is pending.
pub(crate) fn pr_in_progress(instance: &Instance, packages: &str, owner: &PendingArgs) -> String {
    join_sections(vec![
        marker::pr_comment(instance),
        marker::in_progress(instance),
        marker::run_owner(instance, owner),
        PR_HEADING.to_owned(),
        "Benchmarking is in progress.".to_owned(),
        format_scope(packages),
    ])
}

/// Explains an explicit scope-selection result even when no rolling comment previously existed.
pub(crate) fn pr_nothing_in_scope(instance: &Instance, owner: &PendingArgs) -> String {
    join_sections(vec![
        marker::pr_comment(instance),
        marker::empty_scope(instance),
        marker::run_owner(instance, owner),
        PR_HEADING.to_owned(),
        "No benchmarkable package is affected by this pull request.".to_owned(),
    ])
}

/// Replaces an owned unfinished PR placeholder with its terminal execution notice.
pub(crate) fn pr_failed(
    instance: &Instance,
    owner: &PendingArgs,
    run_url: &str,
    conclusion: Conclusion,
) -> String {
    join_sections(vec![
        marker::pr_comment(instance),
        marker::failed(instance),
        marker::run_owner(instance, owner),
        PR_HEADING.to_owned(),
        failure_notice(conclusion).to_owned(),
        format!("Failed run: {run_url}"),
    ])
}

/// Keeps cancellation distinct from failure in terminal messages shared by both sinks.
pub(crate) fn failure_notice(conclusion: Conclusion) -> &'static str {
    match conclusion {
        Conclusion::Failure => "Benchmarking failed; no completed analysis verdict is available.",
        Conclusion::Cancelled => {
            "Benchmarking was cancelled; no completed analysis verdict is available."
        }
    }
}

/// Explains an inconclusive history analysis while its previous issue report remains intact.
pub(crate) fn inconclusive_details(
    evidence: &Evidence,
    summary: &str,
    artifact_url: Option<&str>,
) -> String {
    let mut sections = vec![format!(
        "Analysis at {} could not establish recovery.",
        evidence.report.commit.as_str()
    )];
    push_result_status(&mut sections, evidence);
    sections.push(summary.to_owned());
    push_links(&mut sections, artifact_url);
    join_sections(sections)
}

/// Describes known stale attribution with a verified distance or an explicit unknown distance.
pub(crate) fn stale_warning(distance: Option<u64>) -> String {
    match distance {
        Some(commits) => {
            let noun = if commits == 1 { "commit" } else { "commits" };
            format!("Benchmark results are {commits} {noun} behind HEAD.")
        }
        None => "Benchmark results are out of date; the commit distance is unavailable.".to_owned(),
    }
}

/// Qualifies publication when the live-head lookup cannot establish freshness at all.
pub(crate) fn freshness_unverified(subject: &str) -> String {
    format!("{subject} freshness could not be verified.")
}

/// Replaces complete owned warning blocks while preserving the surrounding report.
///
/// Preflight and finish-side publication share this operation. An unterminated block does not
/// authorize dropping the rest of the body.
pub(crate) fn insert_stale_banner(body: &str, instance: &Instance, warning: &str) -> String {
    let start = marker::stale_start(instance);
    let end = marker::stale_end(instance);
    let mut without_old = Vec::new();
    let mut lines = body.lines();

    // Only a complete owned block may be removed. An unmatched start does not authorize
    // deleting the remaining report, whose ownership cannot be inferred from that delimiter.
    while let Some(line) = lines.next() {
        if line != start {
            without_old.push(line);
            continue;
        }

        let mut stale_block = vec![line];
        let mut complete = false;
        for stale_line in lines.by_ref() {
            stale_block.push(stale_line);
            if stale_line == end {
                complete = true;
                break;
            }
        }
        if !complete {
            without_old.extend(stale_block);
        }
    }

    let quoted_warning = format!("> {warning}");
    let banner = [
        start.as_str(),
        WARNING_HEADING,
        quoted_warning.as_str(),
        end.as_str(),
    ];
    let insertion = without_old
        .iter()
        .position(|line| line.starts_with("<!-- cargo-bench-history:"))
        .and_then(|position| position.checked_add(1))
        .unwrap_or_default();
    without_old.splice(insertion..insertion, banner);
    without_old.join("\n")
}

/// Identifies placeholders eligible for exact-owner failed-state retirement.
pub(crate) fn is_in_progress(body: &str, instance: &Instance) -> bool {
    let in_progress = marker::in_progress(instance);
    body.lines().any(|line| line == in_progress)
}

/// Distinguishes restartable execution/scope notes from completed report bodies.
pub(crate) fn is_terminal_note(body: &str, instance: &Instance) -> bool {
    let empty_scope = marker::empty_scope(instance);
    let failed = marker::failed(instance);
    body.lines()
        .any(|line| line == empty_scope || line == failed)
}

/// Adds coverage qualifications and the outcome headline without replacing domain prose.
fn push_result_status(sections: &mut Vec<String>, evidence: &Evidence) {
    let outcome = evidence.report.outcome;
    if !evidence.platforms.is_complete() {
        sections.push(format!(
            "{WARNING_HEADING}\n> Partial platform coverage. Completed: {}. Missing: {}.\n\
             > Findings and absence-of-findings statements apply only to completed platforms.",
            evidence.platforms.completed().join(", "),
            evidence.platforms.missing().join(", ")
        ));
    }
    if evidence.report.coverage == Coverage::Partial {
        sections.push(format!(
            "{WARNING_HEADING}\n> Some in-scope metric series could not be judged. \
             See the report for coverage details."
        ));
    }
    let headline = match outcome {
        Outcome::Findings => "Notable benchmark changes detected.",
        Outcome::Clean if evidence.platforms.is_complete() => {
            "No notable changes detected across the completed collection."
        }
        Outcome::Clean => "No notable changes detected on the completed platforms only.",
        Outcome::InsufficientBaseline => "Insufficient evidence to judge the in-scope series.",
        Outcome::NothingInScope => "No metric series was analyzed at this commit.",
        Outcome::Partial => "No notable changes detected among the series that could be judged.",
    };
    sections.push(headline.to_owned());
}

/// Renders the package selection consistently across pending and completed PR comments.
fn format_scope(packages: &str) -> String {
    let packages = packages
        .split(',')
        .map(str::trim)
        .filter(|one| !one.is_empty())
        .map(|one| format!("`{one}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("Packages benchmarked: {packages}")
}

/// Adds the optional report artifact and the fixed reading guide to standard messages.
fn push_links(sections: &mut Vec<String>, artifact_url: Option<&str>) {
    if let Some(url) = artifact_url {
        sections.push(format!("[Download the full report bundle]({url})"));
    }
    sections.push(format!("[How to read this report]({DOCUMENTATION_URL})"));
}

/// Assembles optional message sections without introducing empty presentation blocks.
fn join_sections(sections: Vec<String>) -> String {
    sections
        .into_iter()
        .filter(|one| !one.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::num::NonZero;

    use super::*;
    use crate::cli::RunArgs;
    use crate::model::CommitSha;
    use crate::result::AnalysisMode;
    use crate::result::tests::evidence;

    fn instance() -> Instance {
        "default".parse().unwrap()
    }

    fn sha() -> CommitSha {
        "0123456789abcdef0123456789abcdef01234567".parse().unwrap()
    }

    fn owner() -> PendingArgs {
        PendingArgs {
            run: RunArgs {
                run_id: NonZero::new(1).unwrap(),
                run_attempt: NonZero::new(1).unwrap(),
            },
            head: sha(),
        }
    }

    #[test]
    fn regression_issue_embeds_domain_summary_without_interpreting_it() {
        let body = regression_issue(
            &instance(),
            &owner(),
            &evidence(AnalysisMode::History, Outcome::Findings, true),
            PublicationState::Findings,
            "DOMAIN SUMMARY",
            Some("https://example.test/artifact"),
        );

        assert!(body.starts_with("<!-- cargo-bench-history:default:issue:regression -->"));
        assert!(body.contains("DOMAIN SUMMARY"));
        assert!(body.contains("Benchmark results are advisory and do not block merging."));
        assert!(body.contains("[Download the full report bundle](https://example.test/artifact)"));
        assert!(body.contains(
            "[How to read this report](https://folo-rs.github.io/folo/cargo-bench-history/)"
        ));
    }

    #[test]
    fn standard_messages_link_documentation_without_fabricating_an_artifact() {
        let result = evidence(AnalysisMode::Branch, Outcome::Findings, true);
        for body in [
            regression_issue(
                &instance(),
                &owner(),
                &evidence(AnalysisMode::History, Outcome::Clean, true),
                PublicationState::Clean,
                "Clean summary",
                None,
            ),
            failure_issue(&instance(), 1, "https://example.test/run"),
            pr_result(
                &instance(),
                &owner(),
                &result,
                PublicationState::Findings,
                "foo",
                "Tool summary",
                None,
            ),
        ] {
            assert!(body.contains(
                "[How to read this report](https://folo-rs.github.io/folo/cargo-bench-history/)"
            ));
            assert!(!body.contains("[Download the full report bundle]"));
        }
    }

    #[test]
    fn pr_result_embeds_summary_and_report_artifact_with_standard_advice() {
        let body = pr_result(
            &instance(),
            &owner(),
            &evidence(AnalysisMode::Branch, Outcome::Findings, true),
            PublicationState::Findings,
            "foo",
            "Tool summary\n\n| Metric | Value |\n| --- | --- |\n| Time | +10% |",
            Some("https://example.test/artifact"),
        );
        assert!(body.starts_with(&marker::pr_comment(&instance())));
        assert!(body.contains("Benchmark results are advisory and do not block merging."));
        assert!(
            body.contains("Tool summary\n\n| Metric | Value |\n| --- | --- |\n| Time | +10% |")
        );
        assert!(body.contains("[Download the full report bundle](https://example.test/artifact)"));
    }

    #[test]
    fn stale_banner_replaces_a_previous_banner() {
        let instance = instance();
        let body = pr_result(
            &instance,
            &owner(),
            &evidence(AnalysisMode::Branch, Outcome::Findings, true),
            PublicationState::Findings,
            "foo, bar",
            "summary",
            None,
        );
        let stale = insert_stale_banner(&body, &instance, "old warning");
        let refreshed = insert_stale_banner(&stale, &instance, "new warning");

        assert!(!refreshed.contains("old warning"));
        assert!(refreshed.starts_with(&marker::pr_comment(&instance)));
        assert_eq!(refreshed.matches("new warning").count(), 1);
        assert_eq!(refreshed.matches(WARNING_HEADING).count(), 1);
    }

    #[test]
    fn stale_banner_is_inserted_after_the_identity_marker() {
        let instance = instance();
        let body = format!("{}\nbody", marker::pr_comment(&instance));
        let refreshed = insert_stale_banner(&body, &instance, "warning");
        let expected = format!(
            "{}\n{}\n{}\n> warning\n{}\nbody",
            marker::pr_comment(&instance),
            marker::stale_start(&instance),
            WARNING_HEADING,
            marker::stale_end(&instance)
        );
        assert_eq!(refreshed, expected);
    }

    #[test]
    fn unterminated_banner_is_preserved_before_a_new_banner() {
        let instance = instance();
        let body = format!(
            "{}\n{}\nold",
            marker::pr_comment(&instance),
            marker::stale_start(&instance)
        );
        let refreshed = insert_stale_banner(&body, &instance, "new warning");

        assert!(refreshed.contains("old"));
        assert!(refreshed.contains("new warning"));
    }

    #[test]
    fn scope_is_trimmed_and_rendered_consistently() {
        let body = pr_in_progress(&instance(), "foo, bar", &owner());
        assert!(body.contains("Packages benchmarked: `foo`, `bar`"));
        assert!(is_in_progress(&body, &instance()));
        assert!(!is_in_progress(
            &pr_nothing_in_scope(&instance(), &owner()),
            &instance()
        ));
    }

    #[test]
    fn failure_and_cancellation_have_distinct_terminal_notices() {
        let failed = pr_failed(
            &instance(),
            &owner(),
            "https://example.test/run",
            Conclusion::Failure,
        );
        let cancelled = pr_failed(
            &instance(),
            &owner(),
            "https://example.test/run",
            Conclusion::Cancelled,
        );
        assert!(failed.contains("Benchmarking failed"));
        assert!(!failed.contains("was cancelled"));
        assert!(cancelled.contains("Benchmarking was cancelled"));
        assert_ne!(failed, cancelled);
    }

    #[test]
    fn partial_series_do_not_hide_findings_or_missing_platforms() {
        let mut result = evidence(AnalysisMode::Branch, Outcome::Findings, false);
        result.report.coverage = Coverage::Partial;
        let body = pr_result(
            &instance(),
            &owner(),
            &result,
            PublicationState::Findings,
            "foo",
            "exact tool summary",
            None,
        );
        assert!(body.contains("Notable benchmark changes detected."));
        assert!(body.contains("Some in-scope metric series could not be judged."));
        assert!(body.contains("Missing: windows."));
        assert!(body.contains("exact tool summary"));
    }

    #[test]
    fn silent_outcomes_select_distinct_qualified_messages() {
        let cases = [
            (
                Outcome::Clean,
                "No notable changes detected across the completed collection.",
            ),
            (
                Outcome::InsufficientBaseline,
                "Insufficient evidence to judge",
            ),
            (Outcome::NothingInScope, "No metric series was analyzed"),
            (Outcome::Partial, "among the series that could be judged"),
        ];
        for (outcome, expected) in cases {
            let result = evidence(AnalysisMode::Branch, outcome, true);
            let state = result.require_state(result.publication_state()).unwrap();
            let body = pr_result(
                &instance(),
                &owner(),
                &result,
                state,
                "foo",
                "tool summary",
                None,
            );
            assert!(body.contains(expected), "{body}");
            assert!(!body.contains("Missing:"));
        }
        let result = evidence(AnalysisMode::Branch, Outcome::Clean, false);
        let state = result
            .require_state(PublicationState::Inconclusive)
            .unwrap();
        let body = pr_result(
            &instance(),
            &owner(),
            &result,
            state,
            "foo",
            "tool summary",
            None,
        );
        assert!(body.contains("completed platforms only"));
        assert!(!body.contains("across the completed collection"));
    }
}
