//! Resolving the git topology a `Selection` targets: the target ref's first-parent
//! ancestry, its split at the base-branch merge-base, and the dirty-tip admission policy.

use std::collections::HashMap;
use std::time::Instant;

use cargo_bench_history_core::analyze::select_commits;
use jiff::Timestamp;

use super::selection::Selection;
use crate::RunError;
use crate::config::Config;
use crate::git_history::GitHistory;
use crate::report::{Reporter, ReporterExt};
use crate::text::count_noun;

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
    /// split base-side history from the branch's own commits. `None` when no base
    /// is known or the merge-base is off the analyzed chain.
    pub(crate) merge_base_index: Option<usize>,
    /// Whether the target's tip *is* its own merge-base with the base (or no base
    /// is known): the signal that this is an official base-branch view.
    pub(crate) tip_is_merge_base: bool,
}

/// Resolves the git topology for a selection: the target ref's first-parent
/// ancestry, the merge-base with the base ref, and the per-commit dirty-admission
/// flags. Requires a repository — an unresolvable target ref is an error rather
/// than an empty success.
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
        merge_base.as_deref(),
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
    // splits base-side history from the branch's own commits in branch mode.
    let merge_base_index = merge_base
        .as_deref()
        .and_then(|base| order.get(base).copied());
    // The target's tip is its own merge-base (or no base is known) exactly when
    // this is an official base-branch view rather than a feature branch.
    let tip_is_merge_base = merge_base
        .as_deref()
        .is_none_or(|base| base == target_commit_id);

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
