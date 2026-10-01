//! Resolves actual release history and an optional final snapshot anticipated to squash onto it.

use crp_diag::{Quotable as _, Verbose};
use crp_workspace::git::{DefaultReleaseHistory, GitRepo};
use ohno::AppError;

/// Immutable assessment boundaries plus the caller's refs, retained to detect ref movement.
#[derive(Debug)]
pub(crate) struct AssessmentHistory {
    pub(crate) release_history: String,
    pub(crate) release_history_revision: String,
    pub(crate) merge_target: Option<String>,
    pub(crate) merge_target_revision: Option<String>,
}

impl AssessmentHistory {
    pub(crate) fn resolve(
        git: &GitRepo,
        release_history: Option<&str>,
        merge_target: Option<&str>,
        verbose: Verbose<'_>,
    ) -> Result<Self, AppError> {
        let release_history_revision = match release_history {
            Some(revision) => revision.to_owned(),
            None => {
                let default = git.default_release_history()?;
                verbose.note(|| match &default {
                    DefaultReleaseHistory::RemoteHead(revision) => format!(
                        "no --release-history given; the remote default {} supplies committed release history",
                        revision.quoted()
                    ),
                    DefaultReleaseHistory::Convention(revision) => format!(
                        "no --release-history or recorded remote default; {} supplies committed release history",
                        revision.quoted()
                    ),
                });
                default.revision().to_owned()
            }
        };
        let release_history = git.rev_parse(&format!("{release_history_revision}^{{commit}}"))?;
        let merge_target_revision = merge_target.map(str::to_owned);
        let merge_target = resolve_merge_target(git, &release_history, merge_target)?;
        Ok(Self {
            release_history,
            release_history_revision,
            merge_target,
            merge_target_revision,
        })
    }

    pub(crate) fn effective_target(&self) -> Option<&str> {
        self.merge_target.as_deref()
    }

    // Acquires native Git observations; verify_with owns ref reuse and movement decisions.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn verify(&self, git: &GitRepo) -> Result<(), AppError> {
        self.verify_with(
            |revision| git.rev_parse(revision),
            |ancestor, descendant| git.is_ancestor(ancestor, descendant),
        )
    }

    fn verify_with(
        &self,
        mut resolve: impl FnMut(&str) -> Result<String, AppError>,
        is_ancestor: impl FnMut(&str, &str) -> Result<bool, AppError>,
    ) -> Result<(), AppError> {
        let release_history = resolve(&format!("{}^{{commit}}", self.release_history_revision))?;
        let merge_target = resolve_merge_target_with(
            &release_history,
            self.merge_target_revision.as_deref(),
            resolve,
            is_ancestor,
        )?;
        if release_history != self.release_history || merge_target != self.merge_target {
            return Err(AssessmentHistoryMoved::new().into());
        }
        Ok(())
    }
}

/// Resolves a target and removes boundaries already represented by actual release history.
///
/// `release_history` is an already resolved commit. The optional target is resolved as a commit,
/// then retained only when it is a distinct descendant. An equal or ancestral target returns
/// `None`; divergence requires refreshing/rebasing rather than synthesizing merged history.
/// This observes the local repository without fetching or acquiring a Cargo workspace.
// Native ref and ancestry acquisition; the shared decision protocol is tested in process.
#[cfg_attr(test, mutants::skip)]
pub fn resolve_merge_target(
    git: &GitRepo,
    release_history: &str,
    merge_target: Option<&str>,
) -> Result<Option<String>, AppError> {
    resolve_merge_target_with(
        release_history,
        merge_target,
        |revision| git.rev_parse(revision),
        |ancestor, descendant| git.is_ancestor(ancestor, descendant),
    )
}

fn resolve_merge_target_with(
    release_history: &str,
    merge_target: Option<&str>,
    resolve: impl FnOnce(&str) -> Result<String, AppError>,
    mut is_ancestor: impl FnMut(&str, &str) -> Result<bool, AppError>,
) -> Result<Option<String>, AppError> {
    let Some(revision) = merge_target else {
        return Ok(None);
    };
    let target = resolve(&format!("{revision}^{{commit}}"))?;
    if target == release_history || is_ancestor(&target, release_history)? {
        return Ok(None);
    }
    if !is_ancestor(release_history, &target)? {
        return Err(UnrelatedMergeTarget::new(release_history, target).into());
    }
    Ok(Some(target))
}

/// A projected predecessor must descend from the actual committed release history.
#[ohno::error]
#[display("merge target {} diverges from release history {}; refresh the refs and rebase the target onto release history before assessing", target.quoted(), history.quoted())]
struct UnrelatedMergeTarget {
    history: String,
    target: String,
}

/// A changing ref cannot supply a stable assessment boundary.
#[ohno::error]
#[display(
    "release-history or merge-target reference moved during assessment; capture fresh evidence"
)]
struct AssessmentHistoryMoved;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn history(target: Option<&str>) -> AssessmentHistory {
        AssessmentHistory {
            release_history: "released".into(),
            release_history_revision: "release-branch".into(),
            merge_target: target.map(str::to_owned),
            merge_target_revision: Some("parent-branch".into()),
        }
    }

    #[test]
    fn effective_target_is_only_the_captured_anticipated_predecessor() {
        assert_eq!(
            history(Some("parent-final")).effective_target(),
            Some("parent-final")
        );
        assert_eq!(history(None).effective_target(), None);
    }

    #[test]
    fn absent_and_equal_targets_need_no_ancestry_observation() {
        assert_eq!(
            resolve_merge_target_with(
                "released",
                None,
                |_| panic!("no target ref"),
                |_, _| panic!("no target commit"),
            )
            .unwrap(),
            None
        );
        assert_eq!(
            resolve_merge_target_with(
                "released",
                Some("parent-branch"),
                |revision| {
                    assert_eq!(revision, "parent-branch^{commit}");
                    Ok("released".into())
                },
                |_, _| panic!("equality already establishes ancestry"),
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn target_admission_distinguishes_integrated_descendant_and_divergent_commits() {
        for (integrated, descendant, expected) in [
            (true, false, Ok(None)),
            (false, true, Ok(Some("parent-final".to_owned()))),
            (false, false, Err(())),
        ] {
            let mut queries = Vec::new();
            let result = resolve_merge_target_with(
                "released",
                Some("parent-branch"),
                |revision| {
                    assert_eq!(revision, "parent-branch^{commit}");
                    Ok("parent-final".into())
                },
                |ancestor, descendant_commit| {
                    queries.push((ancestor.to_owned(), descendant_commit.to_owned()));
                    match (ancestor, descendant_commit) {
                        ("parent-final", "released") => Ok(integrated),
                        ("released", "parent-final") => Ok(descendant),
                        _ => panic!("unexpected ancestry query"),
                    }
                },
            );
            let mut expected_queries = vec![("parent-final".into(), "released".into())];
            if !integrated {
                expected_queries.push(("released".into(), "parent-final".into()));
            }
            assert_eq!(queries, expected_queries);
            match expected {
                Ok(expected) => assert_eq!(result.unwrap(), expected),
                Err(()) => {
                    let error = result.unwrap_err();
                    let condition = error.find_source::<UnrelatedMergeTarget>().unwrap();
                    assert_eq!(condition.history, "released");
                    assert_eq!(condition.target, "parent-final");
                }
            }
        }
    }

    #[test]
    fn history_verification_reuses_original_refs_and_rejects_either_changed_identity() {
        for (released, target, accepted) in [
            ("released", "parent-final", true),
            ("new-release", "parent-final", false),
            ("released", "new-parent", false),
            ("new-release", "new-parent", false),
        ] {
            let mut revisions = Vec::new();
            let result = history(Some("parent-final")).verify_with(
                |revision| {
                    revisions.push(revision.to_owned());
                    match revision {
                        "release-branch^{commit}" => Ok(released.into()),
                        "parent-branch^{commit}" => Ok(target.into()),
                        _ => panic!("verification must reuse caller refs"),
                    }
                },
                |ancestor, descendant| {
                    assert!(
                        (ancestor == released && descendant == target)
                            || (ancestor == target && descendant == released)
                    );
                    Ok(ancestor == released)
                },
            );
            assert_eq!(
                revisions,
                ["release-branch^{commit}", "parent-branch^{commit}"]
            );
            if accepted {
                result.unwrap();
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .find_source::<AssessmentHistoryMoved>()
                        .is_some()
                );
            }
        }
    }

    #[test]
    fn verification_compares_effective_targets_not_refs_within_committed_history() {
        history(None)
            .verify_with(
                |revision| match revision {
                    "release-branch^{commit}" => Ok("released".into()),
                    "parent-branch^{commit}" => Ok("older-release".into()),
                    _ => panic!("unexpected ref"),
                },
                |ancestor, descendant| {
                    assert_eq!((ancestor, descendant), ("older-release", "released"));
                    Ok(true)
                },
            )
            .unwrap();
        for (captured, current) in [(None, "parent-final"), (Some("parent-final"), "released")] {
            let result = history(captured).verify_with(
                |revision| match revision {
                    "release-branch^{commit}" => Ok("released".into()),
                    "parent-branch^{commit}" => Ok(current.into()),
                    _ => panic!("unexpected ref"),
                },
                |ancestor, descendant| {
                    assert!(
                        (ancestor == "released" && descendant == "parent-final")
                            || (ancestor == "parent-final" && descendant == "released")
                    );
                    Ok(ancestor == "released")
                },
            );
            assert!(
                result
                    .unwrap_err()
                    .find_source::<AssessmentHistoryMoved>()
                    .is_some()
            );
        }
        let history = AssessmentHistory {
            merge_target_revision: None,
            ..history(None)
        };
        history
            .verify_with(
                |revision| {
                    assert_eq!(revision, "release-branch^{commit}");
                    Ok("released".into())
                },
                |_, _| panic!("no target"),
            )
            .unwrap();
    }

    #[test]
    fn acquisition_failures_propagate_without_becoming_ancestry_or_movement() {
        // Each observation can fail independently; later observations must not run after failure.
        for fail_at in 0..4 {
            let step = Cell::new(0);
            let observe = || -> Result<(), AppError> {
                let current = step.get();
                step.set(current + 1);
                assert!(current <= fail_at);
                if current == fail_at {
                    Err(HistoryObservationFailure::new().into())
                } else {
                    Ok(())
                }
            };
            let result = history(Some("parent-final")).verify_with(
                |revision| {
                    observe()?;
                    Ok(match revision {
                        "release-branch^{commit}" => "released".into(),
                        "parent-branch^{commit}" => "parent-final".into(),
                        _ => panic!("unexpected ref"),
                    })
                },
                |_, _| {
                    observe()?;
                    Ok(false)
                },
            );
            let error = result.unwrap_err();
            assert!(error.find_source::<HistoryObservationFailure>().is_some());
            assert!(error.find_source::<AssessmentHistoryMoved>().is_none());
            assert_eq!(step.get(), fail_at + 1);
        }
    }

    /// Distinguishes failed acquisition from a successfully observed history mismatch.
    #[ohno::error]
    struct HistoryObservationFailure;
}
