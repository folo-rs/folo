//! The `clean` command: remove the dirty runs a matching `analyze`/`list` pass
//! would include.
//!
//! `clean` accepts the same data-set-selection options as `analyze`/`list` (minus
//! `--no-dirty`, meaningless when the command only ever touches dirty runs, and
//! `--metric`, a series filter rather than a run selector) and resolves the
//! identical commit topology via [`resolve_history`](super::resolve_history).
//! Where `analyze`/`list` admit a base-branch tip's dirty runs only when the
//! working tree is currently dirty, `clean` admits them unconditionally
//! ([`DirtyTipPolicy::Always`](super::DirtyTipPolicy)) — discarding ephemeral
//! dirty snapshots is the whole point, regardless of the present tree state.
//! `--dry-run` previews what would be removed without deleting anything.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::config::{Config, load_config};
use crate::git_history::{GitHistory, SystemGitHistory};
use crate::model::ResultSet;
use crate::report::{Reporter, StderrReporter};
use crate::storage::{Storage, build_storage};
use crate::text::count_noun;
use crate::wiring::{resolve_config_path, resolve_project_id, resolve_repo};
use crate::{CleanOptions, RunError, RunOutcome};

use super::discriminant::DiscriminantSet;
use super::report::ReportFormat;
use super::{
    DirtyTipPolicy, ResolvedHistory, Selection, facet_filtered_candidates, parse_format,
    parse_since, parsed_facets, resolve_history,
};

/// The real `clean`: load configuration, wire the configured storage and git
/// history, and orchestrate.
pub(crate) async fn execute(
    options: &CleanOptions,
    workspace_dir: &Path,
) -> Result<RunOutcome, RunError> {
    let reporter = StderrReporter::new(options.verbose);

    let config_path = resolve_config_path(workspace_dir, options.config_path.as_deref());
    reporter.note(&format!(
        "loading configuration from {}",
        config_path.display()
    ));
    let config = load_config(&config_path).await?;

    let project_id = resolve_project_id(&config, workspace_dir);
    let storage = build_storage(&config, workspace_dir)?;

    let git = SystemGitHistory::new(resolve_repo(workspace_dir, options.repo.as_deref()));

    clean_with(&git, &storage, &project_id, &config, options, &reporter).await
}

/// Storage- and git-generic `clean`: resolve the same dirty runs `analyze`/`list`
/// would include on the target side (plus the base-branch tip unconditionally),
/// then either preview them (`--dry-run`) or delete them.
pub(crate) async fn clean_with<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    config: &Config,
    options: &CleanOptions,
    reporter: &dyn Reporter,
) -> Result<RunOutcome, RunError>
where
    G: GitHistory,
    S: Storage,
{
    let format = parse_format(options.format.as_deref())?;
    let since = parse_since(options.since.as_deref())?;
    let selection = Selection::from_clean(options);

    let (engine, facets) = parsed_facets(&selection)?;
    let candidates =
        facet_filtered_candidates(storage, project_id, engine, &facets, reporter).await?;

    let ResolvedHistory {
        target_ref,
        order,
        admit_dirty,
        ..
    } = resolve_history(git, config, &selection, DirtyTipPolicy::Always, reporter).await?;

    // Select each dirty object on a commit whose dirty runs the matching
    // analyze/list would admit. Clean runs are never touched, so a corrupt clean
    // object can never block a cleanup.
    let mut items: Vec<RemovalItem> = Vec::new();
    for (key, parsed) in candidates {
        if !parsed.is_dirty() {
            reporter.note(&format!(
                "skipping {key}: clean run (clean removes only dirty runs)"
            ));
            continue;
        }
        let Some(&index) = order.get(&parsed.commit) else {
            reporter.note(&format!(
                "skipping {key}: commit {} is not on {target_ref}'s history",
                parsed.commit
            ));
            continue;
        };
        if !admit_dirty
            .get(parsed.commit.as_str())
            .copied()
            .unwrap_or(false)
        {
            reporter.note(&format!(
                "skipping {key}: dirty snapshot on a base-side commit ({} \
                 admits only clean runs)",
                parsed.commit
            ));
            continue;
        }
        // Apply `--since` only when set: it requires the object's effective time,
        // so a corrupt dirty object surfaces only under `--since`.
        if let Some(since) = since {
            let bytes = storage.get(&key).await.map_err(RunError::Storage)?;
            let text = String::from_utf8(bytes).map_err(|error| RunError::Analyze {
                message: format!("stored object {key} is not valid UTF-8: {error}"),
            })?;
            let result = ResultSet::from_json(&text).map_err(|error| RunError::Analyze {
                message: format!("stored object {key} is not a valid result set: {error}"),
            })?;
            if result.context.timestamps.effective < since {
                reporter.note(&format!(
                    "skipping {key}: effective time is before the --since cutoff"
                ));
                continue;
            }
        }
        reporter.note(&format!("selected {key} for removal"));
        items.push(RemovalItem {
            index,
            set: parsed.set,
            commit: parsed.commit,
            key,
        });
    }

    let plan = build_plan(project_id, &target_ref, &items);

    if !options.dry_run {
        for set in &plan.sets {
            for commit in &set.commits {
                for key in &commit.keys {
                    reporter.note(&format!("deleting {key}"));
                    storage.delete(key).await.map_err(RunError::Storage)?;
                }
            }
        }
    }

    let message = render_plan(&plan, format, options.dry_run);
    Ok(RunOutcome::Completed { message })
}

/// One dirty object slated for removal, with its first-parent position.
#[derive(Clone, Debug)]
struct RemovalItem {
    /// First-parent position of the commit, for oldest-first ordering.
    index: usize,
    /// The discriminant set the object belongs to.
    set: DiscriminantSet,
    /// The commit the snapshot was measured against.
    commit: String,
    /// The storage key to delete.
    key: String,
}

/// One commit's dirty runs to remove, in first-parent order.
#[derive(Clone, Debug)]
struct CommitRemoval {
    /// The commit the runs were measured against (full SHA, or a label in tests).
    commit: String,
    /// How many dirty runs are removed on this commit.
    runs: usize,
    /// The storage keys removed on this commit, in storage-key order.
    keys: Vec<String>,
}

/// One discriminant set's slice of the removal plan.
#[derive(Clone, Debug)]
struct SetRemoval {
    /// The comparable partition this slice covers.
    set: DiscriminantSet,
    /// Dirty runs removed in this set.
    runs: usize,
    /// Per-commit breakdown, oldest first by git topology.
    commits: Vec<CommitRemoval>,
}

/// The fully resolved removal plan, ready to render.
#[derive(Clone, Debug)]
struct Plan {
    /// The project the runs belong to.
    project: String,
    /// The target ref the topology was resolved against.
    target_ref: String,
    /// Per-set breakdown, one entry per discriminant set with removable runs.
    sets: Vec<SetRemoval>,
    /// Total dirty runs removed across every set.
    total_runs: usize,
}

/// Groups the selected dirty objects by discriminant set and commit (ordered by
/// first-parent topology, oldest first).
fn build_plan(project_id: &str, target_ref: &str, items: &[RemovalItem]) -> Plan {
    let mut sets: Vec<DiscriminantSet> = items.iter().map(|item| item.set.clone()).collect();
    sets.sort();
    sets.dedup();

    let set_removals: Vec<SetRemoval> = sets
        .iter()
        .map(|set| {
            let in_set: Vec<&RemovalItem> = items.iter().filter(|item| &item.set == set).collect();

            // Key by first-parent position so commits read oldest-first.
            let mut by_commit: BTreeMap<usize, CommitRemoval> = BTreeMap::new();
            for item in &in_set {
                let entry = by_commit
                    .entry(item.index)
                    .or_insert_with(|| CommitRemoval {
                        commit: item.commit.clone(),
                        runs: 0,
                        keys: Vec::new(),
                    });
                entry.runs = entry.runs.saturating_add(1);
                entry.keys.push(item.key.clone());
            }

            SetRemoval {
                set: set.clone(),
                runs: in_set.len(),
                commits: by_commit.into_values().collect(),
            }
        })
        .collect();

    Plan {
        project: project_id.to_owned(),
        target_ref: target_ref.to_owned(),
        sets: set_removals,
        total_runs: items.len(),
    }
}

/// The past-tense or conditional verb for the plan, matching `--dry-run`.
fn verb(dry_run: bool) -> &'static str {
    if dry_run { "Would remove" } else { "Removed" }
}

/// Renders the removal plan in the requested format.
fn render_plan(plan: &Plan, format: ReportFormat, dry_run: bool) -> String {
    match format {
        ReportFormat::Text => render_plan_text(plan, dry_run),
        ReportFormat::Markdown => render_plan_markdown(plan, dry_run),
        ReportFormat::Json => render_plan_json(plan, dry_run),
    }
}

fn render_plan_text(plan: &Plan, dry_run: bool) -> String {
    let mut lines = vec![format!(
        "Clean plan for project {} (target {})",
        plan.project, plan.target_ref
    )];
    if plan.sets.is_empty() {
        lines.push(String::new());
        lines.push("No dirty run matches the selection.".to_owned());
    } else {
        for set in &plan.sets {
            lines.push(String::new());
            lines.push(format!(
                "{} (os={} arch={})",
                set.set,
                set.set.os(),
                set.set.architecture()
            ));
            lines.push(format!(
                "  {} across {}",
                count_noun(set.runs, "dirty run"),
                count_noun(set.commits.len(), "commit")
            ));
            for commit in &set.commits {
                lines.push(format!(
                    "    {}  {}",
                    commit.commit,
                    count_noun(commit.runs, "dirty run")
                ));
            }
        }
        lines.push(String::new());
        lines.push(format!(
            "{} {} across {}",
            verb(dry_run),
            count_noun(plan.total_runs, "dirty run"),
            count_noun(plan.sets.len(), "discriminant set")
        ));
    }
    format!("{}\n", lines.join("\n"))
}

fn render_plan_markdown(plan: &Plan, dry_run: bool) -> String {
    let mut lines = vec![format!(
        "# Clean plan for {} (target {})",
        plan.project, plan.target_ref
    )];
    if plan.sets.is_empty() {
        lines.push(String::new());
        lines.push("No dirty run matches the selection.".to_owned());
    } else {
        for set in &plan.sets {
            lines.push(String::new());
            lines.push(format!(
                "## {} (os={} arch={})",
                set.set,
                set.set.os(),
                set.set.architecture()
            ));
            lines.push(String::new());
            lines.push(format!(
                "{} across {}",
                count_noun(set.runs, "dirty run"),
                count_noun(set.commits.len(), "commit")
            ));
            lines.push(String::new());
            lines.push("| Commit | Dirty runs |".to_owned());
            lines.push("| --- | --- |".to_owned());
            for commit in &set.commits {
                lines.push(format!("| {} | {} |", commit.commit, commit.runs));
            }
        }
        lines.push(String::new());
        lines.push(format!(
            "**{} {} across {}**",
            verb(dry_run),
            count_noun(plan.total_runs, "dirty run"),
            count_noun(plan.sets.len(), "discriminant set")
        ));
    }
    format!("{}\n", lines.join("\n"))
}

fn render_plan_json(plan: &Plan, dry_run: bool) -> String {
    #[derive(Serialize)]
    struct JsonCommit<'a> {
        commit: &'a str,
        runs: usize,
        keys: &'a [String],
    }
    #[derive(Serialize)]
    struct JsonSet<'a> {
        engine: &'a str,
        target_triple: &'a str,
        os: &'a str,
        architecture: &'a str,
        machine: &'a str,
        runs: usize,
        commits: Vec<JsonCommit<'a>>,
    }
    #[derive(Serialize)]
    struct JsonTotals {
        runs: usize,
        discriminant_sets: usize,
    }
    #[derive(Serialize)]
    struct JsonPlan<'a> {
        project: &'a str,
        target_ref: &'a str,
        dry_run: bool,
        sets: Vec<JsonSet<'a>>,
        totals: JsonTotals,
    }

    let sets: Vec<JsonSet<'_>> = plan
        .sets
        .iter()
        .map(|set| JsonSet {
            engine: &set.set.engine,
            target_triple: &set.set.target_triple,
            os: set.set.os(),
            architecture: set.set.architecture(),
            machine: &set.set.machine,
            runs: set.runs,
            commits: set
                .commits
                .iter()
                .map(|commit| JsonCommit {
                    commit: &commit.commit,
                    runs: commit.runs,
                    keys: &commit.keys,
                })
                .collect(),
        })
        .collect();

    let document = JsonPlan {
        project: &plan.project,
        target_ref: &plan.target_ref,
        dry_run,
        sets,
        totals: JsonTotals {
            runs: plan.total_runs,
            discriminant_sets: plan.sets.len(),
        },
    };
    serde_json::to_string_pretty(&document).expect("clean plan structures always serialize to JSON")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use futures::executor::block_on;
    use jiff::Timestamp;

    use crate::config::{Config, parse_config};
    use crate::context::{CiInfo, GitInfo, RunContext, Timestamps, ToolchainInfo};
    use crate::git_history::FakeGitHistory;
    use crate::model::{BenchmarkId, Metric, MetricKind, ResultRecord, ResultSet};
    use crate::report::RecordingReporter;
    use crate::storage::{MemoryStorage, Storage};

    use super::*;

    fn config() -> Config {
        parse_config("[storage.local]\npath = \"./data\"\n").expect("config parses")
    }

    fn options() -> CleanOptions {
        CleanOptions::default()
    }

    /// A minimal result set with the given effective time and commit.
    fn set(effective: i64, commit: &str) -> ResultSet {
        let time = Timestamp::from_second(effective).expect("seconds within range");
        let context = RunContext::new(
            Timestamps::new(time, time, time),
            GitInfo {
                commit: Some(commit.to_owned()),
                short_commit: Some(commit.to_owned()),
                branch: Some("main".to_owned()),
                dirty: false,
            },
            CiInfo::default(),
            ToolchainInfo::default(),
            "0.0.1".to_owned(),
        );
        let record = ResultRecord::new(
            BenchmarkId::new(
                Some("nm".to_owned()),
                "nm::observe".to_owned(),
                Some("pull".to_owned()),
                None,
            ),
            vec![Metric::new(
                "Ir".to_owned(),
                MetricKind::InstructionCount,
                100.0,
                Some("count".to_owned()),
            )],
        );
        ResultSet::new(context, vec![record])
    }

    fn clean_key(commit: &str) -> String {
        format!("v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/clean.json")
    }

    fn dirty_key(commit: &str, unix: i64) -> String {
        format!("v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/dirty-{unix}.json")
    }

    fn store(storage: &MemoryStorage, key: &str, value: &ResultSet) {
        let json = value.to_json().expect("result set serializes");
        block_on(storage.put(key, json.as_bytes())).expect("store succeeds");
    }

    fn keys(storage: &MemoryStorage) -> Vec<String> {
        let mut keys = block_on(storage.list("v2/")).expect("list succeeds");
        keys.sort();
        keys
    }

    /// A linear master history `c0 - c1 - c2 - c3`, HEAD at the tip.
    fn linear_git() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .branch("master", "c3")
            .head("master")
            .mark_default("master");
        git
    }

    /// A master history with a feature branch off `c1`:
    ///
    /// ```text
    /// master:  c0 - c1 - c2 - c3
    ///                \
    /// feature:        f1 - f2   (HEAD)
    /// ```
    fn feature_git() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .commit("f1", Some("c1"))
            .commit("f2", Some("f1"))
            .branch("master", "c3")
            .branch("feature", "f2")
            .head("feature")
            .mark_default("master");
        git
    }

    /// Drives `clean_with` and unwraps the rendered message.
    fn clean(storage: &MemoryStorage, git: &FakeGitHistory, options: &CleanOptions) -> String {
        let outcome = block_on(clean_with(
            git,
            storage,
            "folo",
            &config(),
            options,
            &RecordingReporter::new(),
        ))
        .expect("clean runs");
        match outcome {
            RunOutcome::Completed { message } => message,
            RunOutcome::Analyzed { .. } => panic!("clean returns a Completed outcome"),
        }
    }

    #[test]
    fn clean_removes_target_side_dirty_runs_on_a_feature_branch() {
        let storage = MemoryStorage::new();
        // Clean runs across the whole history.
        for commit in ["c0", "c1", "f1", "f2"] {
            store(&storage, &clean_key(commit), &set(0, commit));
        }
        // Dirty snapshots: base-side (c1) must survive; target-side (f1, f2) go.
        store(&storage, &dirty_key("c1", 150), &set(150, "c1"));
        store(&storage, &dirty_key("f1", 200), &set(200, "f1"));
        store(&storage, &dirty_key("f2", 300), &set(300, "f2"));
        let git = feature_git();

        let opts = CleanOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let report = clean(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(
            parsed["totals"]["runs"], 2,
            "two target-side dirty runs: {report}"
        );
        assert_eq!(parsed["dry_run"], false);

        let remaining = keys(&storage);
        assert!(
            remaining.contains(&dirty_key("c1", 150)),
            "base-side dirty survives"
        );
        assert!(remaining.contains(&clean_key("f1")), "clean runs survive");
        assert!(
            !remaining.contains(&dirty_key("f1", 200)),
            "target dirty removed"
        );
        assert!(
            !remaining.contains(&dirty_key("f2", 300)),
            "target dirty removed"
        );
    }

    #[test]
    fn clean_removes_the_base_tip_dirty_unconditionally() {
        // The key divergence from analyze/list: on the base branch with a CLEAN
        // working tree, the base-branch tip's dirty runs are still removed (no
        // working-tree-dirty guard). Earlier base-side dirty runs stay untouched.
        let storage = MemoryStorage::new();
        for commit in ["c0", "c1", "c2", "c3"] {
            store(&storage, &clean_key(commit), &set(0, commit));
        }
        store(&storage, &dirty_key("c1", 150), &set(150, "c1"));
        store(&storage, &dirty_key("c3", 300), &set(300, "c3"));
        let git = linear_git(); // No mark_dirty: the working tree is clean.

        let report = clean(&storage, &git, &options());
        assert!(report.contains("Removed 1 dirty run"), "{report}");

        let remaining = keys(&storage);
        assert!(
            !remaining.contains(&dirty_key("c3", 300)),
            "the base-tip dirty run is removed even with a clean tree"
        );
        assert!(
            remaining.contains(&dirty_key("c1", 150)),
            "an earlier base-side dirty run is left untouched"
        );
    }

    #[test]
    fn dry_run_previews_without_deleting() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &set(0, "c0"));
        store(&storage, &clean_key("c1"), &set(0, "c1"));
        store(&storage, &clean_key("f1"), &set(0, "f1"));
        store(&storage, &dirty_key("f1", 200), &set(200, "f1"));
        let git = feature_git();

        let before = keys(&storage);
        let opts = CleanOptions {
            dry_run: true,
            format: Some("json".to_owned()),
            ..options()
        };
        let report = clean(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(parsed["totals"]["runs"], 1, "{report}");

        // Nothing was actually deleted.
        assert_eq!(keys(&storage), before, "--dry-run deletes nothing");
    }

    #[test]
    fn clean_text_format_reports_would_remove_under_dry_run() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &set(0, "c0"));
        store(&storage, &clean_key("f1"), &set(0, "f1"));
        store(&storage, &dirty_key("f1", 200), &set(200, "f1"));
        let git = feature_git();

        let opts = CleanOptions {
            dry_run: true,
            ..options()
        };
        let report = clean(&storage, &git, &opts);
        assert!(report.contains("Clean plan for project folo"), "{report}");
        assert!(report.contains("Would remove 1 dirty run"), "{report}");
        assert!(report.contains("1 discriminant set"), "{report}");
    }

    #[test]
    fn clean_never_touches_clean_runs() {
        let storage = MemoryStorage::new();
        for commit in ["c0", "c1", "f1", "f2"] {
            store(&storage, &clean_key(commit), &set(0, commit));
        }
        let git = feature_git();

        let report = clean(&storage, &git, &options());
        assert!(
            report.contains("No dirty run matches the selection."),
            "{report}"
        );
        // Every clean run survives.
        assert_eq!(keys(&storage).len(), 4, "clean runs are untouched");
    }

    #[test]
    fn clean_requires_a_repository() {
        let storage = MemoryStorage::new();
        store(&storage, &dirty_key("c0", 100), &set(100, "c0"));
        let git = FakeGitHistory::new(); // No commits: HEAD does not resolve.
        let error = block_on(clean_with(
            &git,
            &storage,
            "folo",
            &config(),
            &options(),
            &RecordingReporter::new(),
        ))
        .unwrap_err();
        match error {
            RunError::Analyze { message } => {
                assert!(message.contains("requires a git repository"), "{message}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn engine_facet_restricts_the_cleanup() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("f1"), &set(0, "f1"));
        store(&storage, &dirty_key("f1", 200), &set(200, "f1"));
        // A criterion dirty run on the same commit must survive an engine-scoped
        // callgrind cleanup.
        let criterion_dirty = "v2/folo/criterion/x86_64-unknown-linux-gnu/m1/f1/dirty-200.json";
        store(&storage, criterion_dirty, &set(200, "f1"));
        let git = feature_git();

        let opts = CleanOptions {
            engine: Some("callgrind".to_owned()),
            ..options()
        };
        clean(&storage, &git, &opts);

        let remaining = keys(&storage);
        assert!(
            !remaining.contains(&dirty_key("f1", 200)),
            "callgrind dirty removed"
        );
        assert!(
            remaining.contains(&criterion_dirty.to_owned()),
            "criterion dirty survives the engine-scoped cleanup"
        );
    }

    #[test]
    fn since_only_removes_runs_on_or_after_the_cutoff() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("f1"), &set(0, "f1"));
        // Two dirty snapshots on the target side, one before and one after the cutoff.
        store(&storage, &dirty_key("f1", 100), &set(100, "f1"));
        store(&storage, &dirty_key("f2", 300), &set(300, "f2"));
        // A snapshot whose effective time is exactly the cutoff: the cutoff is
        // inclusive, so it is removed.
        store(&storage, &dirty_key("f1", 180), &set(180, "f1"));
        let git = feature_git();

        let opts = CleanOptions {
            since: Some("1970-01-01T00:03:00Z".to_owned()), // 180s
            ..options()
        };
        clean(&storage, &git, &opts);

        let remaining = keys(&storage);
        assert!(
            remaining.contains(&dirty_key("f1", 100)),
            "a run before --since is kept"
        );
        assert!(
            !remaining.contains(&dirty_key("f1", 180)),
            "a run exactly at --since is removed (inclusive cutoff)"
        );
        assert!(
            !remaining.contains(&dirty_key("f2", 300)),
            "a run after --since is removed"
        );
    }

    #[test]
    fn commits_are_listed_oldest_first_by_topology() {
        let storage = MemoryStorage::new();
        store(&storage, &dirty_key("f2", 300), &set(300, "f2"));
        store(&storage, &dirty_key("f1", 200), &set(200, "f1"));
        let git = feature_git();

        let opts = CleanOptions {
            dry_run: true,
            format: Some("json".to_owned()),
            ..options()
        };
        let report = clean(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        let commits = parsed["sets"][0]["commits"].as_array().unwrap();
        assert_eq!(commits[0]["commit"], "f1", "oldest first: {report}");
        assert_eq!(commits[1]["commit"], "f2");
    }

    #[test]
    fn clean_markdown_format_renders_a_table_per_set() {
        let storage = MemoryStorage::new();
        store(&storage, &dirty_key("f1", 200), &set(200, "f1"));
        store(&storage, &dirty_key("f2", 300), &set(300, "f2"));
        let git = feature_git();

        let opts = CleanOptions {
            dry_run: true,
            format: Some("markdown".to_owned()),
            ..options()
        };
        let report = clean(&storage, &git, &opts);
        assert!(report.contains("# Clean plan for"), "{report}");
        assert!(report.contains("| Commit | Dirty runs |"), "{report}");
        assert!(report.contains("| f1 |"), "{report}");
        assert!(report.contains("**Would remove"), "{report}");
    }
}
