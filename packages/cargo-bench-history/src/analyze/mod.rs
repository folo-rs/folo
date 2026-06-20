//! The `analyze` command: resolve which stored runs make up each benchmark's
//! history from live git topology, reconstruct the series, and report the notable
//! changes.
//!
//! Unlike a snapshot tool, `analyze` orders a series by *git history* rather than
//! by ingest time (see DESIGN §8.4): it resolves the target ref's first-parent
//! ancestry, splits it at the merge-base with a base branch, and admits dirty
//! (uncommitted-tree) snapshots only on the target side of that split. The pure
//! logic (selection, series reconstruction, finding detection, report rendering)
//! stays sync and Miri-safe; only the git queries and object loads touch async
//! ports. [`execute`] wires the real adapters; [`analyze_with`] is the
//! storage- and git-generic orchestrator the in-memory tests drive.

pub(crate) mod clean;
mod discriminant;
mod findings;
pub(crate) mod list;
mod report;
mod selection;
mod series;
mod stats;

use std::collections::HashMap;

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};

use crate::comparability::{EngineSystem, sanitize_segment};
use crate::config::{Config, load_config};
use crate::git_history::{GitHistory, SystemGitHistory};
use crate::model::ResultSet;
use crate::report::{Reporter, StderrReporter};
use crate::storage::{Storage, build_storage};
use crate::text::count_noun;
use crate::wiring::{default_config_path, resolve_project_id};
use crate::{AnalyzeOptions, CleanOptions, ListOptions, RunError, RunOutcome};

use discriminant::{DiscriminantSet, Facets, ParsedKey, parse_key};
use findings::{AnalysisConfig, AnalysisContext, AnalysisMode, find_changes};
use report::{ReportFormat, ReportInput, SetSummary, render};
use selection::select_commits;
use series::{LoadedObject, SeriesFilter, build_series};

/// The real `analyze`: load configuration, wire the configured storage and git
/// history, and orchestrate.
///
/// `now_override` anchors the history-mode default `--since` lookback to a fixed
/// instant; production passes `None` so the anchor is the wall clock, while tests
/// pass a deterministic instant (the relative-`--since` parsing keeps using the
/// real clock regardless — only the *default* lookback uses this anchor).
pub(crate) async fn execute(
    options: &AnalyzeOptions,
    now_override: Option<Timestamp>,
) -> Result<RunOutcome, RunError> {
    let reporter = StderrReporter::new(options.verbose);

    let config_path = options
        .config_path
        .clone()
        .unwrap_or_else(default_config_path);
    reporter.note(&format!(
        "loading configuration from {}",
        config_path.display()
    ));
    let config = load_config(&config_path).await?;

    let workspace_dir = std::env::current_dir().map_err(RunError::Io)?;
    let project_id = resolve_project_id(&config, &workspace_dir);
    let storage = build_storage(&config)?;

    let repo = options.repo.clone().unwrap_or(workspace_dir);
    let git = SystemGitHistory::new(repo);

    let now = now_override.unwrap_or_else(Timestamp::now);
    analyze_with(
        &git,
        &storage,
        &project_id,
        &config,
        options,
        now,
        &reporter,
    )
    .await
}

/// Storage- and git-generic `analyze`: facet-filter the stored objects, resolve
/// the git topology, select the comparable commits, build the series, detect
/// changes, and render a report for the requested format.
pub(crate) async fn analyze_with<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    config: &Config,
    options: &AnalyzeOptions,
    now: Timestamp,
    reporter: &dyn Reporter,
) -> Result<RunOutcome, RunError>
where
    G: GitHistory,
    S: Storage,
{
    let format = parse_format(options.format.as_deref())?;
    let selection = Selection::from_analyze(options)?;
    let dataset =
        select_dataset(git, storage, project_id, config, &selection, now, reporter).await?;

    let filter = SeriesFilter {
        metric: options.metric.as_deref(),
    };
    let series = build_series(&dataset.loaded, &dataset.order, &filter);
    let context = AnalysisContext {
        mode: dataset.mode,
        config: AnalysisConfig::default(),
        merge_base_index: dataset.merge_base_index,
        include_improvements: options.include_improvements,
    };
    let findings = find_changes(&series, &context);
    let regressions = findings
        .iter()
        .filter(|finding| finding.is_regression())
        .count();
    let notable = !findings.is_empty();

    // Break the report down by comparable set so each partition reads on its own.
    let mut sets: Vec<DiscriminantSet> = series.iter().map(|one| one.set.clone()).collect();
    sets.sort();
    sets.dedup();
    let summaries: Vec<SetSummary<'_>> = sets
        .iter()
        .map(|set| SetSummary {
            set,
            runs: dataset
                .loaded
                .iter()
                .filter(|object| &object.key.set == set)
                .count(),
            series: series.iter().filter(|one| &one.set == set).count(),
            findings: findings
                .iter()
                .filter(|finding| &finding.set == set)
                .collect(),
        })
        .collect();

    // When stored runs existed but none entered the analysis, the empty outcome is
    // otherwise indistinguishable from "no data". Explain the dominant reasons so
    // the user can act without resorting to `--verbose`.
    let hint = empty_history_hint(
        dataset.loaded.is_empty(),
        dataset.candidate_count,
        &dataset.target_ref,
        dataset.tally,
    );

    // Admitting a dirty snapshot on the base branch's tip is a courtesy for the
    // "evaluating the tool" / "accidentally working on the base branch" cases; warn
    // that such data is not persisted across commits.
    let warning = dataset
        .included_dirty_base_exception
        .then(dirty_base_exception_warning);

    let input = ReportInput {
        project: project_id,
        mode: dataset.mode.as_str(),
        notable,
        runs: dataset.loaded.len(),
        series: series.len(),
        findings: &findings,
        sets: &summaries,
        hint: hint.as_deref(),
        warning: warning.as_deref(),
    };
    let report = render(&input, format);

    Ok(RunOutcome::Analyzed {
        report,
        regressions,
    })
}

/// The data-set selection parameters shared by `analyze` and `list`: which stored
/// objects to consider (facets + `--since`) and how to resolve the git timeline
/// (`--repo` is resolved by the caller into the [`GitHistory`] adapter; `--branch` /
/// `--base` / `--no-dirty` steer the topology query). `--metric` is deliberately
/// *not* here: it filters which series are built, not which runs load.
struct Selection<'a> {
    branch: Option<&'a str>,
    base: Option<&'a str>,
    no_dirty: bool,
    since: Option<&'a str>,
    engine: Option<&'a str>,
    target_triple: Option<&'a str>,
    os: Option<&'a str>,
    architecture: Option<&'a str>,
    machine_key: Option<&'a str>,
    /// Explicit `--mode` override; `None` lets the mode auto-detect from topology.
    mode_override: Option<AnalysisMode>,
}

impl<'a> Selection<'a> {
    fn from_analyze(options: &'a AnalyzeOptions) -> Result<Self, RunError> {
        Ok(Self {
            branch: options.branch.as_deref(),
            base: options.base.as_deref(),
            no_dirty: options.no_dirty,
            since: options.since.as_deref(),
            engine: options.engine.as_deref(),
            target_triple: options.target_triple.as_deref(),
            os: options.os.as_deref(),
            architecture: options.architecture.as_deref(),
            machine_key: options.machine_key.as_deref(),
            mode_override: parse_mode(options.mode.as_deref())?,
        })
    }

    fn from_list(options: &'a ListOptions) -> Self {
        Self {
            branch: options.branch.as_deref(),
            base: options.base.as_deref(),
            no_dirty: options.no_dirty,
            since: options.since.as_deref(),
            engine: options.engine.as_deref(),
            target_triple: options.target_triple.as_deref(),
            os: options.os.as_deref(),
            architecture: options.architecture.as_deref(),
            machine_key: options.machine_key.as_deref(),
            mode_override: None,
        }
    }

    fn from_clean(options: &'a CleanOptions) -> Self {
        Self {
            branch: options.branch.as_deref(),
            base: options.base.as_deref(),
            // `clean` only ever touches dirty runs, so dirty admission is always
            // on; the base-tip exception is applied unconditionally (see
            // `DirtyTipPolicy::Always`).
            no_dirty: false,
            since: options.since.as_deref(),
            engine: options.engine.as_deref(),
            target_triple: options.target_triple.as_deref(),
            os: options.os.as_deref(),
            architecture: options.architecture.as_deref(),
            machine_key: options.machine_key.as_deref(),
            mode_override: None,
        }
    }
}

/// The objects an analysis (or listing) draws on, plus the bookkeeping needed to
/// explain an empty outcome and warn about ephemeral data.
struct SelectedDataSet {
    /// The in-selection objects, loaded and parsed, in storage-key order.
    loaded: Vec<LoadedObject>,
    /// First-parent position of each selected commit, for series ordering.
    order: HashMap<String, usize>,
    /// How many facet-matching candidates existed before topology filtering.
    candidate_count: usize,
    /// Why candidates were excluded, for the empty-history hint.
    tally: ExclusionTally,
    /// Whether a dirty run was admitted solely by the base-branch dirty-tree
    /// exception (triggers the ephemeral-data warning).
    included_dirty_base_exception: bool,
    /// The target ref the timeline was resolved against (for diagnostics).
    target_ref: String,
    /// The resolved analysis mode (auto-detected from topology, or overridden).
    mode: AnalysisMode,
    /// First-parent topological index of the merge-base, used by branch mode to
    /// split base-side history from the branch's own commits.
    merge_base_index: Option<usize>,
}

/// Parses the `--engine` facet and validates the triple/os/arch exclusivity, then
/// assembles the [`Facets`] borrow used to filter stored discriminant sets.
fn parsed_facets<'a>(
    selection: &Selection<'a>,
) -> Result<(Option<EngineSystem>, Facets<'a>), RunError> {
    let engine = parse_engine(selection.engine)?;
    validate_triple_exclusivity(
        selection.target_triple,
        selection.os,
        selection.architecture,
    )?;
    let facets = Facets {
        engine: engine.map(EngineSystem::as_str),
        target_triple: selection.target_triple,
        os: selection.os,
        architecture: selection.architecture,
        machine_key: selection.machine_key,
    };
    Ok((engine, facets))
}

/// Lists the stored objects under the project's partition and keeps the ones whose
/// discriminant set matches the facet filters. Shared by the topology-aware
/// selection and the discriminant listing (which needs no repository).
async fn facet_filtered_candidates<S: Storage>(
    storage: &S,
    project_id: &str,
    engine: Option<EngineSystem>,
    facets: &Facets<'_>,
    reporter: &dyn Reporter,
) -> Result<Vec<(String, ParsedKey)>, RunError> {
    // The listing prefix must use the same sanitized project segment that
    // `ComparabilityKey` writes its storage keys under. A project id containing a
    // character that sanitizes (a space, `/`, a non-ASCII letter, ...) is stored
    // mangled, so listing under the raw id would silently find an empty history.
    let project = sanitize_segment(project_id);
    let prefix = match engine {
        Some(engine) => format!("v2/{project}/{}/", engine.as_str()),
        None => format!("v2/{project}/"),
    };

    reporter.note(&format!(
        "project id: {project_id} (storage segment: {project})"
    ));
    reporter.note(&format!("listing stored objects under prefix {prefix}"));
    if reporter.enabled() {
        reporter.note(&format!("facet filters: {}", describe_facets(facets)));
    }

    let keys = storage.list(&prefix).await.map_err(RunError::Storage)?;
    reporter.note(&format!(
        "storage returned {}",
        count_noun(keys.len(), "object key")
    ));

    let mut candidates: Vec<(String, ParsedKey)> = Vec::new();
    for key in keys {
        if !key.ends_with(".json") {
            reporter.note(&format!("skipping {key}: not a .json object"));
            continue;
        }
        let Some(parsed) = parse_key(&key) else {
            reporter.note(&format!("skipping {key}: not a recognized v2 storage key"));
            continue;
        };
        if !parsed.set.matches(facets) {
            reporter.note(&format!(
                "skipping {key}: discriminant {} does not match the facet filters",
                parsed.set
            ));
            continue;
        }
        candidates.push((key, parsed));
    }
    reporter.note(&format!(
        "{} match the facet filters",
        count_noun(candidates.len(), "object")
    ));
    Ok(candidates)
}

/// Resolves the git topology, selects the comparable commits, and loads the
/// in-selection objects into a [`SelectedDataSet`]. Requires a repository: the
/// timeline is reconstructed from git history, not from stored timestamps.
async fn select_dataset<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    config: &Config,
    selection: &Selection<'_>,
    now: Timestamp,
    reporter: &dyn Reporter,
) -> Result<SelectedDataSet, RunError>
where
    G: GitHistory,
    S: Storage,
{
    let (engine, facets) = parsed_facets(selection)?;
    let candidates =
        facet_filtered_candidates(storage, project_id, engine, &facets, reporter).await?;

    let ResolvedHistory {
        target_ref,
        order,
        admit_dirty,
        dirty_base_exception,
        merge_base_index,
        tip_is_merge_base,
        dirty_tip_admitted,
    } = resolve_history(
        git,
        config,
        selection,
        DirtyTipPolicy::WhenWorkingTreeDirty,
        reporter,
    )
    .await?;

    // The mode steers the analysis and the default `--since`: an official base
    // branch (its tip is its own merge-base, working tree clean) is `history`;
    // commits — or a dirty tree — on top of the base make it `branch`. An explicit
    // `--mode` overrides the auto-detection.
    let mode = selection
        .mode_override
        .unwrap_or_else(|| auto_mode(tip_is_merge_base, dirty_tip_admitted));
    let since = resolve_since(selection.since, mode, now)?;
    reporter.note(&format!(
        "analysis mode: {} (since cutoff: {})",
        mode.as_str(),
        since.map_or_else(|| "none".to_owned(), |since| since.to_string())
    ));

    // Tally why candidates do not enter the analysis, so a `0 runs` outcome can
    // explain itself (via `--verbose` per object, and via a summary hint when
    // candidates existed but none were admitted).
    let candidate_count = candidates.len();
    let mut excluded_outside_history = 0_usize;
    let mut excluded_dirty_base = 0_usize;
    let mut excluded_since = 0_usize;
    // Whether at least one dirty run was admitted solely by the base-branch
    // dirty-tree exception, so the report can warn that it is ephemeral.
    let mut included_dirty_base_exception = false;

    // Load each in-selection object, admitting a dirty snapshot only on a commit
    // whose side of the merge-base allows it, then apply the `--since` window.
    let mut loaded: Vec<LoadedObject> = Vec::new();
    for (key, parsed) in candidates {
        if !order.contains_key(&parsed.commit) {
            excluded_outside_history = excluded_outside_history.saturating_add(1);
            reporter.note(&format!(
                "excluding {key}: commit {} is not on {target_ref}'s analyzed history",
                parsed.commit
            ));
            continue;
        }
        if parsed.is_dirty()
            && !admit_dirty
                .get(parsed.commit.as_str())
                .copied()
                .unwrap_or(false)
        {
            excluded_dirty_base = excluded_dirty_base.saturating_add(1);
            reporter.note(&format!(
                "excluding {key}: dirty snapshot on a base-side commit ({} \
                 only admits clean runs); dirty runs count only on the target side",
                parsed.commit
            ));
            continue;
        }
        let bytes = storage.get(&key).await.map_err(RunError::Storage)?;
        let text = String::from_utf8(bytes).map_err(|error| RunError::Analyze {
            message: format!("stored object {key} is not valid UTF-8: {error}"),
        })?;
        let result = ResultSet::from_json(&text).map_err(|error| RunError::Analyze {
            message: format!("stored object {key} is not a valid result set: {error}"),
        })?;
        if since.is_some_and(|since| result.context.timestamps.effective < since) {
            excluded_since = excluded_since.saturating_add(1);
            reporter.note(&format!(
                "excluding {key}: effective time is before the --since cutoff"
            ));
            continue;
        }
        if parsed.is_dirty()
            && dirty_base_exception
                .get(parsed.commit.as_str())
                .copied()
                .unwrap_or(false)
        {
            included_dirty_base_exception = true;
            reporter.note(&format!(
                "including {key}: dirty snapshot on the base-branch tip, admitted \
                 because the working tree is dirty (ephemeral — see the warning)"
            ));
        } else {
            reporter.note(&format!("including {key}"));
        }
        loaded.push(LoadedObject {
            key: parsed,
            object_key: key,
            result,
        });
    }
    reporter.note(&format!(
        "{} entered the analysis ({excluded_outside_history} outside history, \
         {excluded_dirty_base} dirty-on-base, {excluded_since} before --since)",
        count_noun(loaded.len(), "object")
    ));

    Ok(SelectedDataSet {
        loaded,
        order,
        candidate_count,
        tally: ExclusionTally {
            outside_history: excluded_outside_history,
            dirty_base: excluded_dirty_base,
            since: excluded_since,
        },
        included_dirty_base_exception,
        target_ref,
        mode,
        merge_base_index,
    })
}

/// Auto-detects the analysis mode from the resolved topology.
///
/// An official base-branch view — the target's tip *is* its own merge-base (or no
/// base is known) and the working tree is clean — is [`AnalysisMode::History`].
/// Anything else (commits past the merge-base, or a dirty tree admitting in-flight
/// snapshots on top of the base) is treated as an unnamed feature branch:
/// [`AnalysisMode::Branch`]. [`AnalysisMode::Tip`] is never auto-selected.
fn auto_mode(tip_is_merge_base: bool, dirty_tip_admitted: bool) -> AnalysisMode {
    if tip_is_merge_base && !dirty_tip_admitted {
        AnalysisMode::History
    } else {
        AnalysisMode::Branch
    }
}

/// Resolves the effective `--since` cutoff: an explicit value always wins;
/// otherwise history mode applies a default look-back so a scheduled trend watch
/// does not silently widen as history accumulates, while branch and tip modes have
/// no default (a feature branch's whole history is in scope).
fn resolve_since(
    value: Option<&str>,
    mode: AnalysisMode,
    now: Timestamp,
) -> Result<Option<Timestamp>, RunError> {
    if value.is_some() {
        return parse_since(value);
    }
    if mode == AnalysisMode::History {
        return default_history_since(now).map(Some);
    }
    Ok(None)
}

/// Default history-mode look-back window: six months before `now`.
const HISTORY_DEFAULT_LOOKBACK_MONTHS: i32 = 6;

/// The instant [`HISTORY_DEFAULT_LOOKBACK_MONTHS`] before `now`, anchored with
/// calendar-correct zoned arithmetic (months have no fixed length).
fn default_history_since(now: Timestamp) -> Result<Timestamp, RunError> {
    now.to_zoned(TimeZone::UTC)
        .checked_sub(Span::new().months(HISTORY_DEFAULT_LOOKBACK_MONTHS))
        .map(|zoned| zoned.timestamp())
        .map_err(|error| RunError::Analyze {
            message: format!("default --since window is out of the representable range: {error}"),
        })
}

/// Parses the `--mode` option into an explicit [`AnalysisMode`] override.
///
/// `auto` (the default when omitted) resolves to `None` so the mode is detected
/// from topology; `history`, `branch`, and `tip` force that mode.
fn parse_mode(value: Option<&str>) -> Result<Option<AnalysisMode>, RunError> {
    match value {
        None | Some("auto") => Ok(None),
        Some(name) => AnalysisMode::from_name(name)
            .map(Some)
            .ok_or_else(|| RunError::Analyze {
                message: format!(
                    "unknown analysis mode {name:?}; expected auto, history, branch, or tip"
                ),
            }),
    }
}

/// How the base-branch dirty-tip exception is gated.
///
/// On a feature branch the target-side commits admit dirty runs unconditionally;
/// this policy only governs the *base* branch's tip. `analyze`/`list` admit a
/// base-tip dirty run only when the working tree is currently dirty (the
/// "evaluating the tool / accidentally on the base branch" case); `clean` removes
/// base-tip dirty runs regardless of the current working-tree state.
#[derive(Clone, Copy)]
enum DirtyTipPolicy {
    /// Admit a base-side tip's dirty runs only when the working tree is dirty now.
    WhenWorkingTreeDirty,
    /// Always treat a base-side tip as admitting dirty runs.
    Always,
}

/// The git topology a selection resolves to: the target ref it was resolved
/// against, the first-parent position of each selected commit, and the per-commit
/// dirty-admission flags. All maps use owned commit SHAs so the borrowed
/// `selected` set can drop before the caller's load loop.
struct ResolvedHistory {
    /// The target ref the timeline was resolved against (for diagnostics).
    target_ref: String,
    /// First-parent position of each selected commit, for series ordering. An
    /// object whose commit is absent is outside the analyzed history.
    order: HashMap<String, usize>,
    /// Whether each selected commit admits dirty (uncommitted-tree) snapshots.
    admit_dirty: HashMap<String, bool>,
    /// Whether a commit's dirty runs are admitted *only* by the base-branch
    /// dirty-tree exception, which triggers the ephemeral-data warning.
    dirty_base_exception: HashMap<String, bool>,
    /// First-parent topological index of the merge-base, used by branch mode to
    /// split base-side history from the branch's own commits. `None` when no base
    /// is known or the merge-base is off the analyzed chain.
    merge_base_index: Option<usize>,
    /// Whether the target's tip *is* its own merge-base with the base (or no base
    /// is known): the signal that this is an official base-branch view.
    tip_is_merge_base: bool,
    /// Whether a dirty (uncommitted-tree) snapshot on top of the base is admitted
    /// — the working tree is dirty under the `WhenWorkingTreeDirty` policy — which
    /// makes even an on-the-base-branch view an unnamed feature branch.
    dirty_tip_admitted: bool,
}

/// Resolves the git topology for a selection: the target ref's first-parent
/// ancestry, the merge-base with the base ref, and the per-commit dirty-admission
/// flags. Requires a repository — an unresolvable target ref is an error rather
/// than an empty success.
async fn resolve_history<G>(
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
    let target_ref = selection.branch.unwrap_or("HEAD");
    let Some(target_sha) = git.resolve(target_ref).await.map_err(RunError::Io)? else {
        return Err(RunError::Analyze {
            message: format!(
                "this command requires a git repository: could not resolve {target_ref:?}. \
                 Run inside a repository (or pass --repo / --branch)."
            ),
        });
    };

    let base_sha = resolve_base_ref(git, config, selection.base).await?;
    let ancestry = git.first_parent(&target_sha).await.map_err(RunError::Io)?;
    let merge_base = match &base_sha {
        Some(base) => git
            .merge_base(&target_sha, base)
            .await
            .map_err(RunError::Io)?,
        None => None,
    };

    reporter.note(&format!(
        "target ref {target_ref} resolves to {target_sha}; {} on its first-parent line",
        count_noun(ancestry.len(), "commit")
    ));
    reporter.note(&format!(
        "base ref resolves to {}; merge-base with target is {}",
        base_sha.as_deref().unwrap_or("<none>"),
        merge_base.as_deref().unwrap_or("<none>")
    ));

    // The base-branch dirty-tip exception: `analyze`/`list` admit a base-side tip's
    // dirty runs only when the working tree is currently dirty (`--no-dirty` skips
    // both the probe and the exception); `clean` admits them unconditionally so it
    // can remove them regardless of the present working-tree state.
    let dirty_tip_exception = match policy {
        DirtyTipPolicy::Always => !selection.no_dirty,
        DirtyTipPolicy::WhenWorkingTreeDirty => {
            let working_tree_dirty = if selection.no_dirty {
                false
            } else {
                git.is_dirty().await.map_err(RunError::Io)?
            };
            if working_tree_dirty {
                reporter.note(
                    "working tree is dirty: dirty snapshots on a base-side tip will be admitted",
                );
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
    let tip_is_merge_base = merge_base.as_deref().is_none_or(|base| base == target_sha);

    Ok(ResolvedHistory {
        target_ref: target_ref.to_owned(),
        order,
        admit_dirty,
        dirty_base_exception,
        merge_base_index,
        tip_is_merge_base,
        dirty_tip_admitted: dirty_tip_exception,
    })
}

/// The ephemeral-data warning appended when a dirty base-branch-tip run is admitted.
fn dirty_base_exception_warning() -> String {
    "Warning: analysis included dirty runs (with uncommitted changes) on top of the \
     base branch. These may be excluded from future analysis. Switch to a new branch \
     to persist benchmark history of your changes."
        .to_owned()
}

/// Resolves the base ref the target's history is split against, returning its
/// commit SHA or `None` when no base can be determined.
///
/// Precedence: an explicit `--base` (an error if it does not resolve), then the
/// configured `project.default_branch`, then the repository's detected default
/// branch (`origin/HEAD`, else `main`/`master`).
async fn resolve_base_ref<G: GitHistory>(
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

/// Parses the `--format` option, defaulting to text.
fn parse_format(name: Option<&str>) -> Result<ReportFormat, RunError> {
    match name {
        None => Ok(ReportFormat::Text),
        Some(name) => ReportFormat::from_name(name).ok_or_else(|| RunError::Analyze {
            message: format!("unknown report format {name:?}; expected text, json, or markdown"),
        }),
    }
}

/// A human-readable summary of the active facet filters, for `--verbose` notes.
fn describe_facets(facets: &Facets<'_>) -> String {
    let mut parts = Vec::new();
    if let Some(engine) = facets.engine {
        parts.push(format!("engine={engine}"));
    }
    if let Some(target_triple) = facets.target_triple {
        parts.push(format!("target_triple={target_triple}"));
    }
    if let Some(os) = facets.os {
        parts.push(format!("os={os}"));
    }
    if let Some(architecture) = facets.architecture {
        parts.push(format!("architecture={architecture}"));
    }
    if let Some(machine_key) = facets.machine_key {
        parts.push(format!("machine={machine_key}"));
    }
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(", ")
    }
}

/// How many facet-matching candidates were excluded, by reason.
#[derive(Clone, Copy, Debug)]
struct ExclusionTally {
    /// Commit is not on the analyzed first-parent history.
    outside_history: usize,
    /// Dirty snapshot on a base-side commit (clean-only).
    dirty_base: usize,
    /// Effective time is before the `--since` cutoff.
    since: usize,
}

/// Builds a diagnostic hint for the case where stored runs matched the facet
/// filters but none entered the analysis, so the empty outcome explains itself.
///
/// Returns `None` when at least one run was loaded, or when there were no
/// candidates at all (a genuinely empty history needs no special explanation).
fn empty_history_hint(
    loaded_is_empty: bool,
    candidate_count: usize,
    target_ref: &str,
    tally: ExclusionTally,
) -> Option<String> {
    if !loaded_is_empty || candidate_count == 0 {
        return None;
    }

    let mut lines = vec![format!(
        "Found {} for this project, but none entered the analysis:",
        count_noun(candidate_count, "stored run")
    )];
    if tally.dirty_base > 0 {
        lines.push(format!(
            "  - {} on base-branch commits — only clean runs count on the base \
             branch. Commit your working tree (including the configuration file) and re-run, \
             or analyze a feature branch with --branch.",
            count_noun(tally.dirty_base, "dirty (uncommitted-tree) snapshot")
        ));
    }
    if tally.outside_history > 0 {
        lines.push(format!(
            "  - {} on commits outside {target_ref}'s analyzed history — check out the \
             branch they were recorded on, or pass --branch.",
            count_noun(tally.outside_history, "run")
        ));
    }
    if tally.since > 0 {
        lines.push(format!(
            "  - {} older than the --since cutoff.",
            count_noun(tally.since, "run")
        ));
    }
    lines.push("Re-run with --verbose for a per-object explanation.".to_owned());
    Some(lines.join("\n"))
}

/// Parses the `--engine` facet into an [`EngineSystem`], if set.
fn parse_engine(name: Option<&str>) -> Result<Option<EngineSystem>, RunError> {
    match name {
        None => Ok(None),
        Some(name) => EngineSystem::from_name(name)
            .map(Some)
            .ok_or_else(|| RunError::Analyze {
                message: format!("unknown engine {name:?}; expected criterion or callgrind"),
            }),
    }
}

/// Rejects combining `--target-triple` with the derived `--os` / `--architecture`
/// facets. The triple already determines both, so accepting them together invites
/// silently contradictory filters; they are mutually exclusive — specify either the
/// whole triple or its individual components.
fn validate_triple_exclusivity(
    target_triple: Option<&str>,
    os: Option<&str>,
    architecture: Option<&str>,
) -> Result<(), RunError> {
    if target_triple.is_some() && (os.is_some() || architecture.is_some()) {
        return Err(RunError::Analyze {
            message: "--target-triple cannot be combined with --os or --architecture; \
                      the triple already determines both, so specify either the full \
                      target triple or its individual components"
                .to_owned(),
        });
    }
    Ok(())
}

/// Parses the `--since` option into an absolute cutoff instant, if set.
///
/// Three input forms are accepted, tried in order:
///
/// * an RFC 3339 timestamp (`2024-01-01T00:00:00Z`),
/// * a bare `YYYY-MM-DD` date, interpreted at UTC midnight, and
/// * a relative duration in jiff's friendly or ISO 8601 form (`5 months`,
///   `5 months ago`, `P6M`, `2w`), interpreted as *that far in the past* —
///   resolved against the current instant via calendar-correct zoned arithmetic.
///
/// The relative form is normalized through [`Span::abs`] before subtracting, so a
/// duration written with the friendly `ago` suffix (which jiff parses as a
/// *negative* span) still means "this far back" rather than flipping into the
/// future. A lower bound in the future is never a sensible cutoff, so both
/// `5 months` and `5 months ago` resolve to the same past instant.
fn parse_since(value: Option<&str>) -> Result<Option<Timestamp>, RunError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if let Ok(timestamp) = value.parse::<Timestamp>() {
        return Ok(Some(timestamp));
    }
    if let Ok(date) = value.parse::<Date>() {
        // UTC has no DST transitions, so civil midnight always maps to an instant.
        let zoned = date
            .to_zoned(TimeZone::UTC)
            .expect("UTC midnight is always a valid instant");
        return Ok(Some(zoned.timestamp()));
    }
    if let Ok(span) = value.parse::<Span>() {
        return Ok(Some(instant_before_now(span)?));
    }
    Err(RunError::Analyze {
        message: format!(
            "invalid --since value {value:?}; expected an RFC 3339 timestamp, a YYYY-MM-DD \
             date, or a relative duration such as \"6 months\" or \"30 days ago\""
        ),
    })
}

/// Resolves a relative [`Span`] to the instant that far before now, treating the
/// span's magnitude as a look-back regardless of its sign (see [`parse_since`]).
///
/// Calendar units (months, years) have no fixed length, so the subtraction is
/// anchored to the current UTC zoned datetime rather than to a bare duration.
fn instant_before_now(span: Span) -> Result<Timestamp, RunError> {
    let now = Timestamp::now().to_zoned(TimeZone::UTC);
    now.checked_sub(span.abs())
        .map(|zoned| zoned.timestamp())
        .map_err(|error| RunError::Analyze {
            message: format!("--since duration is out of the representable range: {error}"),
        })
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
    use crate::model::{BenchmarkId, Metric, MetricKind, ResultRecord};
    use crate::report::RecordingReporter;
    use crate::storage::{MemoryStorage, Storage};

    use super::*;

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).expect("seconds within range")
    }

    /// A minimal configuration; `analyze_with` only reads `project.default_branch`.
    fn config() -> Config {
        parse_config("[storage.local]\npath = \"./data\"\n").expect("config parses")
    }

    /// Builds a stored result set carrying one record with one `Ir` metric.
    fn ir_set(effective: i64, commit: &str, value: f64) -> ResultSet {
        let time = ts(effective);
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
                value,
                Some("count".to_owned()),
            )],
        );
        ResultSet::new(context, vec![record])
    }

    /// The clean object key for `commit` in the callgrind/linux partition.
    fn clean_key(commit: &str) -> String {
        format!("v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/clean.json")
    }

    /// The clean object key for `commit` in an arbitrary engine/triple/machine partition.
    fn clean_key_in(engine: &str, triple: &str, machine: &str, commit: &str) -> String {
        format!("v2/folo/{engine}/{triple}/{machine}/{commit}/clean.json")
    }

    /// A stored result set whose single record carries two metrics (`Ir` and
    /// `EstimatedCycles`), so its partition reconstructs two distinct series.
    fn two_metric_set(effective: i64, commit: &str, ir: f64, cycles: f64) -> ResultSet {
        let time = ts(effective);
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
            vec![
                Metric::new(
                    "Ir".to_owned(),
                    MetricKind::InstructionCount,
                    ir,
                    Some("count".to_owned()),
                ),
                Metric::new(
                    "EstimatedCycles".to_owned(),
                    MetricKind::EstimatedCycles,
                    cycles,
                    Some("count".to_owned()),
                ),
            ],
        );
        ResultSet::new(context, vec![record])
    }

    /// A dirty snapshot key for `commit` taken at `unix`.
    fn dirty_key(commit: &str, unix: i64) -> String {
        format!("v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/dirty-{unix}.json")
    }

    /// Stores a value at `key` in `storage`, panicking on failure (test helper).
    fn store(storage: &MemoryStorage, key: &str, set: &ResultSet) {
        let json = set.to_json().expect("result set serializes");
        block_on(storage.put(key, json.as_bytes())).expect("store succeeds");
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

    /// A linear master history `c0 - c1 - c2 - c3 - c4 - c5`, HEAD at the tip.
    ///
    /// Long enough to host a sustained level shift with at least two points on
    /// each side, which the change-point detector requires before it flags.
    fn linear6_git() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .commit("c4", Some("c3"))
            .commit("c5", Some("c4"))
            .branch("master", "c5")
            .head("master")
            .mark_default("master");
        git
    }

    /// A six-commit master history `c0..c5` with a feature branch off `c1`,
    /// HEAD on the feature branch. The longer master line lets `--branch master`
    /// reconstruct a sustained step.
    fn feature6_git() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .commit("c4", Some("c3"))
            .commit("c5", Some("c4"))
            .commit("f1", Some("c1"))
            .commit("f2", Some("f1"))
            .branch("master", "c5")
            .branch("feature", "f2")
            .head("feature")
            .mark_default("master");
        git
    }

    /// Seeds a clean linear sustained-step history (`c0..c5` =
    /// 100,100,100,130,130,130) under the default partition, so the change-point
    /// detector flags a single major regression at `c3`.
    fn seed_linear_step(storage: &MemoryStorage) {
        for (index, value) in [100.0, 100.0, 100.0, 130.0, 130.0, 130.0]
            .into_iter()
            .enumerate()
        {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
    }

    fn options() -> AnalyzeOptions {
        AnalyzeOptions::default()
    }

    /// A fixed clock anchor for the history-mode default `--since` window in unit
    /// tests. The seeded data sits at the Unix epoch (`ts(0..)`); anchoring here
    /// keeps the default six-month look-back well before it, so the default window
    /// never drops a seeded point.
    fn now_anchor() -> Timestamp {
        Timestamp::from_second(0).expect("epoch is a valid timestamp")
    }

    #[test]
    fn auto_mode_detects_history_only_for_a_clean_tip_at_the_merge_base() {
        // A clean base branch whose tip is its own merge-base is history mode.
        assert_eq!(auto_mode(true, false), AnalysisMode::History);
        // Commits past the merge-base, or a dirty tree admitting tip snapshots,
        // make it a (possibly unnamed) feature branch.
        assert_eq!(auto_mode(false, false), AnalysisMode::Branch);
        assert_eq!(auto_mode(true, true), AnalysisMode::Branch);
        assert_eq!(auto_mode(false, true), AnalysisMode::Branch);
    }

    #[test]
    fn parse_mode_resolves_auto_to_none_and_rejects_unknown() {
        assert_eq!(parse_mode(None).expect("none parses"), None);
        assert_eq!(parse_mode(Some("auto")).expect("auto parses"), None);
        assert_eq!(
            parse_mode(Some("history")).expect("history parses"),
            Some(AnalysisMode::History)
        );
        assert_eq!(
            parse_mode(Some("branch")).expect("branch parses"),
            Some(AnalysisMode::Branch)
        );
        assert_eq!(
            parse_mode(Some("tip")).expect("tip parses"),
            Some(AnalysisMode::Tip)
        );
        let error = parse_mode(Some("weekly")).unwrap_err();
        assert!(
            error.to_string().contains("unknown analysis mode"),
            "{error}"
        );
    }

    #[test]
    fn resolve_since_applies_a_default_only_in_history_mode() {
        let now = "2024-06-01T00:00:00Z".parse::<Timestamp>().unwrap();
        // History mode without an explicit value falls back to the six-month window.
        let history = resolve_since(None, AnalysisMode::History, now)
            .expect("default resolves")
            .expect("history has a default cutoff");
        assert_eq!(
            history,
            "2023-12-01T00:00:00Z".parse::<Timestamp>().unwrap()
        );
        // Branch and tip modes have no default cutoff.
        assert_eq!(
            resolve_since(None, AnalysisMode::Branch, now).expect("resolves"),
            None
        );
        assert_eq!(
            resolve_since(None, AnalysisMode::Tip, now).expect("resolves"),
            None
        );
        // An explicit value always wins, even in branch mode.
        let explicit = resolve_since(Some("2024-01-01"), AnalysisMode::Branch, now)
            .expect("explicit parses")
            .expect("explicit produces a cutoff");
        assert_eq!(
            explicit,
            "2024-01-01T00:00:00Z".parse::<Timestamp>().unwrap()
        );
    }

    #[test]
    fn default_history_since_subtracts_six_calendar_months() {
        let now = "2024-03-31T00:00:00Z".parse::<Timestamp>().unwrap();
        // Calendar arithmetic, not a fixed number of days: six months before
        // 2024-03-31 is 2023-09-30 (September has 30 days).
        let cutoff = default_history_since(now).expect("resolves");
        assert_eq!(cutoff, "2023-09-30T00:00:00Z".parse::<Timestamp>().unwrap());
    }

    /// Runs `analyze_with` and unwraps the rendered report and regression count.
    fn analyze(
        git: &FakeGitHistory,
        storage: &MemoryStorage,
        project: &str,
        options: &AnalyzeOptions,
    ) -> (String, usize) {
        let reporter = RecordingReporter::new();
        let outcome = block_on(analyze_with(
            git,
            storage,
            project,
            &config(),
            options,
            now_anchor(),
            &reporter,
        ))
        .expect("analysis runs");
        match outcome {
            RunOutcome::Analyzed {
                report,
                regressions,
                ..
            } => (report, regressions),
            RunOutcome::Completed { message } => (message, 0),
        }
    }

    #[test]
    fn analyze_without_a_repository_is_an_error() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let git = FakeGitHistory::new(); // No commits: HEAD does not resolve.
        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &options(),
            now_anchor(),
            &RecordingReporter::new(),
        ))
        .unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
        assert!(
            error.to_string().contains("requires a git repository"),
            "{error}"
        );
    }

    #[test]
    fn empty_history_reports_no_changes() {
        let storage = MemoryStorage::new();
        let git = linear_git();
        let (report, regressions) = analyze(&git, &storage, "folo", &options());
        assert_eq!(regressions, 0);
        assert!(report.contains("No notable changes detected."), "{report}");
    }

    #[test]
    fn official_view_detects_a_clean_regression_in_topology_order() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let git = linear6_git();
        let (report, regressions) = analyze(&git, &storage, "folo", &options());
        assert_eq!(regressions, 1);
        assert!(report.contains("regression"), "{report}");
        assert!(report.contains("nm::observe/pull/Ir"), "{report}");
    }

    #[test]
    fn per_set_report_counts_runs_and_series_independently() {
        let storage = MemoryStorage::new();
        // Set A — callgrind/linux/synthetic: three runs (c0..c2), each carrying two
        // metrics so the set reconstructs two distinct series.
        for index in 0..3 {
            let commit = format!("c{index}");
            let second = i64::from(index);
            store(
                &storage,
                &clean_key(&commit),
                &two_metric_set(second, &commit, 100.0, 200.0),
            );
        }
        // Set B — callgrind/darwin/synthetic: two runs (c0..c1), each carrying one
        // metric so the set reconstructs a single series. Distinct run AND series
        // counts from set A make an `==`/`!=` swap in either per-set tally observable.
        for index in 0..2 {
            let commit = format!("c{index}");
            let second = i64::from(index);
            store(
                &storage,
                &clean_key_in("callgrind", "aarch64-apple-darwin", "synthetic", &commit),
                &ir_set(second, &commit, 100.0),
            );
        }

        let git = linear_git();
        let mut options = options();
        options.format = Some("json".to_owned());
        let (report, _) = analyze(&git, &storage, "folo", &options);

        let parsed: serde_json::Value = serde_json::from_str(&report).expect("report is JSON");
        let sets = parsed["sets"].as_array().expect("sets array present");

        let set_a = sets
            .iter()
            .find(|set| set["target_triple"] == "x86_64-unknown-linux-gnu")
            .expect("linux set present");
        assert_eq!(set_a["runs"], 3, "{report}");
        assert_eq!(set_a["series"], 2, "{report}");

        let set_b = sets
            .iter()
            .find(|set| set["target_triple"] == "aarch64-apple-darwin")
            .expect("darwin set present");
        assert_eq!(set_b["runs"], 2, "{report}");
        assert_eq!(set_b["series"], 1, "{report}");
    }

    #[test]
    fn series_order_follows_topology_not_effective_time() {
        // Topology is c0..c5 with a sustained step at c3 (100,100,100,130,130,130),
        // but the effective clock is reversed (c0 newest, c5 oldest). Ordering by
        // topology reconstructs the rising step and flags a regression; ordering by
        // effective time would reverse it into a falling step (an improvement, no
        // regression). So a single detected regression proves topology won.
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0, 130.0, 130.0, 130.0]
            .into_iter()
            .enumerate()
        {
            let commit = format!("c{index}");
            // Reverse the clock: c0 is newest, c5 is oldest by effective time.
            let second = 100 - i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        let git = linear6_git();
        let (_, regressions) = analyze(&git, &storage, "folo", &options());
        assert_eq!(regressions, 1, "the step must be read in topology order");
    }

    #[test]
    fn official_view_excludes_dirty_runs() {
        // A dirty snapshot on the master tip must not enter the official timeline.
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        // A wildly different dirty value on the tip: if admitted it would flag.
        store(&storage, &dirty_key("c3", 500), &ir_set(500, "c3", 999.0));
        let git = linear_git();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, regressions) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "the dirty tip run is excluded");
        assert_eq!(regressions, 0);
    }

    #[test]
    fn feature_view_admits_dirty_after_the_merge_base() {
        // feature branched at c1; the target side rises at f1 and the dirty f2
        // snapshot sustains the new level. The dirty run is admitted (runs == 4)
        // and is essential to the flag: without it the lone f1 point cannot satisfy
        // the change-point detector's persistence requirement.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &clean_key("f1"), &ir_set(2, "f1", 130.0));
        store(&storage, &dirty_key("f2", 3), &ir_set(3, "f2", 130.0));
        let git = feature_git();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, regressions) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 4, "the dirty f2 snapshot is admitted");
        assert_eq!(regressions, 1, "the admitted dirty f2 completes the step");
    }

    #[test]
    fn no_dirty_suppresses_the_target_side_dirty_run() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &clean_key("f1"), &ir_set(2, "f1", 100.0));
        store(&storage, &dirty_key("f2", 3), &ir_set(3, "f2", 130.0));
        let git = feature_git();

        let opts = AnalyzeOptions {
            no_dirty: true,
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "--no-dirty drops the dirty snapshot");
    }

    #[test]
    fn dirty_run_on_a_base_side_commit_is_excluded() {
        // A dirty snapshot on c1 (at/before the merge-base) is base-side, so even
        // on the feature view it is clean-only and the dirty file is excluded.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &dirty_key("c1", 9), &ir_set(9, "c1", 999.0));
        store(&storage, &clean_key("f1"), &ir_set(2, "f1", 100.0));
        let git = feature_git();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "the base-side dirty c1 run is excluded");
    }

    #[test]
    fn all_dirty_on_base_yields_zero_runs_with_a_hint() {
        // The user-reported trap: on the default branch's tip every run is a
        // dirty snapshot (e.g. because the config file was never committed), so
        // all are excluded and the empty outcome must explain itself with a hint
        // and per-object verbose notes rather than looking like "no data".
        let storage = MemoryStorage::new();
        store(&storage, &dirty_key("c3", 100), &ir_set(100, "c3", 100.0));
        store(&storage, &dirty_key("c3", 200), &ir_set(200, "c3", 130.0));
        let git = linear_git();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let reporter = RecordingReporter::new();
        let outcome = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            now_anchor(),
            &reporter,
        ))
        .expect("analysis runs");
        let RunOutcome::Analyzed { report, .. } = outcome else {
            panic!("expected an analysis outcome");
        };

        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(
            parsed["runs"], 0,
            "every dirty-on-base snapshot is excluded"
        );
        let hint = parsed["hint"]
            .as_str()
            .expect("a diagnostic hint is present");
        assert!(
            hint.contains("Found 2 stored runs"),
            "the hint should count the stored runs: {hint}"
        );
        assert!(
            hint.contains("dirty"),
            "the hint should explain the dirty-on-base exclusion: {hint}"
        );

        assert!(
            reporter.contains("dirty snapshot on a base-side commit"),
            "verbose notes should explain each exclusion: {:?}",
            reporter.notes()
        );
    }

    #[test]
    fn dirty_tree_on_base_branch_admits_tip_dirty_runs_with_a_warning() {
        // On the base branch (official view) with a currently-dirty working tree,
        // the dirty snapshots on the tip are the user's in-flight work and ARE
        // admitted, with a warning that they are ephemeral. Two snapshots at the
        // raised level complete a sustained step over the clean baseline.
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        store(&storage, &dirty_key("c3", 300), &ir_set(300, "c3", 130.0));
        store(&storage, &dirty_key("c3", 400), &ir_set(400, "c3", 130.0));
        let mut git = linear_git();
        git.mark_dirty();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let reporter = RecordingReporter::new();
        let outcome = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            now_anchor(),
            &reporter,
        ))
        .expect("analysis runs");
        let RunOutcome::Analyzed {
            report,
            regressions,
            ..
        } = outcome
        else {
            panic!("expected an analysis outcome");
        };

        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 5, "both dirty tip snapshots are admitted");
        assert_eq!(regressions, 1, "the dirty tip snapshots complete the step");
        let warning = parsed["warning"]
            .as_str()
            .expect("the ephemeral-data warning is present");
        assert!(
            warning.contains("dirty runs") && warning.contains("Switch to a new branch"),
            "{warning}"
        );
        assert!(
            reporter.contains("ephemeral"),
            "a verbose note should flag the ephemeral inclusion: {:?}",
            reporter.notes()
        );
    }

    #[test]
    fn clean_tree_on_base_branch_excludes_dirty_and_warns_nothing() {
        // The exception is gated on the working tree being dirty: with a clean
        // tree the base-tip dirty snapshot stays excluded and no warning fires.
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        store(&storage, &dirty_key("c3", 300), &ir_set(300, "c3", 999.0));
        let git = linear_git(); // Clean working tree (the default).

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "the dirty tip run stays excluded");
        assert!(
            parsed["warning"].is_null(),
            "no warning when the tree is clean"
        );
    }

    #[test]
    fn no_dirty_overrides_the_dirty_tree_exception() {
        // `--no-dirty` skips the dirtiness probe and the exception, so even with a
        // dirty tree the base-tip dirty snapshot is excluded and no warning fires.
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        store(&storage, &dirty_key("c3", 300), &ir_set(300, "c3", 999.0));
        let mut git = linear_git();
        git.mark_dirty();

        let opts = AnalyzeOptions {
            no_dirty: true,
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "--no-dirty drops the dirty tip snapshot");
        assert!(parsed["warning"].is_null(), "no warning under --no-dirty");
    }

    #[test]
    fn only_the_tip_admits_dirty_under_the_exception() {
        // With a dirty tree the exception applies ONLY to the base-branch tip: a
        // dirty snapshot on an earlier base-side commit stays excluded while the
        // tip's dirty snapshot is admitted (and warned).
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0, 100.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        store(&storage, &dirty_key("c1", 150), &ir_set(150, "c1", 999.0));
        store(&storage, &dirty_key("c3", 300), &ir_set(300, "c3", 130.0));
        let mut git = linear_git();
        git.mark_dirty();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let reporter = RecordingReporter::new();
        let outcome = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            now_anchor(),
            &reporter,
        ))
        .expect("analysis runs");
        let RunOutcome::Analyzed { report, .. } = outcome else {
            panic!("expected an analysis outcome");
        };
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(
            parsed["runs"], 5,
            "only the tip's dirty run joins the four clean runs"
        );
        assert!(
            !parsed["warning"].is_null(),
            "the tip's admitted dirty run warns: {report}"
        );
        assert!(
            reporter.contains("dirty snapshot on a base-side commit"),
            "the earlier base-side dirty run is still excluded: {:?}",
            reporter.notes()
        );
    }

    #[test]
    fn commits_off_the_first_parent_chain_are_excluded() {
        // c2 and c3 are on master but not on feature's first-parent ancestry, so
        // their runs never enter a feature-view analysis.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &clean_key("c2"), &ir_set(2, "c2", 999.0));
        store(&storage, &clean_key("c3"), &ir_set(3, "c3", 999.0));
        store(&storage, &clean_key("f1"), &ir_set(4, "f1", 100.0));
        let git = feature_git();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "c2 and c3 are off the feature mainline");
    }

    #[test]
    fn explicit_branch_selects_the_official_master_view() {
        // From a feature checkout, `--branch master` analyzes master's own history:
        // six clean commits with a sustained step at c3.
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0, 130.0, 130.0, 130.0]
            .into_iter()
            .enumerate()
        {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        let git = feature6_git();

        let opts = AnalyzeOptions {
            branch: Some("master".to_owned()),
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, regressions) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 6, "master's six commits");
        assert_eq!(regressions, 1);
    }

    #[test]
    fn within_a_commit_clean_precedes_dirty() {
        // On a target-side commit, a clean run and dirty snapshots both load; the
        // clean run is the baseline and the later dirty values are the latest
        // points. Two dirty snapshots at the raised level complete a sustained step.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &clean_key("f1"), &ir_set(2, "f1", 100.0));
        store(&storage, &clean_key("f2"), &ir_set(3, "f2", 100.0));
        store(&storage, &dirty_key("f2", 4), &ir_set(4, "f2", 130.0));
        store(&storage, &dirty_key("f2", 5), &ir_set(5, "f2", 130.0));
        let git = feature_git();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (_, regressions) = analyze(&git, &storage, "folo", &opts);
        assert_eq!(regressions, 1, "the dirty f2 values are the latest points");
    }

    #[test]
    fn os_facet_selects_one_set() {
        // Two sets differing only by OS; `--os windows` reports just the one.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(
            &storage,
            "v2/folo/callgrind/x86_64-pc-windows-msvc/synthetic/c0/clean.json",
            &ir_set(0, "c0", 100.0),
        );
        let git = linear_git();

        let opts = AnalyzeOptions {
            os: Some("windows".to_owned()),
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 1, "only the windows set is loaded");
        assert_eq!(parsed["sets"].as_array().unwrap().len(), 1, "{report}");
    }

    #[test]
    fn target_triple_facet_selects_one_set() {
        // Two sets differing only by triple; `--target-triple` reports just the one.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(
            &storage,
            "v2/folo/callgrind/x86_64-pc-windows-msvc/synthetic/c0/clean.json",
            &ir_set(0, "c0", 100.0),
        );
        let git = linear_git();

        let opts = AnalyzeOptions {
            target_triple: Some("x86_64-unknown-linux-gnu".to_owned()),
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 1, "only the linux-gnu triple is loaded");
        assert_eq!(parsed["sets"].as_array().unwrap().len(), 1, "{report}");
        assert_eq!(
            parsed["sets"][0]["target_triple"], "x86_64-unknown-linux-gnu",
            "{report}"
        );
    }

    #[test]
    fn target_triple_combined_with_os_or_architecture_is_an_error() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        let git = linear_git();

        for conflicting in [
            AnalyzeOptions {
                target_triple: Some("x86_64-unknown-linux-gnu".to_owned()),
                os: Some("linux".to_owned()),
                ..options()
            },
            AnalyzeOptions {
                target_triple: Some("x86_64-unknown-linux-gnu".to_owned()),
                architecture: Some("x86_64".to_owned()),
                ..options()
            },
        ] {
            let error = block_on(analyze_with(
                &git,
                &storage,
                "folo",
                &config(),
                &conflicting,
                now_anchor(),
                &RecordingReporter::new(),
            ))
            .unwrap_err();
            assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
            assert!(
                error
                    .to_string()
                    .contains("--target-triple cannot be combined"),
                "{error}"
            );
        }
    }

    #[test]
    fn two_sets_produce_two_report_sections() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(
            &storage,
            "v2/folo/callgrind/x86_64-pc-windows-msvc/synthetic/c0/clean.json",
            &ir_set(0, "c0", 100.0),
        );
        let git = linear_git();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["sets"].as_array().unwrap().len(), 2, "{report}");
    }

    #[test]
    fn engine_facet_narrows_the_listing() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(
            &storage,
            "v2/folo/criterion/x86_64-pc-windows-msvc/m1/c0/clean.json",
            &ir_set(0, "c0", 100.0),
        );
        let git = linear_git();

        let opts = AnalyzeOptions {
            engine: Some("callgrind".to_owned()),
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 1, "only the callgrind object is loaded");
    }

    #[test]
    fn since_window_excludes_earlier_runs() {
        let storage = MemoryStorage::new();
        // c0,c1 at epoch seconds 0,1; c2,c3 at 2,3. `--since` epoch 2 keeps c2,c3.
        for (index, value) in [100.0, 100.0, 100.0, 130.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        let git = linear_git();

        let opts = AnalyzeOptions {
            since: Some("1970-01-01T00:00:02Z".to_owned()),
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 2, "only c2 and c3 are within the window");
    }

    #[test]
    fn history_is_found_for_a_project_id_that_requires_sanitizing() {
        // `run` stores under the sanitized project segment, so `analyze` must list
        // under that same segment; listing under the raw id would miss the history.
        let storage = MemoryStorage::new();
        let raw_project = "my project/v2";
        let sanitized = sanitize_segment(raw_project);
        for (index, value) in [100.0, 100.0, 100.0, 130.0, 130.0, 130.0]
            .into_iter()
            .enumerate()
        {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            let key = format!(
                "v2/{sanitized}/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/clean.json"
            );
            store(&storage, &key, &ir_set(second, &commit, value));
        }
        let git = linear6_git();

        let (report, regressions) = analyze(&git, &storage, raw_project, &options());
        assert_eq!(
            regressions, 1,
            "history stored under the sanitized key must be found"
        );
        assert!(report.contains("nm::observe/pull/Ir"), "{report}");
    }

    #[test]
    fn analyzed_outcome_is_always_successful() {
        // The exit code no longer depends on findings: even a flagged regression
        // yields a successful outcome (the signal lives in the report JSON).
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let git = linear6_git();

        let outcome = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &options(),
            now_anchor(),
            &RecordingReporter::new(),
        ))
        .expect("analysis runs");
        assert!(outcome.is_success(), "findings must never fail the build");
    }

    #[test]
    fn json_format_is_rendered() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 10.0));
        let git = linear_git();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
        assert_eq!(parsed["project"], "folo");
        assert_eq!(parsed["runs"], 1);
    }

    #[test]
    fn non_json_objects_are_skipped() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 10.0));
        // A stray non-result object under the prefix must be ignored, not parsed.
        block_on(storage.put("v2/folo/callgrind/README.txt", b"not json")).unwrap();
        let git = linear_git();

        let (_, regressions) = analyze(&git, &storage, "folo", &options());
        assert_eq!(regressions, 0);
    }

    #[test]
    fn malformed_stored_object_is_an_analyze_error() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c0"), b"{ not valid")).unwrap();
        let git = linear_git();

        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &options(),
            now_anchor(),
            &RecordingReporter::new(),
        ))
        .unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn invalid_utf8_object_is_an_analyze_error() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c0"), &[0xff, 0xfe])).unwrap();
        let git = linear_git();

        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &options(),
            now_anchor(),
            &RecordingReporter::new(),
        ))
        .unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn unknown_format_is_rejected() {
        let storage = MemoryStorage::new();
        let git = linear_git();
        let opts = AnalyzeOptions {
            format: Some("yaml".to_owned()),
            ..options()
        };
        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            now_anchor(),
            &RecordingReporter::new(),
        ))
        .unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn unknown_engine_is_rejected() {
        let storage = MemoryStorage::new();
        let git = linear_git();
        let opts = AnalyzeOptions {
            engine: Some("dhat".to_owned()),
            ..options()
        };
        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            now_anchor(),
            &RecordingReporter::new(),
        ))
        .unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn triple_exclusivity_allows_either_side_but_not_both() {
        // The triple alone, or its derived parts alone, are fine.
        validate_triple_exclusivity(Some("x86_64-unknown-linux-gnu"), None, None).unwrap();
        validate_triple_exclusivity(None, Some("linux"), Some("x86_64")).unwrap();
        validate_triple_exclusivity(None, None, None).unwrap();
        // Combining the triple with either derived facet is rejected.
        validate_triple_exclusivity(Some("x86_64-unknown-linux-gnu"), Some("linux"), None)
            .unwrap_err();
        validate_triple_exclusivity(Some("x86_64-unknown-linux-gnu"), None, Some("x86_64"))
            .unwrap_err();
    }

    #[test]
    fn unresolvable_base_is_rejected() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let git = linear_git();
        let opts = AnalyzeOptions {
            base: Some("does-not-exist".to_owned()),
            ..options()
        };
        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            now_anchor(),
            &RecordingReporter::new(),
        ))
        .unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
        assert!(error.to_string().contains("--base"), "{error}");
    }

    #[test]
    fn configured_default_branch_is_used_as_the_base() {
        // The config names `master` as the default branch; analyzing the feature
        // branch must split at the master merge-base even without `--base`.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &dirty_key("c1", 9), &ir_set(9, "c1", 999.0));
        store(&storage, &clean_key("f1"), &ir_set(2, "f1", 100.0));
        // A git history that does NOT advertise a default branch, so resolution
        // must fall through to the configured `project.default_branch`.
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("f1", Some("c1"))
            .branch("master", "c1")
            .branch("feature", "f1")
            .head("feature");
        let config = parse_config(
            "[project]\ndefault_branch = \"master\"\n[storage.local]\npath = \"./d\"\n",
        )
        .unwrap();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let outcome = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config,
            &opts,
            now_anchor(),
            &RecordingReporter::new(),
        ))
        .expect("analysis runs");
        let RunOutcome::Analyzed { report, .. } = outcome else {
            panic!("expected an analyzed outcome");
        };
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        // c1's dirty run is base-side (excluded); c0, c1 clean and f1 clean load.
        assert_eq!(
            parsed["runs"], 3,
            "base-side dirty c1 excluded via config base"
        );
    }

    #[test]
    fn since_accepts_timestamp_and_date() {
        assert_eq!(
            parse_since(Some("2024-01-01T00:00:00Z")).unwrap(),
            Some("2024-01-01T00:00:00Z".parse().unwrap())
        );
        assert_eq!(
            parse_since(Some("2024-01-01")).unwrap(),
            Some("2024-01-01T00:00:00Z".parse().unwrap())
        );
        assert_eq!(parse_since(None).unwrap(), None);
    }

    #[test]
    #[cfg_attr(miri, ignore = "relative `--since` parsing reads the wall clock")]
    fn since_accepts_relative_durations_as_look_back() {
        // A friendly duration resolves to an instant in the past, and the `ago`
        // suffix (which jiff parses as a negative span) means the same look-back
        // rather than flipping into the future.
        let now = Timestamp::now();
        let plain = parse_since(Some("5 months")).unwrap().expect("a cutoff");
        let with_ago = parse_since(Some("5 months ago"))
            .unwrap()
            .expect("a cutoff");
        assert!(plain < now, "a look-back must be in the past");
        assert!(with_ago < now, "the `ago` suffix must still look back");
        // Both spellings denote the same magnitude, so they land within a tiny
        // wall-clock window of each other (the two `now` reads differ by µs).
        let gap = (plain.as_second() - with_ago.as_second()).abs();
        assert!(gap <= 1, "`5 months` and `5 months ago` agree (gap {gap}s)");
    }

    #[test]
    #[cfg_attr(miri, ignore = "relative `--since` parsing reads the wall clock")]
    fn since_accepts_iso_and_week_durations() {
        let now = Timestamp::now();
        for input in ["P6M", "2w", "30 days", "-P1Y"] {
            let cutoff = parse_since(Some(input)).unwrap().expect("a cutoff");
            assert!(cutoff < now, "{input:?} must resolve to the past");
        }
    }

    #[test]
    fn since_rejects_garbage() {
        let error = parse_since(Some("not-a-date")).unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn metric_filter_limits_series() {
        let storage = MemoryStorage::new();
        let mut set = ir_set(0, "c0", 10.0);
        set.results[0].metrics.push(Metric::new(
            "EstimatedCycles".to_owned(),
            MetricKind::EstimatedCycles,
            20.0,
            Some("count".to_owned()),
        ));
        store(&storage, &clean_key("c0"), &set);
        let git = linear_git();

        let opts = AnalyzeOptions {
            metric: Some("Ir".to_owned()),
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["series"], 1, "only the Ir metric forms a series");
    }
}
