//! The `prune` command: delete stored runs (and their blessing sidecars) from the
//! data set a matching `analyze`/`list` pass resolves.
//!
//! `prune` accepts the same data-set-selection options as `analyze`/`list` and
//! resolves the identical commit topology via [`resolve_history`](super::resolve_history),
//! admitting a base-branch tip's dirty runs unconditionally
//! ([`DirtyTipPolicy::Always`](super::DirtyTipPolicy)) — a deletion tool sees every
//! stored run as a candidate, regardless of the present working-tree state.
//!
//! By default `prune` deletes clean *and* dirty runs (plus the blessing sidecars on
//! every commit whose clean run it removes); `--dirty` restricts it to dirty
//! (uncommitted-tree) snapshots and `--clean` to clean runs (and their blessings).
//! Because deleting clean history is destructive, the default and `--clean` scopes
//! refuse an un-narrowed selection unless `--all` is given: narrow with a facet, a
//! `<commit>` argument, `--since`, or `--until`. `--dirty` discards only ephemeral
//! data and is exempt from that guard. `--dry-run` previews what would be removed
//! without deleting anything.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::Serialize;

use crate::config::{Config, load_config};
use crate::git_history::{GitHistory, SystemGitHistory};
use crate::model::ResultSet;
use crate::report::{Reporter, StderrReporter};
use crate::storage::{Storage, build_storage};
use crate::text::count_noun;
use crate::wiring::{resolve_config_path, resolve_project_id, resolve_repo};
use crate::{PruneOptions, RunError, RunOutcome};

use super::discriminant::DiscriminantSet;
use super::report::ReportFormat;
use super::{
    AutoFacets, DirtyTipPolicy, ResolvedHistory, Selection, detect_auto_facets,
    facet_filtered_candidates, parse_format, parse_since, parse_until, resolve_facets,
    resolve_history,
};

/// Which objects a prune pass deletes.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Scope {
    /// Delete clean and dirty runs (plus the blessings on removed clean commits).
    All,
    /// Delete only dirty (uncommitted-tree) snapshots.
    Dirty,
    /// Delete only clean runs (plus their blessing sidecars).
    Clean,
}

impl Scope {
    /// Resolves the deletion scope from the mutually exclusive `--dirty`/`--clean`
    /// switches.
    fn from_options(options: &PruneOptions) -> Result<Self, RunError> {
        match (options.dirty, options.clean) {
            (true, true) => Err(RunError::Analyze {
                message: "--dirty and --clean are mutually exclusive: --dirty removes only \
                          dirty runs, --clean only clean runs; omit both to remove every run"
                    .to_owned(),
            }),
            (true, false) => Ok(Self::Dirty),
            (false, true) => Ok(Self::Clean),
            (false, false) => Ok(Self::All),
        }
    }

    /// Whether this scope deletes clean runs (and thus durable history).
    fn touches_clean(self) -> bool {
        matches!(self, Self::All | Self::Clean)
    }

    /// Whether this scope deletes dirty runs.
    fn touches_dirty(self) -> bool {
        matches!(self, Self::All | Self::Dirty)
    }
}

/// The real `prune`: load configuration, wire the configured storage and git
/// history, and orchestrate.
pub(crate) async fn execute(
    options: &PruneOptions,
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
    let auto = detect_auto_facets().await?;

    prune_with(
        &git,
        &storage,
        &project_id,
        &config,
        options,
        &auto,
        &reporter,
    )
    .await
}

/// Storage- and git-generic `prune`: resolve the selected commit topology, choose
/// the objects to delete per the scope and filters, then either preview them
/// (`--dry-run`) or delete them.
pub(crate) async fn prune_with<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    config: &Config,
    options: &PruneOptions,
    auto: &AutoFacets,
    reporter: &dyn Reporter,
) -> Result<RunOutcome, RunError>
where
    G: GitHistory,
    S: Storage,
{
    let format = parse_format(options.format.as_deref())?;
    let since = parse_since(options.since.as_deref())?;
    let until = parse_until(options.until.as_deref())?;
    let scope = Scope::from_options(options)?;
    let selection = Selection::from_prune(options);

    // Guard the destructive clean-history scopes: refuse an un-narrowed selection
    // unless `--all` confirms it. `--dirty` only discards ephemeral data, so it is
    // exempt. A facet, a `<commit>` argument, `--since`, or `--until` all narrow
    // the range.
    let narrowed = is_narrowed(options, since.is_some(), until.is_some());
    if scope.touches_clean() && !narrowed && !options.all {
        return Err(RunError::Analyze {
            message: "prune would delete clean benchmark history across the entire selected \
                      range. Narrow the selection with a facet (--engine / --target-triple / \
                      --machine-key), a <commit> argument, --since, or --until, or pass --all \
                      to delete everything in range."
                .to_owned(),
        });
    }

    let facets = resolve_facets(&selection, Some(auto))?;
    let candidates = facet_filtered_candidates(storage, project_id, &facets, reporter).await?;

    let ResolvedHistory {
        target_ref,
        order,
        admit_dirty,
        ..
    } = resolve_history(git, config, &selection, DirtyTipPolicy::Always, reporter).await?;

    // Separate blessing sidecars from runs: a blessing is removed only when the
    // commit's clean run is removed (it references that run), so it is selected in
    // a second pass keyed off the clean removals.
    let (runs, blessings): (Vec<_>, Vec<_>) = candidates
        .into_iter()
        .partition(|(_, parsed)| !parsed.is_bless());

    let mut items: Vec<RemovalItem> = Vec::new();
    // Commits whose clean run is removed, so their blessing sidecars go too.
    let mut clean_removed: HashSet<(DiscriminantSet, String)> = HashSet::new();

    for (key, parsed) in runs {
        let Some(&index) = order.get(&parsed.commit) else {
            reporter.note(&format!(
                "skipping {key}: commit {} is not on {target_ref}'s history",
                parsed.commit
            ));
            continue;
        };
        if !commit_matches(&parsed.commit, &options.commit) {
            reporter.note(&format!(
                "skipping {key}: commit {} does not match the requested <commit> arguments",
                parsed.commit
            ));
            continue;
        }

        let kind = if parsed.is_dirty() {
            RunKind::Dirty
        } else {
            RunKind::Clean
        };

        match kind {
            RunKind::Dirty => {
                if !scope.touches_dirty() {
                    reporter.note(&format!("skipping {key}: --clean removes only clean runs"));
                    continue;
                }
                // `--dirty` reproduces `clean`: a dirty snapshot is removed only on
                // a commit whose dirty runs the matching analyze/list would admit.
                // The broader scopes delete dirty runs wherever they sit.
                if scope == Scope::Dirty
                    && !admit_dirty
                        .get(parsed.commit.as_str())
                        .copied()
                        .unwrap_or(false)
                {
                    reporter.note(&format!(
                        "skipping {key}: dirty snapshot on a base-side commit ({} admits \
                         only clean runs)",
                        parsed.commit
                    ));
                    continue;
                }
            }
            RunKind::Clean => {
                if !scope.touches_clean() {
                    reporter.note(&format!("skipping {key}: --dirty removes only dirty runs"));
                    continue;
                }
            }
            RunKind::Bless => unreachable!("blessings were partitioned out"),
        }

        // The time window requires the object's effective time, so a corrupt run
        // surfaces only under `--since`/`--until`.
        if since.is_some() || until.is_some() {
            let effective = load_effective(storage, &key).await?;
            if let Some(since) = since
                && effective < since
            {
                reporter.note(&format!(
                    "skipping {key}: effective time is before the --since cutoff"
                ));
                continue;
            }
            if let Some(until) = until
                && effective > until
            {
                reporter.note(&format!(
                    "skipping {key}: effective time is after the --until cutoff"
                ));
                continue;
            }
        }

        if kind == RunKind::Clean {
            clean_removed.insert((parsed.set.clone(), parsed.commit.clone()));
        }
        reporter.note(&format!("selected {key} for removal"));
        items.push(RemovalItem {
            index,
            set: parsed.set,
            commit: parsed.commit,
            key,
            kind,
        });
    }

    // Second pass: a blessing sidecar is removed exactly when its commit's clean
    // run is removed (the clean run's blessing has nothing left to baseline once
    // the run is gone).
    if scope.touches_clean() {
        for (key, parsed) in blessings {
            let Some(&index) = order.get(&parsed.commit) else {
                continue;
            };
            if !clean_removed.contains(&(parsed.set.clone(), parsed.commit.clone())) {
                reporter.note(&format!(
                    "skipping {key}: its clean run is not being removed"
                ));
                continue;
            }
            reporter.note(&format!("selected {key} for removal (blessing sidecar)"));
            items.push(RemovalItem {
                index,
                set: parsed.set,
                commit: parsed.commit,
                key,
                kind: RunKind::Bless,
            });
        }
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

/// Whether the selection is narrowed enough to permit a clean-history prune
/// without `--all`: any discriminant facet, an explicit `<commit>` argument, or a
/// `--since`/`--until` time bound restricts the range.
fn is_narrowed(options: &PruneOptions, since: bool, until: bool) -> bool {
    facets_present(options) || !options.commit.is_empty() || since || until
}

/// Whether any discriminant facet carries a **concrete** narrowing value. An
/// omitted facet (auto-detect) and the widening `all` keyword do not narrow.
fn facets_present(options: &PruneOptions) -> bool {
    facet_narrows(&options.engine)
        || facet_narrows(&options.target_triple)
        || facet_narrows(&options.machine_key)
}

/// Whether a repeatable facet's values include at least one concrete (non-`all`)
/// entry. An empty list (auto-detect) or an `all`-only list does not narrow.
fn facet_narrows(values: &[String]) -> bool {
    values
        .iter()
        .any(|value| !value.eq_ignore_ascii_case("all"))
}

/// Whether a commit matches the `<commit>` selection (case-insensitive prefix
/// match, so a short SHA selects the full one). An empty selection matches every
/// commit.
fn commit_matches(commit: &str, filters: &[String]) -> bool {
    if filters.is_empty() {
        return true;
    }
    let commit = commit.to_ascii_lowercase();
    filters
        .iter()
        .any(|filter| commit.starts_with(&filter.to_ascii_lowercase()))
}

/// Fetches and parses a stored run's effective timestamp for the time-window
/// filters.
async fn load_effective<S: Storage>(storage: &S, key: &str) -> Result<jiff::Timestamp, RunError> {
    let bytes = storage.get(key).await.map_err(RunError::Storage)?;
    let text = String::from_utf8(bytes).map_err(|error| RunError::Analyze {
        message: format!("stored object {key} is not valid UTF-8: {error}"),
    })?;
    let result = ResultSet::from_json(&text).map_err(|error| RunError::Analyze {
        message: format!("stored object {key} is not a valid result set: {error}"),
    })?;
    Ok(result.context.timestamps.effective)
}

/// Which kind of object a removal item names.
#[derive(Clone, Copy, Eq, PartialEq)]
enum RunKind {
    Clean,
    Dirty,
    Bless,
}

impl RunKind {
    /// Whether this kind counts as a stored run (clean or dirty) rather than a
    /// blessing sidecar.
    fn is_run(self) -> bool {
        matches!(self, Self::Clean | Self::Dirty)
    }
}

/// One object slated for removal, with its first-parent position.
#[derive(Clone)]
struct RemovalItem {
    /// First-parent position of the commit, for oldest-first ordering.
    index: usize,
    /// The discriminant set the object belongs to.
    set: DiscriminantSet,
    /// The commit the object was measured against.
    commit: String,
    /// The storage key to delete.
    key: String,
    /// Whether the object is a clean run, dirty run, or blessing sidecar.
    kind: RunKind,
}

/// One commit's objects to remove, in first-parent order.
#[derive(Clone)]
struct CommitRemoval {
    /// The commit the runs were measured against (full SHA, or a label in tests).
    commit: String,
    /// How many runs (clean or dirty) are removed on this commit.
    runs: usize,
    /// How many blessing sidecars are removed on this commit.
    blessings: usize,
    /// The storage keys removed on this commit, in storage-key order.
    keys: Vec<String>,
}

/// One discriminant set's slice of the removal plan.
#[derive(Clone)]
struct SetRemoval {
    /// The comparable partition this slice covers.
    set: DiscriminantSet,
    /// Runs removed in this set.
    runs: usize,
    /// Blessing sidecars removed in this set.
    blessings: usize,
    /// Per-commit breakdown, oldest first by git topology.
    commits: Vec<CommitRemoval>,
}

/// The fully resolved removal plan, ready to render.
#[derive(Clone)]
struct Plan {
    /// The project the runs belong to.
    project: String,
    /// The target ref the topology was resolved against.
    target_ref: String,
    /// Per-set breakdown, one entry per discriminant set with removable objects.
    sets: Vec<SetRemoval>,
    /// Total runs removed across every set.
    total_runs: usize,
    /// Total blessing sidecars removed across every set.
    total_blessings: usize,
}

/// Groups the selected objects by discriminant set and commit (ordered by
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
                        blessings: 0,
                        keys: Vec::new(),
                    });
                if item.kind.is_run() {
                    entry.runs = entry.runs.saturating_add(1);
                } else {
                    entry.blessings = entry.blessings.saturating_add(1);
                }
                entry.keys.push(item.key.clone());
            }

            SetRemoval {
                set: set.clone(),
                runs: in_set.iter().filter(|item| item.kind.is_run()).count(),
                blessings: in_set.iter().filter(|item| !item.kind.is_run()).count(),
                commits: by_commit.into_values().collect(),
            }
        })
        .collect();

    Plan {
        project: project_id.to_owned(),
        target_ref: target_ref.to_owned(),
        sets: set_removals,
        total_runs: items.iter().filter(|item| item.kind.is_run()).count(),
        total_blessings: items.iter().filter(|item| !item.kind.is_run()).count(),
    }
}

/// The past-tense or conditional verb for the plan, matching `--dry-run`.
fn verb(dry_run: bool) -> &'static str {
    if dry_run { "Would remove" } else { "Removed" }
}

/// A `" (plus N blessings)"` fragment when blessings are removed, else empty.
fn blessing_suffix(blessings: usize) -> String {
    if blessings == 0 {
        String::new()
    } else {
        format!(" (plus {})", count_noun(blessings, "blessing"))
    }
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
        "Prune plan for project {} (target {})",
        plan.project, plan.target_ref
    )];
    if plan.sets.is_empty() {
        lines.push(String::new());
        lines.push("No run matches the selection.".to_owned());
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
                "  {}{} across {}",
                count_noun(set.runs, "run"),
                blessing_suffix(set.blessings),
                count_noun(set.commits.len(), "commit")
            ));
            for commit in &set.commits {
                lines.push(format!(
                    "    {}  {}{}",
                    commit.commit,
                    count_noun(commit.runs, "run"),
                    blessing_suffix(commit.blessings)
                ));
            }
        }
        lines.push(String::new());
        lines.push(format!(
            "{} {}{} across {}",
            verb(dry_run),
            count_noun(plan.total_runs, "run"),
            blessing_suffix(plan.total_blessings),
            count_noun(plan.sets.len(), "discriminant set")
        ));
    }
    format!("{}\n", lines.join("\n"))
}

fn render_plan_markdown(plan: &Plan, dry_run: bool) -> String {
    let mut lines = vec![format!(
        "# Prune plan for {} (target {})",
        plan.project, plan.target_ref
    )];
    if plan.sets.is_empty() {
        lines.push(String::new());
        lines.push("No run matches the selection.".to_owned());
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
                "{}{} across {}",
                count_noun(set.runs, "run"),
                blessing_suffix(set.blessings),
                count_noun(set.commits.len(), "commit")
            ));
            lines.push(String::new());
            lines.push("| Commit | Runs | Blessings |".to_owned());
            lines.push("| --- | --- | --- |".to_owned());
            for commit in &set.commits {
                lines.push(format!(
                    "| {} | {} | {} |",
                    commit.commit, commit.runs, commit.blessings
                ));
            }
        }
        lines.push(String::new());
        lines.push(format!(
            "**{} {}{} across {}**",
            verb(dry_run),
            count_noun(plan.total_runs, "run"),
            blessing_suffix(plan.total_blessings),
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
        blessings: usize,
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
        blessings: usize,
        commits: Vec<JsonCommit<'a>>,
    }
    #[derive(Serialize)]
    struct JsonTotals {
        runs: usize,
        blessings: usize,
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
            blessings: set.blessings,
            commits: set
                .commits
                .iter()
                .map(|commit| JsonCommit {
                    commit: &commit.commit,
                    runs: commit.runs,
                    blessings: commit.blessings,
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
            blessings: plan.total_blessings,
            discriminant_sets: plan.sets.len(),
        },
    };
    serde_json::to_string_pretty(&document).expect("prune plan structures always serialize to JSON")
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

    /// The auto-detected facets for the default synthetic partition the tests seed.
    fn auto() -> AutoFacets {
        AutoFacets {
            triple: "x86_64-unknown-linux-gnu".to_owned(),
            machine_key: "synthetic".to_owned(),
        }
    }

    /// Default options select the `--dirty` scope, which is exempt from the
    /// narrowing guard, so most fixtures need no extra flags.
    fn dirty_options() -> PruneOptions {
        PruneOptions {
            dirty: true,
            ..PruneOptions::default()
        }
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

    fn bless_key(commit: &str, unix: i64) -> String {
        format!("v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/bless-{unix}.json")
    }

    fn store(storage: &MemoryStorage, key: &str, value: &ResultSet) {
        let json = value.to_json().expect("result set serializes");
        block_on(storage.put(key, json.as_bytes())).expect("store succeeds");
    }

    /// Stores a blessing sidecar. `prune` never parses these (it only deletes
    /// them), so any well-formed bytes under a `bless-` key suffice.
    fn store_bless(storage: &MemoryStorage, key: &str) {
        block_on(storage.put(key, b"{}")).expect("store succeeds");
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

    /// Drives `prune_with` and unwraps the rendered message.
    fn prune(storage: &MemoryStorage, git: &FakeGitHistory, options: &PruneOptions) -> String {
        let outcome = block_on(prune_with(
            git,
            storage,
            "folo",
            &config(),
            options,
            &auto(),
            &RecordingReporter::new(),
        ))
        .expect("prune runs");
        match outcome {
            RunOutcome::Completed { message } => message,
            RunOutcome::Analyzed { .. } => panic!("prune returns a Completed outcome"),
        }
    }

    #[test]
    fn dirty_scope_removes_target_side_dirty_runs_on_a_feature_branch() {
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

        let opts = PruneOptions {
            format: Some("json".to_owned()),
            ..dirty_options()
        };
        let report = prune(&storage, &git, &opts);
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
    fn dirty_scope_removes_the_base_tip_dirty_unconditionally() {
        // On the base branch with a CLEAN working tree, the base-branch tip's
        // dirty runs are still removed (no working-tree-dirty guard). Earlier
        // base-side dirty runs stay untouched.
        let storage = MemoryStorage::new();
        for commit in ["c0", "c1", "c2", "c3"] {
            store(&storage, &clean_key(commit), &set(0, commit));
        }
        store(&storage, &dirty_key("c1", 150), &set(150, "c1"));
        store(&storage, &dirty_key("c3", 300), &set(300, "c3"));
        let git = linear_git(); // No mark_dirty: the working tree is clean.

        let report = prune(&storage, &git, &dirty_options());
        assert!(report.contains("Removed 1 run"), "{report}");

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
    fn default_scope_removes_clean_dirty_and_blessings_for_a_commit() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("f1"), &set(0, "f1"));
        store(&storage, &dirty_key("f1", 200), &set(200, "f1"));
        store_bless(&storage, &bless_key("f1", 50));
        // A second commit's data must survive a commit-scoped prune.
        store(&storage, &clean_key("f2"), &set(0, "f2"));
        store_bless(&storage, &bless_key("f2", 60));
        let git = feature_git();

        let opts = PruneOptions {
            commit: vec!["f1".to_owned()],
            format: Some("json".to_owned()),
            ..PruneOptions::default()
        };
        let report = prune(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["totals"]["runs"], 2, "clean + dirty: {report}");
        assert_eq!(parsed["totals"]["blessings"], 1, "{report}");

        let remaining = keys(&storage);
        assert!(!remaining.contains(&clean_key("f1")), "f1 clean removed");
        assert!(
            !remaining.contains(&dirty_key("f1", 200)),
            "f1 dirty removed"
        );
        assert!(
            !remaining.contains(&bless_key("f1", 50)),
            "f1 blessing removed"
        );
        assert!(remaining.contains(&clean_key("f2")), "f2 clean survives");
        assert!(
            remaining.contains(&bless_key("f2", 60)),
            "f2 blessing survives"
        );
    }

    #[test]
    fn clean_scope_removes_clean_and_blessings_but_keeps_dirty() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("f1"), &set(0, "f1"));
        store(&storage, &dirty_key("f1", 200), &set(200, "f1"));
        store_bless(&storage, &bless_key("f1", 50));
        let git = feature_git();

        let opts = PruneOptions {
            clean: true,
            commit: vec!["f1".to_owned()],
            ..PruneOptions::default()
        };
        prune(&storage, &git, &opts);

        let remaining = keys(&storage);
        assert!(!remaining.contains(&clean_key("f1")), "clean removed");
        assert!(
            !remaining.contains(&bless_key("f1", 50)),
            "blessing removed with its clean run"
        );
        assert!(
            remaining.contains(&dirty_key("f1", 200)),
            "--clean leaves dirty runs"
        );
    }

    #[test]
    fn unnarrowed_clean_deletion_is_refused_without_all() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("f1"), &set(0, "f1"));
        let git = feature_git();

        let error = block_on(prune_with(
            &git,
            &storage,
            "folo",
            &config(),
            &PruneOptions::default(),
            &auto(),
            &RecordingReporter::new(),
        ))
        .unwrap_err();
        match error {
            RunError::Analyze { message } => {
                assert!(message.contains("--all"), "{message}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
        // Nothing was deleted.
        assert!(keys(&storage).contains(&clean_key("f1")));
    }

    #[test]
    fn all_flag_overrides_the_narrowing_guard() {
        let storage = MemoryStorage::new();
        for commit in ["c0", "c1", "f1", "f2"] {
            store(&storage, &clean_key(commit), &set(0, commit));
        }
        let git = feature_git();

        let opts = PruneOptions {
            all: true,
            ..PruneOptions::default()
        };
        prune(&storage, &git, &opts);
        assert!(keys(&storage).is_empty(), "--all removes every clean run");
    }

    #[test]
    fn dirty_and_clean_are_mutually_exclusive() {
        let storage = MemoryStorage::new();
        let git = feature_git();
        let opts = PruneOptions {
            dirty: true,
            clean: true,
            ..PruneOptions::default()
        };
        let error = block_on(prune_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            &auto(),
            &RecordingReporter::new(),
        ))
        .unwrap_err();
        match error {
            RunError::Analyze { message } => {
                assert!(message.contains("mutually exclusive"), "{message}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
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
        let opts = PruneOptions {
            dry_run: true,
            format: Some("json".to_owned()),
            ..dirty_options()
        };
        let report = prune(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(parsed["totals"]["runs"], 1, "{report}");

        // Nothing was actually deleted.
        assert_eq!(keys(&storage), before, "--dry-run deletes nothing");
    }

    #[test]
    fn text_format_reports_would_remove_under_dry_run() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &set(0, "c0"));
        store(&storage, &clean_key("f1"), &set(0, "f1"));
        store(&storage, &dirty_key("f1", 200), &set(200, "f1"));
        let git = feature_git();

        let opts = PruneOptions {
            dry_run: true,
            ..dirty_options()
        };
        let report = prune(&storage, &git, &opts);
        assert!(report.contains("Prune plan for project folo"), "{report}");
        assert!(report.contains("Would remove 1 run"), "{report}");
        assert!(report.contains("1 discriminant set"), "{report}");
    }

    #[test]
    fn dirty_scope_never_touches_clean_runs() {
        let storage = MemoryStorage::new();
        for commit in ["c0", "c1", "f1", "f2"] {
            store(&storage, &clean_key(commit), &set(0, commit));
        }
        let git = feature_git();

        let report = prune(&storage, &git, &dirty_options());
        assert!(report.contains("No run matches the selection."), "{report}");
        // Every clean run survives.
        assert_eq!(keys(&storage).len(), 4, "clean runs are untouched");
    }

    #[test]
    fn prune_requires_a_repository() {
        let storage = MemoryStorage::new();
        store(&storage, &dirty_key("c0", 100), &set(100, "c0"));
        let git = FakeGitHistory::new(); // No commits: HEAD does not resolve.
        let error = block_on(prune_with(
            &git,
            &storage,
            "folo",
            &config(),
            &dirty_options(),
            &auto(),
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
    fn engine_facet_restricts_removal() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("f1"), &set(0, "f1"));
        store(&storage, &dirty_key("f1", 200), &set(200, "f1"));
        // A criterion dirty run on the same commit must survive an engine-scoped
        // callgrind prune.
        let criterion_dirty =
            "v2/folo/criterion/x86_64-unknown-linux-gnu/synthetic/f1/dirty-200.json";
        store(&storage, criterion_dirty, &set(200, "f1"));
        let git = feature_git();

        let opts = PruneOptions {
            engine: vec!["callgrind".to_owned()],
            ..dirty_options()
        };
        prune(&storage, &git, &opts);

        let remaining = keys(&storage);
        assert!(
            !remaining.contains(&dirty_key("f1", 200)),
            "callgrind dirty removed"
        );
        assert!(
            remaining.contains(&criterion_dirty.to_owned()),
            "criterion dirty survives the engine-scoped prune"
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

        let opts = PruneOptions {
            since: Some("1970-01-01T00:03:00Z".to_owned()), // 180s
            ..dirty_options()
        };
        prune(&storage, &git, &opts);

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
    fn until_only_removes_runs_on_or_before_the_cutoff() {
        let storage = MemoryStorage::new();
        store(&storage, &dirty_key("f1", 100), &set(100, "f1"));
        store(&storage, &dirty_key("f1", 180), &set(180, "f1"));
        store(&storage, &dirty_key("f2", 300), &set(300, "f2"));
        let git = feature_git();

        let opts = PruneOptions {
            until: Some("1970-01-01T00:03:00Z".to_owned()), // 180s
            ..dirty_options()
        };
        prune(&storage, &git, &opts);

        let remaining = keys(&storage);
        assert!(
            !remaining.contains(&dirty_key("f1", 100)),
            "a run before --until is removed"
        );
        assert!(
            !remaining.contains(&dirty_key("f1", 180)),
            "a run exactly at --until is removed (inclusive cutoff)"
        );
        assert!(
            remaining.contains(&dirty_key("f2", 300)),
            "a run after --until is kept"
        );
    }

    #[test]
    fn commit_filter_restricts_removal_to_the_named_commit() {
        let storage = MemoryStorage::new();
        store(&storage, &dirty_key("f1", 200), &set(200, "f1"));
        store(&storage, &dirty_key("f2", 300), &set(300, "f2"));
        let git = feature_git();

        let opts = PruneOptions {
            commit: vec!["f2".to_owned()],
            ..dirty_options()
        };
        prune(&storage, &git, &opts);

        let remaining = keys(&storage);
        assert!(
            remaining.contains(&dirty_key("f1", 200)),
            "an unselected commit's run survives"
        );
        assert!(
            !remaining.contains(&dirty_key("f2", 300)),
            "the selected commit's run is removed"
        );
    }

    #[test]
    fn commits_are_listed_oldest_first_by_topology() {
        let storage = MemoryStorage::new();
        store(&storage, &dirty_key("f2", 300), &set(300, "f2"));
        store(&storage, &dirty_key("f1", 200), &set(200, "f1"));
        let git = feature_git();

        let opts = PruneOptions {
            dry_run: true,
            format: Some("json".to_owned()),
            ..dirty_options()
        };
        let report = prune(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        let commits = parsed["sets"][0]["commits"].as_array().unwrap();
        assert_eq!(commits[0]["commit"], "f1", "oldest first: {report}");
        assert_eq!(commits[1]["commit"], "f2");
    }

    #[test]
    fn markdown_format_renders_a_table_per_set() {
        let storage = MemoryStorage::new();
        store(&storage, &dirty_key("f1", 200), &set(200, "f1"));
        store(&storage, &dirty_key("f2", 300), &set(300, "f2"));
        let git = feature_git();

        let opts = PruneOptions {
            dry_run: true,
            format: Some("markdown".to_owned()),
            ..dirty_options()
        };
        let report = prune(&storage, &git, &opts);
        assert!(report.contains("# Prune plan for"), "{report}");
        assert!(report.contains("| Commit | Runs | Blessings |"), "{report}");
        assert!(report.contains("| f1 |"), "{report}");
        assert!(report.contains("**Would remove"), "{report}");
    }

    #[test]
    fn is_narrowed_accepts_any_single_narrowing_predicate() {
        // A facet alone narrows the range.
        assert!(is_narrowed(
            &PruneOptions {
                engine: vec!["callgrind".to_owned()],
                ..PruneOptions::default()
            },
            false,
            false,
        ));
        // An explicit `<commit>` argument alone narrows it.
        assert!(is_narrowed(
            &PruneOptions {
                commit: vec!["abc".to_owned()],
                ..PruneOptions::default()
            },
            false,
            false,
        ));
        // `--since` alone narrows it.
        assert!(is_narrowed(&PruneOptions::default(), true, false));
        // `--until` alone narrows it (the guard against deleting the whole tail).
        assert!(is_narrowed(&PruneOptions::default(), false, true));
    }

    #[test]
    fn is_narrowed_rejects_an_unconstrained_selection() {
        assert!(!is_narrowed(&PruneOptions::default(), false, false));
    }

    #[test]
    fn facets_present_detects_each_facet_independently() {
        assert!(!facets_present(&PruneOptions::default()));
        assert!(facets_present(&PruneOptions {
            engine: vec!["callgrind".to_owned()],
            ..PruneOptions::default()
        }));
        assert!(facets_present(&PruneOptions {
            target_triple: vec!["x86_64-unknown-linux-gnu".to_owned()],
            ..PruneOptions::default()
        }));
        assert!(facets_present(&PruneOptions {
            machine_key: vec!["m1".to_owned()],
            ..PruneOptions::default()
        }));
        // The widening `all` keyword is not a concrete narrowing value.
        assert!(!facets_present(&PruneOptions {
            engine: vec!["all".to_owned()],
            ..PruneOptions::default()
        }));
    }

    #[test]
    fn blessing_suffix_is_empty_only_when_no_blessings_are_removed() {
        assert_eq!(blessing_suffix(0), "");
        assert_eq!(blessing_suffix(1), " (plus 1 blessing)");
        assert_eq!(blessing_suffix(3), " (plus 3 blessings)");
    }

    #[test]
    fn build_plan_counts_runs_and_blessings_separately() {
        let set = DiscriminantSet {
            engine: "callgrind".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            machine: "synthetic".to_owned(),
        };
        // Two clean runs (c0, c1) plus one blessing on c0: an asymmetric mix so a
        // run/blessing miscount diverges from the truth.
        let items = vec![
            RemovalItem {
                index: 0,
                set: set.clone(),
                commit: "c0".to_owned(),
                key: clean_key("c0"),
                kind: RunKind::Clean,
            },
            RemovalItem {
                index: 1,
                set: set.clone(),
                commit: "c1".to_owned(),
                key: clean_key("c1"),
                kind: RunKind::Clean,
            },
            RemovalItem {
                index: 0,
                set,
                commit: "c0".to_owned(),
                key: bless_key("c0", 10),
                kind: RunKind::Bless,
            },
        ];

        let plan = build_plan("folo", "master", &items);
        assert_eq!(plan.total_runs, 2, "two clean runs");
        assert_eq!(plan.total_blessings, 1, "one blessing sidecar");
        assert_eq!(plan.sets.len(), 1);
        assert_eq!(plan.sets[0].runs, 2);
        assert_eq!(plan.sets[0].blessings, 1);

        let commits = &plan.sets[0].commits;
        assert_eq!(commits.len(), 2, "oldest-first by first-parent index");
        assert_eq!(commits[0].commit, "c0");
        assert_eq!(commits[0].runs, 1, "c0 has one clean run");
        assert_eq!(commits[0].blessings, 1, "c0 has one blessing");
        assert_eq!(commits[1].commit, "c1");
        assert_eq!(commits[1].runs, 1);
        assert_eq!(commits[1].blessings, 0, "c1 has no blessing");
    }
}
