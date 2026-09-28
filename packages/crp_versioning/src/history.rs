//! Resolves actual release history and an optional final snapshot anticipated to squash onto it.

use crp_diag::{Quotable as _, Verbose};
use crp_workspace::git::{DefaultBase, GitRepo};
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
                let default = git.default_base()?;
                verbose.note(|| match &default {
                    DefaultBase::RemoteHead(revision) => format!(
                        "no --release-history given; the remote default {} supplies committed release history",
                        revision.quoted()
                    ),
                    DefaultBase::Convention(revision) => format!(
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

    pub(crate) fn verify(&self, git: &GitRepo) -> Result<(), AppError> {
        let release_history =
            git.rev_parse(&format!("{}^{{commit}}", self.release_history_revision))?;
        let merge_target =
            resolve_merge_target(git, &release_history, self.merge_target_revision.as_deref())?;
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
pub fn resolve_merge_target(
    git: &GitRepo,
    release_history: &str,
    merge_target: Option<&str>,
) -> Result<Option<String>, AppError> {
    let Some(revision) = merge_target else {
        return Ok(None);
    };
    let target = git.rev_parse(&format!("{revision}^{{commit}}"))?;
    if target == release_history || git.is_ancestor(&target, release_history)? {
        return Ok(None);
    }
    if !git.is_ancestor(release_history, &target)? {
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
