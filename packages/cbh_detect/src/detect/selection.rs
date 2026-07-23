//! The query model: resolving *which* commits make up a series and in what order.
//!
//! `analyze` reconstructs a timeline from git topology rather than from stored
//! timestamps (see the `analyze` command in `DESIGN.md`). Given the target ref's
//! first-parent ancestry and
//! its merge-base with the base ref, this pure logic decides, for each commit in
//! topological order, whether it is *base-side* (only clean runs count) or
//! *target-side* (clean and dirty runs count). The async git calls live in
//! [`detect`](super); keeping the split here pure keeps it Miri-testable.

/// How a selected commit treats dirty (uncommitted-tree) snapshots.
///
/// Replacing a pair of booleans, this makes the base-branch exception's invariant
/// structural: the exception is a *kind* of admission, so a commit can never
/// claim the exception while excluding dirty runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirtyAdmission {
    /// Dirty snapshots are excluded: a base-side commit, or `--no-dirty`.
    Excluded,
    /// Dirty snapshots are admitted for a target-side commit.
    Admitted,
    /// Dirty snapshots are admitted *only* by the base-branch dirty-tree exception:
    /// a base-side tip whose dirty runs are the user's current in-flight work. When
    /// such a run is actually included, `analyze` warns that it is ephemeral (see
    /// the `analyze` command in `DESIGN.md`).
    BaseException,
}

impl DirtyAdmission {
    /// Whether dirty snapshots on the commit are admitted at all.
    #[must_use]
    pub fn admits_dirty(self) -> bool {
        matches!(self, Self::Admitted | Self::BaseException)
    }

    /// Whether admission is granted *only* by the base-branch dirty-tree exception,
    /// the case that triggers the ephemeral-data warning.
    #[must_use]
    pub fn is_base_exception(self) -> bool {
        matches!(self, Self::BaseException)
    }
}

/// One commit selected for analysis, in oldest-first topological position.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedCommit {
    /// The commit ID (the storage commit-directory segment).
    pub commit: String,
    /// How this commit treats dirty (uncommitted-tree) snapshots.
    pub dirty: DirtyAdmission,
}

/// Splits the target's first-parent `ancestry` (oldest-first) at the `merge_base`
/// with the base ref.
///
/// Commits at or before the merge-base are *base-side* and contribute only clean
/// runs; commits after it are *target-side* and additionally contribute dirty
/// snapshots when `allow_dirty` is set. When the merge-base is absent or is not on
/// the target's first-parent chain (a degenerate or merge-laden history), every
/// commit is treated as target-side — the inclusive choice for a "how does my
/// branch fit in" view, and harmless for an official view (whose base *is* the
/// target, so the merge-base is always the tip and on the chain).
///
/// `dirty_tip_exception` carves out the on-the-base-branch scenario: when the
/// **tip** commit is base-side (an official view) but the working tree is
/// currently dirty, its dirty snapshots are the user's in-flight work, so they are
/// admitted (flagged via [`DirtyAdmission::BaseException`] so the caller can
/// warn). Earlier base-side commits are unaffected — only the tip. `allow_dirty`
/// still gates it, so `--no-dirty` overrides the exception.
#[must_use]
pub fn select_commits(
    ancestry: &[String],
    merge_base: Option<&str>,
    allow_dirty: bool,
    dirty_tip_exception: bool,
) -> Vec<SelectedCommit> {
    let split = merge_base.and_then(|base| ancestry.iter().position(|commit| commit == base));
    let tip_index = ancestry.len().checked_sub(1);
    ancestry
        .iter()
        .enumerate()
        .map(|(index, commit)| {
            let target_side = split.is_none_or(|boundary| index > boundary);
            let is_tip = tip_index == Some(index);
            let dirty = if !target_side && is_tip && allow_dirty && dirty_tip_exception {
                DirtyAdmission::BaseException
            } else if target_side && allow_dirty {
                DirtyAdmission::Admitted
            } else {
                DirtyAdmission::Excluded
            };
            SelectedCommit {
                commit: commit.clone(),
                dirty,
            }
        })
        .collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn commit_ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn admit(selection: &[SelectedCommit]) -> Vec<(&str, bool)> {
        selection
            .iter()
            .map(|selected| (selected.commit.as_str(), selected.dirty.admits_dirty()))
            .collect()
    }

    #[test]
    fn official_view_admits_no_dirty_when_base_is_the_tip() {
        // Analyzing master with base==master: merge-base is the tip, so every
        // commit is base-side (clean only).
        let ancestry = commit_ids(&["c0", "c1", "c2", "c3"]);
        let selection = select_commits(&ancestry, Some("c3"), true, false);
        assert_eq!(
            admit(&selection),
            vec![("c0", false), ("c1", false), ("c2", false), ("c3", false)]
        );
        assert!(
            selection
                .iter()
                .all(|selected| !selected.dirty.is_base_exception())
        );
    }

    #[test]
    fn feature_view_admits_dirty_after_the_merge_base() {
        // feature ancestry [c0,c1,f1,f2] branched at c1: c0,c1 base-side; f1,f2
        // target-side (admit dirty).
        let ancestry = commit_ids(&["c0", "c1", "f1", "f2"]);
        let selection = select_commits(&ancestry, Some("c1"), true, false);
        assert_eq!(
            admit(&selection),
            vec![("c0", false), ("c1", false), ("f1", true), ("f2", true)]
        );
    }

    #[test]
    fn no_dirty_suppresses_target_side_admission() {
        let ancestry = commit_ids(&["c0", "c1", "f1", "f2"]);
        let selection = select_commits(&ancestry, Some("c1"), false, false);
        assert!(
            selection
                .iter()
                .all(|selected| !selected.dirty.admits_dirty())
        );
    }

    #[test]
    fn absent_merge_base_treats_everything_as_target_side() {
        let ancestry = commit_ids(&["a0", "a1"]);
        let selection = select_commits(&ancestry, None, true, false);
        assert_eq!(admit(&selection), vec![("a0", true), ("a1", true)]);
    }

    #[test]
    fn merge_base_off_the_first_parent_chain_is_target_side() {
        // A merge-base that is not present in the ancestry list falls back to the
        // inclusive (target-side) treatment.
        let ancestry = commit_ids(&["c0", "c1", "c2"]);
        let selection = select_commits(&ancestry, Some("zz"), true, false);
        assert!(
            selection
                .iter()
                .all(|selected| selected.dirty.admits_dirty())
        );
    }

    #[test]
    fn ordering_is_preserved_oldest_first() {
        let ancestry = commit_ids(&["c0", "c1", "c2"]);
        let selection = select_commits(&ancestry, Some("c0"), true, false);
        let order: Vec<&str> = selection
            .iter()
            .map(|selected| selected.commit.as_str())
            .collect();
        assert_eq!(order, vec!["c0", "c1", "c2"]);
    }

    #[test]
    fn dirty_tip_exception_admits_dirty_on_the_base_side_tip_only() {
        // Official view (everything base-side) but the working tree is dirty: only
        // the tip (c3) admits dirty runs, flagged as the base-branch exception;
        // earlier base-side commits stay clean-only.
        let ancestry = commit_ids(&["c0", "c1", "c2", "c3"]);
        let selection = select_commits(&ancestry, Some("c3"), true, true);
        assert_eq!(
            admit(&selection),
            vec![("c0", false), ("c1", false), ("c2", false), ("c3", true)]
        );
        let flagged: Vec<&str> = selection
            .iter()
            .filter(|selected| selected.dirty.is_base_exception())
            .map(|selected| selected.commit.as_str())
            .collect();
        assert_eq!(flagged, vec!["c3"], "only the tip carries the exception");
    }

    #[test]
    fn dirty_tip_exception_is_gated_by_allow_dirty() {
        // --no-dirty (allow_dirty == false) overrides the dirty-tree exception.
        let ancestry = commit_ids(&["c0", "c1", "c2", "c3"]);
        let selection = select_commits(&ancestry, Some("c3"), false, true);
        assert!(
            selection
                .iter()
                .all(|selected| !selected.dirty.admits_dirty())
        );
        assert!(
            selection
                .iter()
                .all(|selected| !selected.dirty.is_base_exception())
        );
    }

    #[test]
    fn dirty_tip_exception_does_not_flag_a_target_side_tip() {
        // On a feature view the tip is already target-side, so the exception adds
        // nothing and must not mark the tip as a base-branch exception.
        let ancestry = commit_ids(&["c0", "c1", "f1", "f2"]);
        let selection = select_commits(&ancestry, Some("c1"), true, true);
        assert!(
            selection
                .iter()
                .all(|selected| !selected.dirty.is_base_exception()),
            "a target-side tip is admitted normally, not via the exception"
        );
    }
}
