use std::panic::{RefUnwindSafe, UnwindSafe};
use std::str::FromStr;

use ohno::AppError;

use crate::model::{CommitSha, Instance, IssueKind};

/// An optional caller-selected exact marker for adopting an existing rolling PR comment.
#[derive(Clone, Debug)]
pub(crate) struct CommentMarker(String);

impl CommentMarker {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for CommentMarker {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some(inner) = value
            .strip_prefix("<!--")
            .and_then(|v| v.strip_suffix("-->"))
        else {
            return Err(InvalidCommentMarker::new().into());
        };
        if inner.trim().is_empty() || inner.contains("--") || inner.chars().any(char::is_control) {
            return Err(InvalidCommentMarker::new().into());
        }
        Ok(Self(value.to_owned()))
    }
}

/// A custom identity must be one complete, nonempty HTML comment line.
#[ohno::error]
#[display("Comment marker must be one nonempty HTML comment without nested delimiters or controls")]
struct InvalidCommentMarker;

impl UnwindSafe for InvalidCommentMarker {}
impl RefUnwindSafe for InvalidCommentMarker {}

/// Issue operations have no PR-comment identity to override.
#[ohno::error]
#[display("--comment-marker applies only to pull-request comment commands")]
pub(crate) struct UnexpectedCommentMarker;

impl UnwindSafe for UnexpectedCommentMarker {}
impl RefUnwindSafe for UnexpectedCommentMarker {}

pub(crate) fn issue(instance: &Instance, kind: IssueKind) -> String {
    format!(
        "<!-- cargo-bench-history:{}:issue:{} -->",
        instance.as_str(),
        kind.as_str()
    )
}

pub(crate) fn pr_comment(instance: &Instance) -> String {
    format!(
        "<!-- cargo-bench-history:{}:pr-comment -->",
        instance.as_str()
    )
}

pub(crate) fn analyzed_sha(instance: &Instance, sha: &CommitSha) -> String {
    format!(
        "<!-- cargo-bench-history:{}:analyzed-sha:{} -->",
        instance.as_str(),
        sha.as_str()
    )
}

pub(crate) fn in_progress(instance: &Instance) -> String {
    format!(
        "<!-- cargo-bench-history:{}:in-progress -->",
        instance.as_str()
    )
}

pub(crate) fn run_owner(instance: &Instance, run_id: u64, head: &CommitSha) -> String {
    format!(
        "<!-- cargo-bench-history:{}:run:{}:{} -->",
        instance.as_str(),
        run_id,
        head.as_str()
    )
}

pub(crate) fn empty_scope(instance: &Instance) -> String {
    format!(
        "<!-- cargo-bench-history:{}:empty-scope -->",
        instance.as_str()
    )
}

pub(crate) fn failed(instance: &Instance) -> String {
    format!("<!-- cargo-bench-history:{}:failed -->", instance.as_str())
}

pub(crate) fn stale_start(instance: &Instance) -> String {
    format!(
        "<!-- cargo-bench-history:{}:stale:start -->",
        instance.as_str()
    )
}

pub(crate) fn stale_end(instance: &Instance) -> String {
    format!(
        "<!-- cargo-bench-history:{}:stale:end -->",
        instance.as_str()
    )
}

pub(crate) fn find_analyzed_sha(body: &str, instance: &Instance) -> Option<CommitSha> {
    let prefix = format!(
        "<!-- cargo-bench-history:{}:analyzed-sha:",
        instance.as_str()
    );
    body.lines().find_map(|line| {
        let value = line.strip_prefix(&prefix)?.strip_suffix(" -->")?;
        value.parse().ok()
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn issue_kind_is_part_of_identity() {
        let instance: Instance = "default".parse().unwrap();
        assert_ne!(
            issue(&instance, IssueKind::Regression),
            issue(&instance, IssueKind::FailureAlert)
        );
    }

    #[test]
    fn analyzed_sha_round_trips_through_body() {
        let instance: Instance = "default".parse().unwrap();
        let sha: CommitSha = "0123456789abcdef0123456789abcdef01234567".parse().unwrap();
        let body = format!("{}\nbody", analyzed_sha(&instance, &sha));
        assert_eq!(find_analyzed_sha(&body, &instance), Some(sha));
    }

    #[test]
    fn every_pr_marker_is_namespaced_by_instance() {
        let instance: Instance = "nightly".parse().unwrap();
        assert_eq!(
            pr_comment(&instance),
            "<!-- cargo-bench-history:nightly:pr-comment -->"
        );
        assert_eq!(
            in_progress(&instance),
            "<!-- cargo-bench-history:nightly:in-progress -->"
        );
        assert_eq!(
            empty_scope(&instance),
            "<!-- cargo-bench-history:nightly:empty-scope -->"
        );
        assert_eq!(
            failed(&instance),
            "<!-- cargo-bench-history:nightly:failed -->"
        );
        assert_eq!(
            stale_start(&instance),
            "<!-- cargo-bench-history:nightly:stale:start -->"
        );
        assert_eq!(
            stale_end(&instance),
            "<!-- cargo-bench-history:nightly:stale:end -->"
        );
    }

    #[test]
    fn custom_marker_requires_an_exact_single_html_comment() {
        let marker = "<!-- team-performance -->"
            .parse::<CommentMarker>()
            .unwrap();
        assert_eq!(marker.as_str(), "<!-- team-performance -->");
        for invalid in ["", "text", "<!-- -->", "<!-- a\nb -->", "<!-- a--b -->"] {
            invalid.parse::<CommentMarker>().unwrap_err();
        }
    }
}
