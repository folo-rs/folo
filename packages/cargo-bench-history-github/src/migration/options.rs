use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::AppError;

use crate::marker::CommentMarker;

/// Explicit one-time adoption exceptions; ordinary lifecycle behavior needs neither option.
#[derive(Clone, Debug, Default)]
pub(crate) struct MigrationOptions {
    pub(crate) issue_title: Option<String>,
    pub(crate) in_progress_marker: Option<CommentMarker>,
}

impl MigrationOptions {
    pub(crate) fn is_legacy_placeholder(&self, body: &str) -> bool {
        self.in_progress_marker
            .as_ref()
            .is_some_and(|marker| body.lines().any(|line| line == marker.as_str()))
    }
}

pub(crate) fn validate_legacy_title(title: &str) -> Result<(), AppError> {
    if title.trim().is_empty() || title.chars().any(char::is_control) {
        return Err(InvalidLegacyIssueTitle::new().into());
    }
    Ok(())
}

/// Title adoption has meaning only for the issue lifecycles, never for PRs or evidence helpers.
#[ohno::error]
#[display(
    "--legacy-issue-title applies only to issue-preflight, publish-issue, issue-cleanup, alert and resolve-alert"
)]
pub(crate) struct UnexpectedLegacyIssueTitle;

/// An explicit title must identify a nonempty, single-line GitHub issue title.
#[ohno::error]
#[display("Legacy issue title must be nonempty and contain no control characters")]
pub(crate) struct InvalidLegacyIssueTitle;

impl UnwindSafe for UnexpectedLegacyIssueTitle {}
impl RefUnwindSafe for UnexpectedLegacyIssueTitle {}
impl UnwindSafe for InvalidLegacyIssueTitle {}
impl RefUnwindSafe for InvalidLegacyIssueTitle {}
