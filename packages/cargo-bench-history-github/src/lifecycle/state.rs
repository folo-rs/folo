use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::AppError;

use crate::cli::PendingArgs;
use crate::model::{CommitSha, Instance, IssueKind};
use crate::result::{AnalysisMode, Evidence, PublicationState};
use crate::{marker, message};

/// A successful analysis and its independently identified workflow writer.
///
/// File loading assembles this input for report-bearing lifecycle commands. The caller pairs
/// the summary with its JSON; the lifecycle validates sink/state and ownership before mutation.
#[derive(Clone, Debug)]
pub(crate) struct Report {
    pub(crate) owner: PendingArgs,
    pub(crate) evidence: Evidence,
    pub(crate) summary: String,
    pub(crate) artifact_url: Option<String>,
}

impl Report {
    /// Checks the sink, requested state and measured ownership before lifecycle discovery.
    ///
    /// The caller keeps JSON and summary paired; this validates the metadata and nonblank
    /// summary without reconstructing the tool's domain rendering.
    pub(crate) fn validate(
        &self,
        mode: AnalysisMode,
        state: PublicationState,
    ) -> Result<PublicationState, AppError> {
        if self.summary.trim().is_empty() {
            return Err(EmptySummary::new().into());
        }
        self.evidence.report.require_mode(mode)?;
        let state = self.evidence.require_state(state)?;
        if self.owner.head != self.evidence.report.commit {
            return Err(InvalidPublication::new().into());
        }
        Ok(state)
    }
}

/// Publication without a complete verdict, backed by a report or explicit empty scope.
///
/// This keeps absent benchmark scope distinct from an inconclusive completed analysis so
/// comments and retained-issue annotations can choose the appropriate explanation.
#[derive(Debug)]
pub(crate) enum Inconclusive {
    Empty(PendingArgs),
    Report(Report),
}

impl Inconclusive {
    /// Supplies ownership uniformly for explicit empty scope and report-backed inconclusive.
    pub(crate) fn owner(&self) -> &PendingArgs {
        match self {
            Self::Empty(owner) => owner,
            Self::Report(report) => &report.owner,
        }
    }

    /// Returns the checked inconclusive state, retaining explicit scope as a separate input form.
    pub(crate) fn validate(&self, mode: AnalysisMode) -> Result<PublicationState, AppError> {
        match self {
            Self::Empty(_) => Ok(PublicationState::Inconclusive),
            Self::Report(report) => report.validate(mode, PublicationState::Inconclusive),
        }
    }

    /// Supplies annotation prose without replacing the issue's retained report.
    pub(crate) fn details(&self) -> String {
        match self {
            Self::Empty(_) => {
                "No benchmarkable packages were selected; this run cannot establish recovery."
                    .to_owned()
            }
            Self::Report(report) => message::inconclusive_details(
                &report.evidence,
                &report.summary,
                report.artifact_url.as_deref(),
            ),
        }
    }
}

/// Interpreted rolling body, separating a retained report from one bounded run annotation.
///
/// Discovery establishes this view before lifecycle code decides whether to replace results
/// or update status without losing the report's measured commit and ownership.
pub(crate) struct IssueBody<'a> {
    pub(crate) report: &'a str,
    pub(crate) owner: PendingArgs,
    pub(crate) commit: CommitSha,
    pub(crate) annotation: Option<Annotation>,
}

impl<'a> IssueBody<'a> {
    /// Interprets coherent issue markers while keeping retained results separate from status.
    ///
    /// Discovery and lifecycle guards use this view before deciding which part may change.
    pub(crate) fn parse(body: &'a str, instance: &Instance) -> Result<Self, AppError> {
        let (report, annotation) = split_annotation(body, instance)?;
        let identity = marker::issue(instance, IssueKind::Regression);
        if report.lines().filter(|line| *line == identity).count() != 1
            || !matches!(
                marker::find_state(report, instance),
                Some("findings" | "clean")
            )
        {
            return Err(UninterpretableIssue::new().into());
        }
        let owner = marker::find_owner(report, instance).ok_or_else(UninterpretableIssue::new)?;
        let commit =
            marker::find_analyzed_sha(report, instance).ok_or_else(UninterpretableIssue::new)?;
        if owner.head != commit {
            return Err(UninterpretableIssue::new().into());
        }
        let annotation = annotation
            .map(|body| {
                let state = match marker::find_state(body, instance) {
                    Some("preflight") => AnnotationState::Preflight,
                    Some("no-data") => AnnotationState::Inconclusive,
                    Some("failed") => AnnotationState::Failed,
                    _ => return Err(UninterpretableIssue::new()),
                };
                let owner =
                    marker::find_owner(body, instance).ok_or_else(UninterpretableIssue::new)?;
                Ok(Annotation { owner, state })
            })
            .transpose()?;
        Ok(Self {
            report,
            owner,
            commit,
            annotation,
        })
    }

    /// Selects annotation ownership when present without treating it as the measured commit.
    pub(crate) fn latest_owner(&self) -> &PendingArgs {
        self.annotation
            .as_ref()
            .map_or(&self.owner, |annotation| &annotation.owner)
    }
}

/// Ownership and lifecycle status carried alongside a retained report.
pub(crate) struct Annotation {
    pub(crate) owner: PendingArgs,
    pub(crate) state: AnnotationState,
}

/// Terminal annotations remain interpreted but no longer belong to unfinished work.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum AnnotationState {
    Preflight,
    Inconclusive,
    Failed,
}

/// Isolates one complete owned annotation without adopting ambiguous or partial delimiters.
fn split_annotation<'a>(
    body: &'a str,
    instance: &Instance,
) -> Result<(&'a str, Option<&'a str>), AppError> {
    let start = marker::annotation_start(instance);
    let end = marker::annotation_end(instance);
    if !body.contains(&start) && !body.contains(&end) {
        return Ok((body, None));
    }
    if body.matches(&start).count() != 1 || body.matches(&end).count() != 1 {
        return Err(UninterpretableIssue::new().into());
    }
    let (report, annotation) = body
        .split_once(&format!("\n\n{start}\n"))
        .ok_or_else(UninterpretableIssue::new)?;
    let annotation = annotation
        .strip_suffix(&format!("\n{end}"))
        .ok_or_else(UninterpretableIssue::new)?;
    Ok((report, Some(annotation)))
}

/// Appends a replacement lifecycle annotation to the retained report portion.
///
/// Callers pass `IssueBody::report`, not the previously annotated whole body, to keep one
/// bounded status block while retaining report ownership and measured-commit metadata.
pub(crate) fn annotate(
    report: &str,
    instance: &Instance,
    owner: &PendingArgs,
    state: &str,
    details: &str,
) -> String {
    format!(
        "{report}\n\n{}\n{}\n{}\n{details}\n{}",
        marker::annotation_start(instance),
        marker::run_owner(instance, owner),
        marker::state(instance, state),
        marker::annotation_end(instance)
    )
}

/// Applies attempt precedence only to two writers belonging to the same workflow run.
///
/// Cross-run authority is decided by commit/live-head guards and serialized same-commit
/// publication, not by this predicate.
pub(crate) fn superseded(existing: &PendingArgs, incoming: &PendingArgs) -> bool {
    // Run IDs identify writers, not chronology. Distinct runs rely on freshness guards
    // and serialized arrival order at the same commit. Ref: docs/design.md, Run ownership.
    existing.run.run_id == incoming.run.run_id
        && existing.run.run_attempt > incoming.run.run_attempt
}

/// Contradictory publication inputs cannot authorize a GitHub mutation.
#[ohno::error]
#[display("Publication ownership or scope is inconsistent with the supplied evidence")]
pub(crate) struct InvalidPublication;

/// A matching issue is reserved but its body cannot safely drive a lifecycle transition.
#[ohno::error]
#[display("Matching issue has an uninterpretable body; preserving it")]
pub(crate) struct UninterpretableIssue;

/// Report-backed publication needs the analyzer's rendered details.
#[ohno::error]
#[display("Report summary must contain non-whitespace content")]
struct EmptySummary;

// These diagnostics expose no mutation of their source chains.
impl UnwindSafe for InvalidPublication {}
impl RefUnwindSafe for InvalidPublication {}
impl UnwindSafe for UninterpretableIssue {}
impl RefUnwindSafe for UninterpretableIssue {}
impl UnwindSafe for EmptySummary {}
impl RefUnwindSafe for EmptySummary {}
