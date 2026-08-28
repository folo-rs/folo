// Anchor resolution.
//
// The anchor is the most recent commit on the base first-parent line in which
// the package's parsed `version` changed (including creation: absent → present).
// A walk that exhausts available history without observing a version change is
// an error, not a pass.

use ohno::AppError;
use semver::Version;

use crate::ShallowHistoryError;

/// The commit that last changed a package's declared version on the base line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Anchor {
    pub(crate) commit: String,
    pub(crate) version: Version,
}

/// One first-parent commit in a synthetic or observed timeline, newest first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TimelineEntry {
    pub(crate) commit: String,
    /// What this commit's workspace says about the package.
    pub(crate) presence: Presence,
    /// Whether this commit has a parent, so reaching it does not prove a root.
    ///
    /// A shallow-boundary commit sets this even though its parent is not
    /// fetched, which is what distinguishes truncated history from a true root.
    pub(crate) has_parent: bool,
}

/// What one commit's workspace says about a package.
///
/// The anchor is the last state a consumer could have received, so the three
/// cases are not interchangeable. An absent package makes its reappearance a
/// version change, while a package that is present but not publishable released
/// nothing and so neither anchors a version nor interrupts an older one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Presence {
    /// The commit's workspace does not carry the package.
    Absent,
    /// The package is a member but declares `publish = false`.
    Unpublished,
    /// The package is a publishable member declaring this version.
    Published(Version),
}

impl Presence {
    /// The version a consumer could have received at this commit.
    pub(crate) fn released_version(&self) -> Option<&Version> {
        match self {
            Self::Published(version) => Some(version),
            Self::Absent | Self::Unpublished => None,
        }
    }

    /// Whether this commit is invisible to the anchor walk.
    pub(crate) fn is_unpublished(&self) -> bool {
        matches!(self, Self::Unpublished)
    }
}

/// Resolves the version-change anchor from a newest-first first-parent timeline.
///
/// The first entry is the base revision. Walking toward the root, the first time
/// the version differs from the previous (newer) commit, that newer commit is
/// the anchor. Reaching a root commit (no parent) treats creation as the change.
pub(crate) fn resolve_anchor(
    package: &str,
    timeline: &[TimelineEntry],
) -> Result<Anchor, AppError> {
    // A commit at which the package was not publishable released nothing, so it
    // is skipped rather than read as an absence: treating it as absent would
    // make the next publishable commit look like a creation and hide everything
    // released before the package was withdrawn.
    let observable: Vec<&TimelineEntry> = timeline
        .iter()
        .filter(|entry| !entry.presence.is_unpublished())
        .collect();

    let Some(first) = observable.first() else {
        return Err(ShallowHistoryError::new(package).into());
    };
    let Some(mut prev_version) = first.presence.released_version() else {
        // Absent on the base revision: not an anchor walk. Callers treat this as
        // a new package (version increased from absent).
        return Err(ShallowHistoryError::new(package).into());
    };
    // The walk carries borrowed candidates, because only the one it stops on is
    // retained and the timeline outlives the search.
    let mut prev_commit = first.commit.as_str();

    for entry in observable.iter().skip(1) {
        if entry.presence.released_version() != Some(prev_version) {
            return Ok(Anchor {
                commit: prev_commit.to_string(),
                version: prev_version.clone(),
            });
        }
        prev_commit = entry.commit.as_str();
        if let Some(version) = entry.presence.released_version() {
            prev_version = version;
        }
    }

    // Whether history ran out is a property of the walk over real commits, so it
    // is read from the oldest commit rather than the oldest observable one.
    let last = timeline.last().unwrap_or(first);
    if last.has_parent {
        return Err(ShallowHistoryError::new(package).into());
    }
    Ok(Anchor {
        commit: prev_commit.to_string(),
        version: prev_version.clone(),
    })
}

/// Resolves the anchor of a package the base revision no longer carries.
///
/// A branch that restores a package Git no longer carries on the base line is
/// not creating it: the version the restored manifest declares may already be
/// published, so the base line's own anchor still governs and the ordinary
/// monotonicity and content checks still apply. Ref: docs/design.md,
/// "Classification".
///
/// The walk resumes at the newest commit that carried the package and then
/// follows the ordinary anchor rule, because that commit is not necessarily the
/// one that last changed the version: content committed without an increment
/// before the deletion must not be absorbed into the anchor and thereby become
/// invisible when the package comes back at the same version.
///
/// Returns `Ok(None)` when the walk reached a true root without the package ever
/// appearing, which is the genuine creation case. Exhausting a truncated history
/// instead proves nothing about whether the package once existed, so that is an
/// error.
pub(crate) fn reintroduction_anchor(
    package: &str,
    timeline: &[TimelineEntry],
) -> Result<Option<Anchor>, AppError> {
    let Some(present) = timeline
        .iter()
        .position(|entry| entry.presence.released_version().is_some())
    else {
        // Never publishable anywhere in the sampled history, so nothing was ever
        // released under this name. Only a walk that ran out of history at a
        // root can rule out an earlier, already-published incarnation.
        return match timeline.last() {
            Some(last) if !last.has_parent => Ok(None),
            _ => Err(ShallowHistoryError::new(package).into()),
        };
    };
    let remainder = timeline
        .get(present..)
        .expect("`position` returned an in-bounds index into this same slice");
    resolve_anchor(package, remainder).map(Some)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        text.parse().unwrap()
    }

    fn entry(commit: &str, version: Option<&str>, has_parent: bool) -> TimelineEntry {
        TimelineEntry {
            commit: commit.to_string(),
            presence: version.map_or(Presence::Absent, |text| Presence::Published(v(text))),
            has_parent,
        }
    }

    fn unpublished(commit: &str, has_parent: bool) -> TimelineEntry {
        TimelineEntry {
            commit: commit.to_string(),
            presence: Presence::Unpublished,
            has_parent,
        }
    }

    #[test]
    fn an_empty_timeline_is_truncated_history() {
        let error = resolve_anchor("foo", &[]).unwrap_err();
        assert!(error.find_source::<ShallowHistoryError>().is_some());
    }

    #[test]
    fn a_base_revision_without_the_package_is_not_an_anchor_walk() {
        let timeline = vec![entry("c1", None, true), entry("c0", None, false)];
        let error = resolve_anchor("foo", &timeline).unwrap_err();
        assert!(error.find_source::<ShallowHistoryError>().is_some());
    }

    #[test]
    fn most_recent_version_change_is_the_anchor() {
        let timeline = vec![
            entry("c3", Some("0.1.1"), true),
            entry("c2", Some("0.1.1"), true),
            entry("c1", Some("0.1.0"), true),
            entry("c0", Some("0.1.0"), false),
        ];
        let anchor = resolve_anchor("foo", &timeline).unwrap();
        assert_eq!(anchor.commit, "c2");
        assert_eq!(anchor.version, v("0.1.1"));
    }

    #[test]
    fn creation_commit_counts_as_a_version_change() {
        let timeline = vec![
            entry("c2", Some("0.1.0"), true),
            entry("c1", None, true),
            entry("c0", None, false),
        ];
        let anchor = resolve_anchor("foo", &timeline).unwrap();
        assert_eq!(anchor.commit, "c2");
        assert_eq!(anchor.version, v("0.1.0"));
    }

    #[test]
    fn root_commit_is_the_anchor_when_version_never_changed() {
        let timeline = vec![
            entry("c1", Some("0.1.0"), true),
            entry("c0", Some("0.1.0"), false),
        ];
        let anchor = resolve_anchor("foo", &timeline).unwrap();
        assert_eq!(anchor.commit, "c0");
    }

    #[test]
    fn truncated_history_without_a_change_is_an_error() {
        let timeline = vec![
            entry("c1", Some("0.1.0"), true),
            entry("c0", Some("0.1.0"), true),
        ];
        let error = resolve_anchor("foo", &timeline).unwrap_err();
        assert!(error.find_source::<ShallowHistoryError>().is_some());
    }

    #[test]
    fn a_withdrawn_period_does_not_hide_the_older_release() {
        // Withdrawing a package and later restoring it at the same version does
        // not make that version free: the earlier release still governs, so the
        // anchor must reach past the withdrawal instead of stopping at the
        // commit that restored the package.
        let timeline = vec![
            entry("c2", Some("0.1.0"), true),
            unpublished("c1", true),
            entry("c0", Some("0.1.0"), false),
        ];
        let anchor = resolve_anchor("foo", &timeline).unwrap();
        assert_eq!(anchor.commit, "c0");
        assert_eq!(anchor.version, v("0.1.0"));
    }

    #[test]
    fn a_withdrawn_period_does_not_move_a_newer_anchor() {
        let timeline = vec![
            entry("c2", Some("0.2.0"), true),
            unpublished("c1", true),
            entry("c0", Some("0.1.0"), false),
        ];
        let anchor = resolve_anchor("foo", &timeline).unwrap();
        assert_eq!(anchor.commit, "c2");
        assert_eq!(anchor.version, v("0.2.0"));
    }

    #[test]
    fn history_ending_in_a_withdrawn_period_still_resolves() {
        // The oldest commit carries no release, so the walk stops on the oldest
        // published commit while the root proves history was not truncated.
        let timeline = vec![entry("c1", Some("0.1.0"), true), unpublished("c0", false)];
        let anchor = resolve_anchor("foo", &timeline).unwrap();
        assert_eq!(anchor.commit, "c1");
    }

    #[test]
    fn truncated_history_ending_in_a_withdrawn_period_is_an_error() {
        let timeline = vec![entry("c1", Some("0.1.0"), true), unpublished("c0", true)];
        let error = resolve_anchor("foo", &timeline).unwrap_err();
        assert!(error.find_source::<ShallowHistoryError>().is_some());
    }

    #[test]
    fn merge_commit_on_first_parent_line_is_visible() {
        // A first-parent walk sees the merge commit and then its mainline
        // parent, so an increment made on a merged topic branch is observed at
        // the merge rather than at the commit that wrote it.
        let timeline = vec![
            entry("m", Some("0.1.1"), true),
            entry("c0", Some("0.1.0"), false),
        ];
        let anchor = resolve_anchor("foo", &timeline).unwrap();
        assert_eq!(anchor.commit, "m");
        assert_eq!(anchor.version, v("0.1.1"));
    }

    #[test]
    fn reintroduction_anchor_walks_on_to_the_last_version_change() {
        // The newest commit that carried the package changed content without
        // incrementing, so the anchor is the older commit where that version
        // first appeared. Anchoring on the newest carrier instead would hide
        // the unreleased change that preceded the deletion.
        let timeline = vec![
            entry("c4", None, true),
            entry("c3", None, true),
            entry("c2", Some("0.3.0"), true),
            entry("c1", Some("0.3.0"), true),
            entry("c0", Some("0.2.0"), false),
        ];
        let anchor = reintroduction_anchor("foo", &timeline).unwrap().unwrap();
        assert_eq!(anchor.commit, "c1");
        assert_eq!(anchor.version, v("0.3.0"));
    }

    #[test]
    fn reintroduction_anchor_finds_the_newest_commit_that_carried_the_package() {
        let timeline = vec![
            entry("c4", None, true),
            entry("c3", None, true),
            entry("c2", Some("0.3.0"), true),
            entry("c1", Some("0.2.0"), false),
        ];
        let anchor = reintroduction_anchor("foo", &timeline).unwrap().unwrap();
        assert_eq!(anchor.commit, "c2");
        assert_eq!(anchor.version, v("0.3.0"));
    }

    #[test]
    fn reintroduction_anchor_is_absent_for_a_genuinely_new_package() {
        let timeline = vec![entry("c2", None, true), entry("c1", None, false)];
        assert!(reintroduction_anchor("foo", &timeline).unwrap().is_none());
    }

    #[test]
    fn a_package_that_was_never_publishable_is_new() {
        // The name exists in history but nothing was ever released under it, so
        // the branch is free to choose any version.
        let timeline = vec![unpublished("c2", true), unpublished("c1", false)];
        assert!(reintroduction_anchor("foo", &timeline).unwrap().is_none());
    }

    #[test]
    fn a_package_never_publishable_in_truncated_history_is_an_error() {
        let timeline = vec![unpublished("c2", true), unpublished("c1", true)];
        let error = reintroduction_anchor("foo", &timeline).unwrap_err();
        assert!(error.find_source::<ShallowHistoryError>().is_some());
    }

    #[test]
    fn a_package_absent_from_every_sampled_commit_is_truncated_history() {
        // No sampled commit carried the package, but the oldest still has a
        // parent, so an earlier already-published incarnation cannot be ruled
        // out and creation must not be assumed.
        let timeline = vec![entry("c2", None, true), entry("c1", None, true)];
        let error = reintroduction_anchor("foo", &timeline).unwrap_err();
        assert!(error.find_source::<ShallowHistoryError>().is_some());
    }

    #[test]
    fn reintroduction_anchor_rejects_an_empty_timeline() {
        let error = reintroduction_anchor("foo", &[]).unwrap_err();
        assert!(error.find_source::<ShallowHistoryError>().is_some());
    }

    #[test]
    fn reintroduction_anchor_rejects_truncated_history() {
        // Every sampled commit declares the same version and the oldest still
        // has a parent, so the version change that would anchor the package
        // lies beyond the sampled history and was never observed.
        let timeline = vec![
            entry("c3", None, true),
            entry("c2", Some("0.3.0"), true),
            entry("c1", Some("0.3.0"), true),
        ];
        let error = reintroduction_anchor("foo", &timeline).unwrap_err();
        assert!(error.find_source::<ShallowHistoryError>().is_some());
    }
}
