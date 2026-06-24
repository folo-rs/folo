//! Pure parsing of `git` command output into [`GitSnapshot`], so the git probe's
//! logic is unit-testable without a real repository.

use jiff::Timestamp;

use crate::model::GitInfo;

/// A git snapshot: the recorded [`GitInfo`] plus the committer date used as the
/// commit timestamp.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GitSnapshot {
    /// Commit, branch and dirty-state facts recorded with the run.
    pub(crate) info: GitInfo,
    /// The committer date of `HEAD`, if it could be parsed.
    pub(crate) committer: Option<Timestamp>,
}

/// Builds a snapshot from raw git command outputs (pure).
///
/// * `commit` — output of `git rev-parse HEAD`.
/// * `short` — output of `git rev-parse --short HEAD`.
/// * `branch` — output of `git rev-parse --abbrev-ref HEAD`.
/// * `status` — output of `git status --porcelain` (non-empty ⇒ dirty).
/// * `committer_iso` — output of `git show -s --format=%cI HEAD` (strict ISO 8601).
pub(crate) fn build_snapshot(
    commit: &str,
    short: &str,
    branch: &str,
    status: &str,
    committer_iso: &str,
) -> GitSnapshot {
    let info = GitInfo {
        commit: non_empty(commit),
        short_commit: non_empty(short),
        branch: non_empty(branch).filter(|branch| branch != "HEAD"),
        dirty: !status.trim().is_empty(),
    };
    GitSnapshot {
        info,
        committer: committer_iso.trim().parse::<Timestamp>().ok(),
    }
}

/// Returns `Some(owned)` for a non-empty trimmed string, `None` otherwise.
fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn builds_clean_snapshot() {
        let snapshot = build_snapshot(
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\n",
            "deadbee\n",
            "main\n",
            "",
            "2025-06-14T20:33:00+02:00\n",
        );

        assert_eq!(
            snapshot.info.commit.as_deref(),
            Some("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
        );
        assert_eq!(snapshot.info.short_commit.as_deref(), Some("deadbee"));
        assert_eq!(snapshot.info.branch.as_deref(), Some("main"));
        assert!(!snapshot.info.dirty);
        let expected: Timestamp = "2025-06-14T18:33:00Z".parse().unwrap();
        assert_eq!(snapshot.committer, Some(expected));
    }

    #[test]
    fn detects_dirty_tree() {
        let snapshot = build_snapshot("c", "c", "main", " M src/lib.rs\n", "");
        assert!(snapshot.info.dirty);
    }

    #[test]
    fn detached_head_branch_is_dropped() {
        let snapshot = build_snapshot("c", "c", "HEAD\n", "", "");
        assert_eq!(snapshot.info.branch, None);
    }

    #[test]
    fn empty_outputs_yield_defaults() {
        let snapshot = build_snapshot("", "", "", "", "");
        assert_eq!(snapshot, GitSnapshot::default());
        assert_eq!(snapshot.committer, None);
    }

    #[test]
    fn unparsable_committer_date_is_none() {
        let snapshot = build_snapshot("c", "c", "main", "", "not-a-date");
        assert_eq!(snapshot.committer, None);
    }
}
