use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::AppError;

use crate::github::{GitHub, Issue};
use crate::marker;
use crate::operations::Context;

/// Distinguishes marker ownership from an explicitly authorized initial adoption.
#[derive(Debug)]
pub(crate) struct IssueTarget {
    pub(crate) issue: Issue,
    pub(crate) legacy: bool,
}

pub(crate) async fn find_issue_target(
    github: &impl GitHub,
    context: &Context,
    identity: &str,
) -> Result<Option<IssueTarget>, AppError> {
    let issues = github.open_issues(&context.repository).await?;
    let target = select_issue(issues, identity, context.migration.issue_title.as_deref())?;
    note_legacy_target(context, target.as_ref());
    Ok(target)
}

// Explanatory stderr has no state-transition effect and is not coupled to unit-test globals.
#[cfg_attr(test, mutants::skip)]
fn note_legacy_target(context: &Context, target: Option<&IssueTarget>) {
    if context.verbose
        && let Some(target) = target
        && target.legacy
    {
        eprintln!(
            "Selected legacy issue {} in {}: no current marker matched, and exactly one unowned bot-authored issue has the explicit title {:?}.",
            target.issue.number, context.repository, target.issue.title
        );
    }
}

fn select_issue(
    issues: Vec<Issue>,
    identity: &str,
    legacy_title: Option<&str>,
) -> Result<Option<IssueTarget>, AppError> {
    if let Some(issue) = issues
        .iter()
        .find(|issue| issue.body.lines().any(|line| line == identity))
    {
        return Ok(Some(IssueTarget {
            issue: issue.clone(),
            legacy: false,
        }));
    }
    let Some(title) = legacy_title else {
        return Ok(None);
    };
    let mut candidates = issues
        .into_iter()
        .filter(|issue| issue.bot_authored && issue.title == title);
    let Some(issue) = candidates.next() else {
        return Ok(None);
    };
    if candidates.next().is_some() {
        return Err(AmbiguousLegacyIssue::new(title).into());
    }
    if marker::has_issue_identity(&issue.body) {
        return Err(AlreadyManagedLegacyIssue::new(issue.number).into());
    }
    Ok(Some(IssueTarget {
        issue,
        legacy: true,
    }))
}

/// A title alone cannot choose between multiple legacy automation issues.
#[ohno::error]
#[display("Multiple bot-authored open issues have the legacy title '{title}'")]
pub(crate) struct AmbiguousLegacyIssue {
    title: String,
}

/// Explicit title adoption must not take ownership from another instance or issue kind.
#[ohno::error]
#[display("Legacy issue {number} already carries a different companion issue identity")]
pub(crate) struct AlreadyManagedLegacyIssue {
    number: u64,
}

impl UnwindSafe for AmbiguousLegacyIssue {}
impl RefUnwindSafe for AmbiguousLegacyIssue {}
impl UnwindSafe for AlreadyManagedLegacyIssue {}
impl RefUnwindSafe for AlreadyManagedLegacyIssue {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::model::{Instance, IssueKind};

    fn issue(number: u64, title: &str, bot_authored: bool, body: &str) -> Issue {
        Issue {
            number,
            title: title.to_owned(),
            bot_authored,
            body: body.to_owned(),
        }
    }

    fn identity(instance: &str, kind: IssueKind) -> String {
        marker::issue(&instance.parse::<Instance>().unwrap(), kind)
    }

    #[test]
    fn title_fallback_is_exact_bot_only_and_opt_in() {
        let identity = identity("current", IssueKind::Regression);
        let issues = vec![
            issue(1, "Legacy", false, "Human issue"),
            issue(2, "legacy", true, "Different case"),
            issue(3, "Legacy ", true, "Different whitespace"),
        ];
        assert!(
            select_issue(issues.clone(), &identity, Some("Legacy"))
                .unwrap()
                .is_none()
        );
        let mut issues = issues;
        issues.push(issue(4, "Legacy", true, "Automation report"));
        assert!(
            select_issue(issues.clone(), &identity, None)
                .unwrap()
                .is_none()
        );
        let selected = select_issue(issues, &identity, Some("Legacy"))
            .unwrap()
            .unwrap();
        assert_eq!(selected.issue.number, 4);
        assert!(selected.legacy);
    }

    #[test]
    fn current_marker_wins_even_when_legacy_titles_are_ambiguous() {
        let identity = identity("current", IssueKind::Regression);
        let selected = select_issue(
            vec![
                issue(1, "Legacy", true, "old"),
                issue(2, "Legacy", true, "old"),
                issue(3, "Renamed managed issue", false, &identity),
            ],
            &identity,
            Some("Legacy"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(selected.issue.number, 3);
        assert!(!selected.legacy);
    }

    #[test]
    fn title_ambiguity_is_an_error() {
        let error = select_issue(
            vec![
                issue(1, "Legacy", true, "one"),
                issue(2, "Legacy", true, "two"),
            ],
            &identity("current", IssueKind::Regression),
            Some("Legacy"),
        )
        .unwrap_err();
        assert!(error.find_source::<AmbiguousLegacyIssue>().is_some());
    }

    #[test]
    fn fallback_cannot_adopt_other_instances_or_issue_kinds() {
        for managed in [
            identity("other", IssueKind::Regression),
            identity("current", IssueKind::FailureAlert),
        ] {
            let error = select_issue(
                vec![issue(1, "Legacy", true, &managed)],
                &identity("current", IssueKind::Regression),
                Some("Legacy"),
            )
            .unwrap_err();
            assert!(error.find_source::<AlreadyManagedLegacyIssue>().is_some());
        }
    }
}
