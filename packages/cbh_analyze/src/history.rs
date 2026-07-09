//! Resolving the git topology a `Selection` targets: the target ref's first-parent
//! ancestry, its split at the base-branch merge-base, and the dirty-tip admission policy.

use std::collections::HashMap;
use std::time::Instant;

use cbh_analysis::select_commits;
use cbh_config::Config;
use cbh_diag::{Reporter, ReporterExt, count_noun};
use cbh_git::GitHistory;
use cbh_run::RunError;
use jiff::Timestamp;

use super::selection::Selection;

/// How the base-branch dirty-tip exception is gated.
///
/// On a feature branch the target-side commits admit dirty runs unconditionally;
/// this policy only governs the *base* branch's tip. `analyze`/`list` admit a
/// base-tip dirty run only when the working tree is currently dirty (the
/// "evaluating the tool / accidentally on the base branch" case); `prune` admits
/// base-tip dirty runs regardless of the current working-tree state.
#[derive(Clone, Copy)]
pub(crate) enum DirtyTipPolicy {
    /// Admit a base-side tip's dirty runs only when the working tree is dirty now.
    WhenWorkingTreeDirty,
    /// Always treat a base-side tip as admitting dirty runs.
    Always,
}

/// The git topology a selection resolves to: the target ref it was resolved
/// against, the first-parent position of each selected commit, and the per-commit
/// dirty-admission flags. All maps use owned commit IDs so the borrowed
/// `selected` set can drop before the caller's load loop.
pub(crate) struct ResolvedHistory {
    /// The target ref the timeline was resolved against (for diagnostics).
    pub(crate) target_ref: String,
    /// The full commit ID the target ref resolved to — the analyzed tip commit, carried
    /// into the report so it names the exact commit the findings describe.
    pub(crate) tip_commit: String,
    /// Whether the working tree carried uncommitted changes when the topology was
    /// resolved. Probed only under [`DirtyTipPolicy::WhenWorkingTreeDirty`] (and
    /// not under `--no-dirty`); `false` otherwise. The report annotates the tip
    /// `+ uncommitted changes` when set.
    pub(crate) tip_dirty: bool,
    /// First-parent position of each selected commit, for series ordering. An
    /// object whose commit is absent is outside the analyzed history.
    pub(crate) order: HashMap<String, usize>,
    /// Committer timestamp of each first-parent commit, for deciding the
    /// `--since`/`--until` window from topology before any object is fetched. A
    /// commit absent here has an unknown time and is treated as in-window.
    pub(crate) commit_times: HashMap<String, Timestamp>,
    /// Subject line of each first-parent commit that has one, for labeling
    /// `examine`'s per-commit data points. A commit absent here has an empty
    /// subject; only `examine` reads this.
    pub(crate) commit_subjects: HashMap<String, String>,
    /// Whether each selected commit admits dirty (uncommitted-tree) snapshots.
    pub(crate) admit_dirty: HashMap<String, bool>,
    /// Whether a commit's dirty runs are admitted *only* by the base-branch
    /// dirty-tree exception, which triggers the ephemeral-data warning.
    pub(crate) dirty_base_exception: HashMap<String, bool>,
    /// First-parent topological index of the merge-base, used by branch mode to
    /// split base-side history from the branch's own commits. `None` when the
    /// merge-base is off the target's first-parent line (an off-chain merge-base);
    /// a merge-base that cannot be determined at all is a hard error, not `None`.
    pub(crate) merge_base_index: Option<usize>,
    /// Whether the target's tip *is* its own merge-base with the base: the signal
    /// that this is an official base-branch view rather than a feature branch.
    pub(crate) tip_is_merge_base: bool,
}

/// Resolves the git topology for a selection: the target ref's first-parent
/// ancestry, the merge-base with the base ref, and the per-commit dirty-admission
/// flags. Requires a repository — an unresolvable target ref is an error rather
/// than an empty success, and so is a merge-base that cannot be determined (the
/// base ref does not resolve, or it shares no common ancestor with the target),
/// rather than silently falling back to a base-branch (history) view of the
/// incomplete topology.
pub(crate) async fn resolve_history<G>(
    git: &G,
    config: &Config,
    selection: &Selection<'_>,
    policy: DirtyTipPolicy,
    reporter: &dyn Reporter,
) -> Result<ResolvedHistory, RunError>
where
    G: GitHistory,
{
    // Resolving the timeline requires a repository: the topology comes from git
    // history, not from stored timestamps. An unresolvable target ref means there
    // is no repository here (or the branch does not exist), which is an error.
    let target_ref = selection.context.unwrap_or("HEAD");
    let Some(target_commit_id) = git.resolve(target_ref).await.map_err(RunError::Io)? else {
        return Err(RunError::Analyze {
            message: format!(
                "this command requires a git repository: could not resolve {target_ref:?}. \
                 Run inside a repository (or pass --repo / --context)."
            ),
        });
    };

    let base_commit_id = resolve_base_ref(git, config, selection.base).await?;
    let first_parent_started = Instant::now();
    let first_parent = git
        .first_parent(&target_commit_id)
        .await
        .map_err(RunError::Io)?;
    reporter.timing(
        "git.first_parent ancestry walk (target's first-parent line)",
        first_parent_started.elapsed(),
    );
    // Split the first-parent ancestry into the commit ID timeline (for commit selection
    // and the merge-base lookup) and a commit ID -> committer-time map (for the window).
    let commit_count = first_parent.len();
    let mut ancestry: Vec<String> = Vec::with_capacity(commit_count);
    let mut commit_times: HashMap<String, Timestamp> = HashMap::new();
    let mut commit_subjects: HashMap<String, String> = HashMap::new();
    for commit in first_parent {
        if let Some(time) = commit.committer_time {
            commit_times.insert(commit.commit_id.clone(), time);
        }
        if !commit.subject.is_empty() {
            commit_subjects.insert(commit.commit_id.clone(), commit.subject);
        }
        ancestry.push(commit.commit_id);
    }
    let merge_base = match &base_commit_id {
        Some(base) => git
            .merge_base(&target_commit_id, base)
            .await
            .map_err(RunError::Io)?,
        None => None,
    };

    reporter.if_enabled(|notes| {
        notes.note(&format!(
            "target ref {target_ref} resolves to {target_commit_id}; {} on its first-parent line",
            count_noun(commit_count, "commit")
        ));
        notes.note(&format!(
            "base ref resolves to {}; merge-base with target is {}",
            base_commit_id.as_deref().unwrap_or("<none>"),
            merge_base.as_deref().unwrap_or("<none>")
        ));
    });

    // The analysis is a comparison against the base branch: it splits the target's
    // first-parent line at the merge-base. A merge-base we cannot determine leaves
    // no topology to split on, and guessing a mode from the incomplete history would
    // silently mislead — so refuse and say how to supply the missing history. The
    // usual cause is a shallow clone whose depth stops short of the branch point, or
    // a checkout that never fetched the base branch.
    let Some(merge_base) = merge_base else {
        let message = match base_commit_id.as_deref() {
            // The base resolved, but shares no common ancestor with the target. By
            // far the usual cause is a shallow clone whose depth stops short of the
            // branch point, so the primary remedy is to deepen the clone — not to
            // pick a different base. Only once the history is known-complete is a
            // genuinely disjoint base worth calling out, and even then the deliberate
            // `--base` a user passed is theirs to reconsider, not ours to second-guess.
            Some(base) => {
                let remedy = match selection.base {
                    Some(explicit) => format!(
                        " If the history is already complete, {explicit} is genuinely unrelated \
                         to the target and cannot serve as its base."
                    ),
                    None => " If the history is already complete, the base branch is unrelated to \
                             the target; name the intended base with --base or \
                             project.default_branch."
                        .to_owned(),
                };
                format!(
                    "could not determine the merge-base of the target {target_ref} \
                     ({target_commit_id}) and the base commit {base}: they share no common \
                     ancestor in the available history. This is almost always a shallow clone \
                     whose depth stops short of the branch point — fetch the full history (`git \
                     fetch --unshallow`, or set fetch-depth: 0 on actions/checkout) so the branch \
                     point is present.{remedy}"
                )
            }
            None => format!(
                "could not determine the base branch to compare {target_ref} against: no \
                 --base was given and no default branch could be resolved. Pass an explicit \
                 --base, set project.default_branch, or make the default branch available \
                 (a shallow clone or a checkout that never fetched the base branch is the \
                 usual cause)."
            ),
        };
        return Err(RunError::Analyze { message });
    };

    // The base-branch dirty-tip exception: `analyze`/`list` admit a base-side tip's
    // dirty runs only when the working tree is currently dirty (`--no-dirty` skips
    // both the probe and the exception); `prune` admits them unconditionally so it
    // can remove them regardless of the present working-tree state. The probe result
    // is reused for the report's tip annotation, so `analyze` never runs it twice.
    let working_tree_dirty = match policy {
        DirtyTipPolicy::WhenWorkingTreeDirty if !selection.no_dirty => {
            git.is_dirty().await.map_err(RunError::Io)?
        }
        _ => false,
    };
    let dirty_tip_exception = match policy {
        DirtyTipPolicy::Always => !selection.no_dirty,
        DirtyTipPolicy::WhenWorkingTreeDirty => {
            if working_tree_dirty {
                reporter.note_with(|| {
                    "working tree is dirty: dirty snapshots on a base-side tip will be admitted"
                        .to_owned()
                });
            }
            working_tree_dirty
        }
    };

    let selected = select_commits(
        &ancestry,
        Some(merge_base.as_str()),
        !selection.no_dirty,
        dirty_tip_exception,
    );
    let order: HashMap<String, usize> = selected
        .iter()
        .enumerate()
        .map(|(index, one)| (one.commit.clone(), index))
        .collect();
    let admit_dirty: HashMap<String, bool> = selected
        .iter()
        .map(|one| (one.commit.clone(), one.admit_dirty))
        .collect();
    let dirty_base_exception: HashMap<String, bool> = selected
        .iter()
        .map(|one| (one.commit.clone(), one.dirty_base_exception))
        .collect();

    // The merge-base's topological position (when it is on the analyzed chain)
    // splits base-side history from the branch's own commits in branch mode. It is
    // absent only when the merge-base is off the target's first-parent line.
    let merge_base_index = order.get(&merge_base).copied();
    // The target's tip is its own merge-base exactly when this is an official
    // base-branch view rather than a feature branch.
    let tip_is_merge_base = merge_base == target_commit_id;

    Ok(ResolvedHistory {
        target_ref: target_ref.to_owned(),
        tip_commit: target_commit_id,
        tip_dirty: working_tree_dirty,
        order,
        commit_times,
        commit_subjects,
        admit_dirty,
        dirty_base_exception,
        merge_base_index,
        tip_is_merge_base,
    })
}

/// The ephemeral-data warning appended when a dirty base-branch-tip run is admitted.
pub(crate) fn dirty_base_exception_warning() -> String {
    "Warning: analysis included dirty runs (with uncommitted changes) on top of the \
     base branch. These may be excluded from future analysis. Switch to a new branch \
     to persist benchmark history of your changes."
        .to_owned()
}

/// Resolves the base ref the target's history is split against, returning its
/// commit ID or `None` when no base can be determined.
///
/// Precedence: an explicit `--base` (an error if it does not resolve), then the
/// configured `project.default_branch`, then the repository's detected default
/// branch (`origin/HEAD`, else `main`/`master`).
pub(crate) async fn resolve_base_ref<G: GitHistory>(
    git: &G,
    config: &Config,
    base: Option<&str>,
) -> Result<Option<String>, RunError> {
    if let Some(base) = base {
        return git
            .resolve(base)
            .await
            .map_err(RunError::Io)?
            .map(Some)
            .ok_or_else(|| RunError::Analyze {
                message: format!("could not resolve --base {base:?}"),
            });
    }
    if let Some(default) = config.project.default_branch.as_deref()
        && let Some(resolved) = git.resolve(default).await.map_err(RunError::Io)?
    {
        return Ok(Some(resolved));
    }
    if let Some(name) = git.default_branch().await.map_err(RunError::Io)?
        && let Some(resolved) = git.resolve(&name).await.map_err(RunError::Io)?
    {
        return Ok(Some(resolved));
    }
    Ok(None)
}

/// Resolves the *display name* of the base ref (without resolving it to a commit ID),
/// for diagnostics such as the `prune --prune-base` guard.
///
/// Mirrors [`resolve_base_ref`]'s precedence: an explicit `--base`, then the
/// configured `project.default_branch` (only when it resolves), then the
/// repository's detected default branch. Returns `None` when no base can be named.
pub(crate) async fn resolve_base_name<G: GitHistory>(
    git: &G,
    config: &Config,
    base: Option<&str>,
) -> Result<Option<String>, RunError> {
    if let Some(base) = base {
        return Ok(Some(base.to_owned()));
    }
    if let Some(default) = config.project.default_branch.as_deref()
        && git.resolve(default).await.map_err(RunError::Io)?.is_some()
    {
        return Ok(Some(default.to_owned()));
    }
    git.default_branch().await.map_err(RunError::Io)
}
