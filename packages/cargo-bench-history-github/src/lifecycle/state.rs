use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::AppError;

use crate::cli::PendingArgs;
use crate::model::{CommitSha, Instance, IssueKind};
use crate::result::{AnalysisMode, Evidence, PublicationState};
use crate::{marker, message};

/// A successful analysis and its independently identified workflow writer.
#[derive(Clone, Debug)]
pub(crate) struct Report {
    pub(crate) owner: PendingArgs,
    pub(crate) evidence: Evidence,
    pub(crate) summary: String,
    pub(crate) artifact_url: Option<String>,
}

impl Report {
    pub(crate) fn validate(
        &self,
        mode: AnalysisMode,
        state: PublicationState,
    ) -> Result<(), AppError> {
        self.evidence.report.require_mode(mode)?;
        self.evidence.require_state(state)?;
        if self.owner.head != self.evidence.report.commit {
            return Err(InvalidPublication::new().into());
        }
        Ok(())
    }
}

/// No complete verdict is distinct from missing evidence or failed execution.
#[derive(Debug)]
pub(crate) enum NoData {
    Empty(PendingArgs),
    Report(Report),
}

impl NoData {
    pub(crate) fn owner(&self) -> &PendingArgs {
        match self {
            Self::Empty(owner) => owner,
            Self::Report(report) => &report.owner,
        }
    }

    pub(crate) fn validate(&self, mode: AnalysisMode) -> Result<(), AppError> {
        match self {
            Self::Empty(_) => Ok(()),
            Self::Report(report) => report.validate(mode, PublicationState::NoData),
        }
    }

    pub(crate) fn details(&self) -> String {
        match self {
            Self::Empty(_) => {
                "No benchmarkable packages were selected; this run cannot establish recovery."
                    .to_owned()
            }
            Self::Report(report) => message::no_data_details(
                &report.evidence,
                &report.summary,
                report.artifact_url.as_deref(),
            ),
        }
    }
}

/// Interpreted rolling body, separating a retained report from one bounded run annotation.
pub(crate) struct IssueBody<'a> {
    pub(crate) report: &'a str,
    pub(crate) owner: PendingArgs,
    pub(crate) commit: CommitSha,
    pub(crate) annotation: Option<Annotation>,
}

impl<'a> IssueBody<'a> {
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
                    Some("no-data") => AnnotationState::NoData,
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

    pub(crate) fn latest_owner(&self) -> &PendingArgs {
        self.annotation
            .as_ref()
            .map_or(&self.owner, |annotation| &annotation.owner)
    }
}

/// The lifecycle status of the run described above a retained report.
pub(crate) struct Annotation {
    pub(crate) owner: PendingArgs,
    pub(crate) state: AnnotationState,
}

/// Terminal annotations remain interpreted but no longer belong to unfinished work.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum AnnotationState {
    Preflight,
    NoData,
    Failed,
}

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

pub(crate) fn superseded(existing: &PendingArgs, incoming: &PendingArgs) -> bool {
    existing.run > incoming.run
}

/// Contradictory publication inputs cannot authorize a GitHub mutation.
#[ohno::error]
#[display("Publication ownership or scope is inconsistent with the supplied evidence")]
pub(crate) struct InvalidPublication;

/// A matching issue is reserved but its body cannot safely drive a lifecycle transition.
#[ohno::error]
#[display("Matching issue has an uninterpretable body; preserving it")]
pub(crate) struct UninterpretableIssue;

// These diagnostics expose no mutation of their source chains.
impl UnwindSafe for InvalidPublication {}
impl RefUnwindSafe for InvalidPublication {}
impl UnwindSafe for UninterpretableIssue {}
impl RefUnwindSafe for UninterpretableIssue {}
