//! Pure parsing of `git` command output into [`GitInfo`], so the git probe's
//! logic is unit-testable without a real repository.

use cbh_model::GitInfo;

/// Builds the recorded git facts from raw git command outputs (pure).
///
/// * `commit` — output of `git rev-parse HEAD`.
/// * `branch` — output of `git rev-parse --abbrev-ref HEAD`.
/// * `status` — output of `git status --porcelain` (non-empty ⇒ dirty).
#[must_use]
pub fn parse_git_info(commit: &str, branch: &str, status: &str) -> GitInfo {
    GitInfo {
        commit: non_empty(commit),
        branch: non_empty(branch).filter(|branch| branch != "HEAD"),
        dirty: !status.trim().is_empty(),
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
    fn builds_clean_info() {
        let info = parse_git_info("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\n", "main\n", "");

        assert_eq!(
            info.commit.as_deref(),
            Some("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
        );
        assert_eq!(info.branch.as_deref(), Some("main"));
        assert!(!info.dirty);
    }

    #[test]
    fn detects_dirty_tree() {
        let info = parse_git_info("c", "main", " M src/lib.rs\n");
        assert!(info.dirty);
    }

    #[test]
    fn detached_head_branch_is_dropped() {
        let info = parse_git_info("c", "HEAD\n", "");
        assert_eq!(info.branch, None);
    }

    #[test]
    fn empty_outputs_yield_defaults() {
        let info = parse_git_info("", "", "");
        assert_eq!(info, GitInfo::default());
    }
}
