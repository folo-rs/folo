use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use cbh_model::{BackfillCompletion, BackfillStop};
use ohno::AppError;
use reqwest::Url;

use crate::message::result_headline;
use crate::model::CommitSha;
use crate::result::{
    AnalysisReport, Coverage, Evidence, Execution, Outcome, PlatformCoverage, platform_list,
};
use crate::summary::input::{
    BackfillPlatform, Publication, ReportDetails, ResultInput, SummaryInput, UnavailableReason,
};

/// Renders a workflow card from JSON and caller-supplied evidence.
///
/// Evidence paths are passed unchanged to `read`; the caller supplies their resolution context.
///
/// # Errors
///
/// Returns an error for invalid inputs, inconsistent evidence or a failed evidence read.
pub fn render_workflow_summary(
    input: &[u8],
    read: impl FnMut(&Path) -> Result<Vec<u8>, AppError>,
) -> Result<String, AppError> {
    let input = serde_json::from_slice(input).map_err(InvalidSummary::caused_by)?;
    render(input, read)
}

/// Projects explicit job facts and validated evidence without scraping logs or report prose.
pub(crate) fn render(
    input: SummaryInput,
    mut read: impl FnMut(&Path) -> Result<Vec<u8>, AppError>,
) -> Result<String, AppError> {
    let run_url = link(&input.run_url)?;
    let artifact_url = input.artifact_url.as_deref().map(link).transpose()?;
    let mut body = format!(
        "## Benchmark workflow results\n\n\
         | Status | Result |\n| --- | --- |\n\
         | Execution | {} |\n| GitHub publication | {} |\n",
        input.execution.label(),
        input.publication.label(),
    );
    match input.result {
        ResultInput::Analysis {
            report_file,
            analyzed_sha,
            expected_platforms,
            completed_platforms,
            packages,
            observed_head,
        } => {
            if input.execution == Execution::Skipped {
                return Err(InvalidSummary::new().into());
            }
            let commit = analyzed_sha.parse()?;
            let json = String::from_utf8(read(&report_file)?).map_err(InvalidSummary::caused_by)?;
            let evidence = Evidence {
                report: AnalysisReport::parse(&json, &commit)?,
                platforms: PlatformCoverage::parse(&expected_platforms, &completed_platforms)?,
            };
            let details: ReportDetails =
                serde_json::from_str(&json).map_err(InvalidSummary::caused_by)?;
            analysis(
                &mut body,
                &evidence,
                &details,
                &packages,
                observed_head.as_deref(),
            )?;
        }
        ResultInput::Unavailable { reason } => {
            body.push_str("| Analysis | Not available; no benchmark verdict |\n\n");
            body.push_str(unavailable(reason));
        }
        ResultInput::Backfill {
            from,
            to,
            expected_platforms,
            platforms,
        } => {
            if !matches!(
                input.publication,
                Publication::NotRequested | Publication::Disabled
            ) {
                return Err(InvalidSummary::new().into());
            }
            let from: CommitSha = from.parse()?;
            let to: CommitSha = to.parse()?;
            body.push_str(
                "| Analysis | Not requested; collection-only backfill has no benchmark verdict |\n",
            );
            writeln!(
                body,
                "\nFrozen inclusive range: `{}` to `{}`.\n",
                from.as_str(),
                to.as_str()
            )
            .expect("writing to String cannot fail");
            backfill(
                &mut body,
                &from,
                &to,
                &expected_platforms,
                platforms,
                &mut read,
            )?;
        }
    }
    if input.execution == Execution::Failure || input.execution == Execution::Cancelled {
        body.push_str("\n\nExecution did not complete successfully. Any available evidence covers only completed work; inspect the job logs. Partial storage writes may remain.");
    }
    write!(body, "\n\n[Workflow run and diagnostics]({run_url})")
        .expect("writing to String cannot fail");
    if let Some(url) = artifact_url {
        write!(body, " - [Full report or completion artifacts]({url})")
            .expect("writing to String cannot fail");
    } else {
        body.push_str(
            "\n\nNo artifact link is available; inspect the workflow run for retained files.",
        );
    }
    body.push('\n');
    Ok(body)
}

/// Reuses the publication evidence gate while keeping the analyzer's outcome visible separately.
fn analysis(
    body: &mut String,
    evidence: &Evidence,
    details: &ReportDetails,
    packages: &[String],
    observed_head: Option<&str>,
) -> Result<(), AppError> {
    let report = &evidence.report;
    let freshness = match observed_head.map(str::parse::<CommitSha>).transpose()? {
        Some(head) if head == report.commit => "Matches the observed head",
        Some(_) => "Out of date relative to the observed head; distance not established",
        None => "Live-head freshness not checked",
    };
    writeln!(
        body,
        "| Analysis | {} |\n| Report disposition | {} |\n\
         | Judged coverage | {} / {} in-scope metric series |\n\
         | Context | `{}` |\n| Freshness | {freshness} |\n\
         | Completed platforms | {} |\n| Missing platforms | {} |\n",
        result_headline(evidence),
        evidence.publication_state().as_str(),
        report.judged,
        report.in_scope,
        report.commit.as_str(),
        inventory(evidence.platforms.completed()),
        if evidence.platforms.is_complete() {
            "None".to_owned()
        } else {
            inventory(evidence.platforms.missing())
        },
    )
    .expect("writing to String cannot fail");
    if !packages.is_empty() {
        writeln!(body, "Selected packages: {}.\n", inventory(packages))
            .expect("writing to String cannot fail");
    } else {
        body.push_str("Package selection: see collection inputs in the workflow run.\n\n");
    }
    if !details.sets.is_empty() {
        let partitions = details
            .sets
            .iter()
            .map(|set| {
                format!(
                    "{} / {} / {}",
                    set.engine, set.target_triple, set.machine_key
                )
            })
            .collect::<Vec<_>>();
        writeln!(
            body,
            "Selected engine / target / machine: {}.\n",
            inventory(&partitions)
        )
        .expect("writing to String cannot fail");
    }
    if !evidence.platforms.is_complete() {
        body.push_str("**Incomplete platform coverage.** Findings and absence-of-findings statements apply only to completed platforms.\n\n");
    }
    if matches!(report.coverage, Coverage::NothingJudged | Coverage::Partial) {
        body.push_str(
            "**Incomplete series evidence.** Findings can coexist with unjudged series.\n\n",
        );
    }
    if let Some(hint) = &details.hint {
        writeln!(body, "Analysis diagnostic: {}.\n", text(hint))
            .expect("writing to String cannot fail");
    }
    let next = match report.outcome {
        Outcome::Findings => {
            "Open the report artifacts and inspect the finding's qualified examine command and evidence."
        }
        Outcome::InsufficientBaseline | Outcome::Partial => {
            "Collect or backfill distinct comparable commits, then analyze again. Check the report's per-reason coverage and selected engine, target and machine. Repeating the same commit does not add independent history; reaching minimum evidence does not guarantee a reportable result."
        }
        Outcome::NothingInScope => {
            "Inspect the analysis diagnostic and collection logs for selected packages, targets and artifact roots. Confirm that a supported engine actually emitted compatible measurement files; empty output alone does not identify compile-only, smoke or unsupported-engine use."
        }
        Outcome::Clean if !evidence.platforms.is_complete() => {
            "Inspect the missing platform jobs and rerun the failed collection before treating the whole workflow as clean."
        }
        Outcome::Clean => {
            "Retain the report and continue collecting comparable commits for subsequent comparisons."
        }
    };
    write!(body, "**Next action:** {next}").expect("writing to String cannot fail");
    Ok(())
}

/// Missing matrix legs remain unknown, never zero-count successful passes.
fn backfill(
    body: &mut String,
    from: &CommitSha,
    to: &CommitSha,
    expected: &str,
    platforms: Vec<BackfillPlatform>,
    read: &mut impl FnMut(&Path) -> Result<Vec<u8>, AppError>,
) -> Result<(), AppError> {
    let expected = platform_list(expected)?;
    let mut seen = BTreeSet::new();
    body.push_str("| Platform | Execution | Stored | Skipped existing | Empty | Failed commits | Deferred | Stop reason |\n| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |\n");
    let mut overwrote = false;
    for platform in platforms {
        if !expected.contains(&platform.platform) || !seen.insert(platform.platform.clone()) {
            return Err(InvalidSummary::new().into());
        }
        let label = platform.execution.label();
        let name = text(&platform.platform);
        if let Some(path) = platform.completion_file {
            let completion =
                BackfillCompletion::from_slice(&read(&path)?).map_err(InvalidSummary::caused_by)?;
            if completion.from() != from.as_str()
                || completion.to() != to.as_str()
                || platform.execution == Execution::Skipped
                || (completion.stop_reason() == BackfillStop::BenchmarkFailure
                    && platform.execution == Execution::Success)
            {
                return Err(InvalidSummary::new().into());
            }
            overwrote |= completion.overwrite();
            let counts = completion.counts();
            let stop = match completion.stop_reason() {
                BackfillStop::RangeExhausted => "Range exhausted",
                BackfillStop::CommitLimit => "Commit limit reached (bounded completion)",
                BackfillStop::BenchmarkFailure => "Stopped on benchmark failure",
            };
            writeln!(
                body,
                "| {name} | {label} | {} | {} | {} | {} | {} | {stop} |",
                counts.stored, counts.skipped, counts.empty, counts.failed, counts.deferred
            )
            .expect("writing to String cannot fail");
        } else {
            writeln!(
                body,
                "| {name} | {label} | - | - | - | - | - | Completion unavailable |"
            )
            .expect("writing to String cannot fail");
        }
    }
    for missing in expected.difference(&seen) {
        writeln!(
            body,
            "| {} | Unavailable | - | - | - | - | - | No platform result |",
            text(missing)
        )
        .expect("writing to String cannot fail");
    }
    body.push_str("\nCounts describe commits, not benchmark verdicts. Missing completion evidence leaves counts unknown; an interrupted or failed job may already have written some results.\n\n**Next action:** For deferred work or an interrupted pass, rerun the same frozen range with the same collection settings and partition, using skip-existing mode. Inspect failed-commit and empty-collection logs before retrying; skip-existing preserves recorded commits, including partially populated engine partitions.");
    if overwrote {
        body.push_str(" This pass used overwrite: repeating overwrite starts from the newest commit again rather than resuming.");
    }
    Ok(())
}

/// No-analysis explanations name the observed boundary, not a speculative benchmark cause.
fn unavailable(reason: UnavailableReason) -> &'static str {
    match reason {
        UnavailableReason::EmptyScope => {
            "No benchmarkable package was selected. **Next action:** Review the selected package scope and exclusions; collection and analysis did not run."
        }
        UnavailableReason::ForkPullRequest => {
            "Fork pull-request policy skipped benchmark storage and publication. **Next action:** Run an authorized same-repository workflow when measurements are required."
        }
        UnavailableReason::NoEligibleCommit => {
            "No commit met the backfill endpoint age policy. **Next action:** Wait for an eligible commit or select an explicit range; no backfill matrix was started."
        }
        UnavailableReason::ReportsUnavailable => {
            "No validated analysis report is available. **Next action:** Inspect preparation, collection and analysis logs; do not interpret process success or a missing artifact as a clean benchmark result."
        }
        UnavailableReason::CollectionOnly => {
            "This pass collected measurements without requesting analysis. **Next action:** Run analysis separately when comparable history is available; storage does not establish a benchmark verdict."
        }
    }
}

/// Bounds inventories so the card remains compact while disclosing omissions.
fn inventory(values: &[String]) -> String {
    // Reserve the card for decisions; full inventories remain in reports and job inputs.
    const VISIBLE_ITEMS: usize = 3;
    let mut result = values
        .iter()
        .take(VISIBLE_ITEMS)
        .map(|value| format!("<code>{}</code>", text(value)))
        .collect::<Vec<_>>()
        .join(", ");
    if values.len() > VISIBLE_ITEMS {
        write!(result, " ({} total; see artifacts/logs)", values.len())
            .expect("writing to String cannot fail");
    }
    result
}

/// Keeps externally supplied identifiers and diagnostics literal and bounded in Markdown.
fn text(value: &str) -> String {
    // Detailed diagnostics belong in logs; this limit bounds each card cell or hint.
    const VISIBLE_CHARACTERS: usize = 400;
    let mut chars = value.chars();
    let short = chars.by_ref().take(VISIBLE_CHARACTERS).collect::<String>();
    let suffix = if chars.next().is_some() { "..." } else { "" };
    format!("{short}{suffix}")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace(['\r', '\n'], " ")
        .replace('`', "&#96;")
        .replace('*', "&#42;")
        .replace('_', "&#95;")
        .replace('[', "&#91;")
        .replace(']', "&#93;")
}

/// Allows only ordinary web links and encodes Markdown delimiters.
fn link(value: &str) -> Result<String, AppError> {
    let url = Url::parse(value).map_err(InvalidSummary::caused_by)?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || value.chars().any(char::is_control)
    {
        return Err(InvalidSummary::new().into());
    }
    Ok(url
        .as_str()
        .replace('(', "%28")
        .replace(')', "%29")
        .replace('<', "%3C")
        .replace('>', "%3E"))
}

/// Contradictory workflow evidence cannot become a reassuring result card.
#[ohno::error]
#[display("Invalid workflow summary inputs or inconsistent completion evidence")]
pub(crate) struct InvalidSummary;
