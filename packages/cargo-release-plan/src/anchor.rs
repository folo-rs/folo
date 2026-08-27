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
    /// Parsed version at this commit, or `None` if the package was absent.
    pub(crate) version: Option<Version>,
    /// Whether this commit has a parent, so reaching it does not prove a root.
    ///
    /// A shallow-boundary commit sets this even though its parent is not
    /// fetched, which is what distinguishes truncated history from a true root.
    pub(crate) has_parent: bool,
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
    let Some(first) = timeline.first() else {
        return Err(ShallowHistoryError::new(package).into());
    };
    let Some(mut prev_version) = first.version.clone() else {
        // Absent on the base revision: not an anchor walk. Callers treat this as
        // a new package (version increased from absent).
        return Err(ShallowHistoryError::new(package).into());
    };
    let mut prev_commit = first.commit.clone();

    for entry in timeline.iter().skip(1) {
        if entry.version.as_ref() != Some(&prev_version) {
            return Ok(Anchor {
                commit: prev_commit,
                version: prev_version,
            });
        }
        prev_commit.clone_from(&entry.commit);
        if let Some(version) = &entry.version {
            prev_version.clone_from(version);
        }
    }

    let last = timeline.last().unwrap_or(first);
    if last.has_parent {
        return Err(ShallowHistoryError::new(package).into());
    }
    Ok(Anchor {
        commit: prev_commit,
        version: prev_version,
    })
}

/// Finds the newest commit at which a package absent from the base still existed.
///
/// A branch that restores a package Git no longer carries on the base line is
/// not creating it: the version the restored manifest declares may already be
/// published, so the last version the base line carried remains the anchor and
/// the ordinary monotonicity and content checks still apply. Ref:
/// docs/design.md, "Classification".
///
/// Returns `None` when no sampled commit carried the package, which is the
/// genuine creation case.
pub(crate) fn reintroduction_anchor(timeline: &[TimelineEntry]) -> Option<Anchor> {
    timeline.iter().find_map(|entry| {
        entry.version.as_ref().map(|version| Anchor {
            commit: entry.commit.clone(),
            version: version.clone(),
        })
    })
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
            version: version.map(v),
            has_parent,
        }
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
    fn merge_commit_on_first_parent_line_is_visible() {
        // First-parent walk sees the merge (m) then mainline (c0), never the
        // merged branch's intermediate commits.
        let timeline = vec![
            entry("m", Some("0.1.1"), true),
            entry("c0", Some("0.1.0"), false),
        ];
        let anchor = resolve_anchor("foo", &timeline).unwrap();
        assert_eq!(anchor.commit, "m");
        assert_eq!(anchor.version, v("0.1.1"));
    }

    #[test]
    fn reintroduction_anchor_finds_the_newest_commit_that_carried_the_package() {
        let timeline = vec![
            entry("c4", None, true),
            entry("c3", None, true),
            entry("c2", Some("0.3.0"), true),
            entry("c1", Some("0.2.0"), false),
        ];
        let anchor = reintroduction_anchor(&timeline).unwrap();
        assert_eq!(anchor.commit, "c2");
        assert_eq!(anchor.version, v("0.3.0"));
    }

    #[test]
    fn reintroduction_anchor_is_absent_for_a_genuinely_new_package() {
        let timeline = vec![entry("c2", None, true), entry("c1", None, false)];
        assert!(reintroduction_anchor(&timeline).is_none());
    }
}
