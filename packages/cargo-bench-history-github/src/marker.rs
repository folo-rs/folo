use crate::cli::{PendingArgs, RunArgs};
use crate::model::{CommitSha, Instance, IssueKind};

/// Identifies an issue body after title-based discovery selects its repository-scoped candidate.
pub(crate) fn issue(instance: &Instance, kind: IssueKind) -> String {
    format!(
        "<!-- cargo-bench-history:{}:issue:{} -->",
        instance.as_str(),
        kind.as_str()
    )
}

/// Names the rolling comment that discovery selects within one pull request.
pub(crate) fn pr_comment(instance: &Instance) -> String {
    format!(
        "<!-- cargo-bench-history:{}:pr-comment -->",
        instance.as_str()
    )
}

/// Records the report's measured commit independently of later pending-work annotations.
pub(crate) fn analyzed_sha(instance: &Instance, sha: &CommitSha) -> String {
    format!(
        "<!-- cargo-bench-history:{}:analyzed-sha:{} -->",
        instance.as_str(),
        sha.as_str()
    )
}

/// Marks an unfinished PR placeholder that only its owning run may retire as failed.
pub(crate) fn in_progress(instance: &Instance) -> String {
    format!(
        "<!-- cargo-bench-history:{}:in-progress -->",
        instance.as_str()
    )
}

/// Records run, attempt and frozen head for lifecycle ownership checks.
pub(crate) fn run_owner(instance: &Instance, owner: &PendingArgs) -> String {
    format!(
        "<!-- cargo-bench-history:{}:run:{}:{}:{} -->",
        instance.as_str(),
        owner.run.run_id,
        owner.run.run_attempt,
        owner.head.as_str()
    )
}

/// Recovers one coherent ownership record without assigning chronology to distinct run IDs.
pub(crate) fn find_owner(body: &str, instance: &Instance) -> Option<PendingArgs> {
    let value = unique_value(body, instance, "run")?;
    let mut parts = value.split(':');
    let owner = PendingArgs {
        run: RunArgs {
            run_id: parts.next()?.parse().ok()?,
            run_attempt: parts.next()?.parse().ok()?,
        },
        head: parts.next()?.parse().ok()?,
    };
    parts.next().is_none().then_some(owner)
}

/// Carries the validated report or annotation state for subsequent lifecycle interpretation.
pub(crate) fn state(instance: &Instance, state: &str) -> String {
    format!(
        "<!-- cargo-bench-history:{}:state:{state} -->",
        instance.as_str()
    )
}

/// Reads a unique state value; callers decide whether that state is legal for the body kind.
pub(crate) fn find_state<'a>(body: &'a str, instance: &Instance) -> Option<&'a str> {
    unique_value(body, instance, "state")
}

/// Detects even malformed value-bearing metadata when note/report states must remain separate.
pub(crate) fn has_value(body: &str, instance: &Instance, key: &str) -> bool {
    let prefix = value_prefix(instance, key);
    body.lines().any(|line| line.starts_with(&prefix))
}

/// Opens the owned status block that issue updates may replace without rewriting the report.
pub(crate) fn annotation_start(instance: &Instance) -> String {
    format!(
        "<!-- cargo-bench-history:{}:annotation:start -->",
        instance.as_str()
    )
}

/// Closes the owned issue annotation so lifecycle parsing can preserve the report separately.
pub(crate) fn annotation_end(instance: &Instance) -> String {
    format!(
        "<!-- cargo-bench-history:{}:annotation:end -->",
        instance.as_str()
    )
}

/// Binds a one-off alert body to the workflow run identified by its title.
pub(crate) fn alert_run(instance: &Instance, run_id: u64) -> String {
    format!(
        "<!-- cargo-bench-history:{}:alert-run:{run_id} -->",
        instance.as_str()
    )
}

/// Identifies an explicit no-benchmarkable-packages PR note, not a missing report.
pub(crate) fn empty_scope(instance: &Instance) -> String {
    format!(
        "<!-- cargo-bench-history:{}:empty-scope -->",
        instance.as_str()
    )
}

/// Identifies a terminal PR execution notice that later preflight may replace.
pub(crate) fn failed(instance: &Instance) -> String {
    format!("<!-- cargo-bench-history:{}:failed -->", instance.as_str())
}

/// Opens the replaceable warning block without changing the report's ownership metadata.
pub(crate) fn stale_start(instance: &Instance) -> String {
    format!(
        "<!-- cargo-bench-history:{}:stale:start -->",
        instance.as_str()
    )
}

/// Closes the warning block used by staleness-banner replacement.
pub(crate) fn stale_end(instance: &Instance) -> String {
    format!(
        "<!-- cargo-bench-history:{}:stale:end -->",
        instance.as_str()
    )
}

/// Reads the uniquely recorded measured commit used by freshness guards.
pub(crate) fn find_analyzed_sha(body: &str, instance: &Instance) -> Option<CommitSha> {
    unique_value(body, instance, "analyzed-sha")?.parse().ok()
}

/// Treats malformed or repeated metadata as unusable rather than choosing an arbitrary value.
fn unique_value<'a>(body: &'a str, instance: &Instance, key: &str) -> Option<&'a str> {
    let prefix = value_prefix(instance, key);
    let mut values = body.lines().filter_map(|line| line.strip_prefix(&prefix));
    let value = values.next()?.strip_suffix(" -->")?;
    values.next().is_none().then_some(value)
}

/// Keeps metadata readers and writers on the same project-qualified marker namespace.
fn value_prefix(instance: &Instance, key: &str) -> String {
    format!("<!-- cargo-bench-history:{}:{key}:", instance.as_str())
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
    fn alert_identity_distinguishes_projects_and_runs() {
        let project: Instance = "project".parse().unwrap();
        let other: Instance = "project.extra".parse().unwrap();
        assert_ne!(alert_run(&project, 42), alert_run(&project, 43));
        assert_ne!(alert_run(&project, 42), alert_run(&other, 42));
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
}
